//! Unit tests of shared subplans (`exec/shared.rs`): which groupings are
//! written alike, which places are made readers, and what a reader reads.

use super::*;
use crate::catalog::{MemoryCatalog, QualifiedName};
use crate::db::State;
use crate::logical::Truth;
use crate::physical::Parallel;
use crate::storage::Row;
use crate::txn::NO_TRANSACTION;
use crate::value::{CompOp, Decimal, Value};

fn number(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

/// A scan of table `name` reading `columns`.
fn scan(name: &str, columns: &[usize]) -> Plan {
    let table = QualifiedName { auth: "A".into(), ident: name.into() };
    Plan::SeqScan {
        table,
        columns: columns.to_vec(),
        rows: 9,
        cost: 1.0_f64,
        order: Vec::new(),
        backward: false,
        most: None,
        located: false,
    }
}

fn filter(cond: Cond, input: Plan) -> Plan {
    Plan::Filter { cond, detail: String::new(), est: 3.0_f64, of_read: false, input: Box::new(input) }
}

/// Column 0 above `n`.
fn above(n: i128) -> Cond {
    Cond::Compare(Expr::Column(0), CompOp::Gt, number(n))
}

/// A grouping by hash of `input` by its first column, estimated `est`.
fn grouping(input: Plan, est: f64) -> Plan {
    let (collations, aggs, seeds) = (vec![None], Vec::new(), Vec::new());
    Plan::Aggregate {
        groups: vec![0],
        collations,
        aggs,
        seeds,
        detail: "shown".into(),
        input: Box::new(input),
        est,
        streamed: false,
        first: None,
    }
}

fn part(input: Plan) -> Plan {
    Plan::Parallel(Box::new(Parallel { degree: 4, input }))
}

fn values(of: &[i128]) -> Plan {
    Plan::Values { rows: of.iter().map(|&n| vec![number(n)]).collect(), detail: String::new() }
}

fn rows_of(plan: Plan) -> Vec<Row> {
    let state = Arc::new(State::with_builtins(Box::new(MemoryCatalog::default())));
    execute(plan, &state, NO_TRANSACTION).collect::<Result<_>>().unwrap()
}

/// Verifies: [DESIGN] RULES.md D1129: a grouping by hash is written the
/// same whatever its estimate, what EXPLAIN shows of it and its mark of a
/// part run in parallel; and otherwise of another table, of other
/// columns read, under another filter, and under none. A streamed
/// grouping, one over a located scan, and one whose filter holds a
/// parameter's value or a subquery are not written.
#[test]
fn groupings_are_written_alike_only_of_the_same_plan() {
    let base = || grouping(filter(above(5), scan("T", &[0, 2])), 7.0_f64);
    let key = written::grouping(&base()).expect("a grouping of a filtered scan is written");
    assert_eq!(written::grouping(&grouping(filter(above(5), scan("T", &[0, 2])), 99.0_f64)), Some(key.clone()), "its estimate aside");
    assert_eq!(written::grouping(&part(base())), Some(key.clone()), "its mark aside");
    for other in [
        grouping(filter(above(5), scan("U", &[0, 2])), 7.0_f64),
        grouping(filter(above(5), scan("T", &[0, 1])), 7.0_f64),
        grouping(filter(above(6), scan("T", &[0, 2])), 7.0_f64),
        grouping(scan("T", &[0, 2]), 7.0_f64),
    ] {
        let written = written::grouping(&other).expect("written");
        assert_ne!(written, key, "{other:?}");
    }
    let Plan::Aggregate { groups, collations, aggs, seeds, detail, input, est, .. } = base() else { panic!("a grouping") };
    let streamed = Plan::Aggregate { groups, collations, aggs, seeds, detail, input, est, streamed: true, first: None };
    assert_eq!(written::grouping(&streamed), None, "streamed, it is no grouping by hash");
    assert_eq!(written::grouping(&filter(above(5), scan("T", &[0]))), None, "a filter is no grouping");
    let located = Plan::SeqScan {
        table: QualifiedName { auth: "A".into(), ident: "T".into() },
        columns: vec![0],
        rows: 9,
        cost: 1.0_f64,
        order: Vec::new(),
        backward: false,
        most: None,
        located: true,
    };
    assert_eq!(written::grouping(&grouping(located, 1.0_f64)), None, "a located scan's rows end in their ids");
    let outer = Cond::Compare(Expr::Column(0), CompOp::Gt, Expr::Outer(0, "X".into()));
    assert_eq!(written::grouping(&grouping(filter(outer, scan("T", &[0])), 1.0_f64)), None, "an outer reference");
    assert_eq!(written::grouping(&grouping(filter(Cond::Const(Truth::True), values(&[1])), 1.0_f64)), None, "no scan below");
}

/// Verifies: [DESIGN] RULES.md D1129: of two plans, a grouping both hold
/// is made a reader of one cell in each, under its mark where it has
/// one; a grouping one alone holds, and the same grouping under another
/// filter, stay as they are; and a grouping held once counts once though
/// it is marked.
#[test]
fn only_a_grouping_two_places_hold_is_made_a_reader() {
    let shared = || grouping(filter(above(5), scan("T", &[0, 2])), 7.0_f64);
    let mut first = Plan::Product { left: Box::new(part(shared())), right: Box::new(grouping(scan("U", &[0]), 1.0_f64)), est: 1.0_f64 };
    let mut second = grouping(
        Plan::Product { left: Box::new(shared()), right: Box::new(grouping(filter(above(6), scan("T", &[0, 2])), 1.0_f64)), est: 1.0_f64 },
        1.0_f64,
    );
    let mut places = HashMap::new();
    assert!(noted(&mut first, &mut places, 0));
    assert!(places.values().all(|&count| count == 1), "a marked grouping is one place: {places:?}");
    assert!(noted(&mut second, &mut places, 0));
    let cells: HashMap<String, SharedRows> =
        places.into_iter().filter(|(_, count)| *count > 1).map(|(key, _)| (key, SharedRows::default())).collect();
    assert_eq!(cells.len(), 1, "one grouping is held twice");
    read_once(&mut first, &cells, 0);
    read_once(&mut second, &cells, 0);
    let Plan::Product { left, right, .. } = &first else { panic!("a product") };
    let Plan::Shared(s) = &**left else { panic!("the grouping both hold is a reader: {left:?}") };
    assert!(matches!(s.input, Plan::Parallel(_)), "its mark is kept under the reader");
    assert!(matches!(**right, Plan::Aggregate { .. }), "a grouping one alone holds stays");
    let Plan::Aggregate { input, .. } = &second else { panic!("the grouping over the product stays") };
    let Plan::Product { left: inner_left, right: inner_right, .. } = &**input else { panic!("a product") };
    assert!(matches!(**inner_left, Plan::Shared(_)));
    assert!(matches!(**inner_right, Plan::Aggregate { .. }), "under another filter it is another grouping");
}

/// Verifies: [DESIGN] RULES.md D1129: the reader that opens first runs
/// its subplan, and each reader of the cell after it reads those rows,
/// its own subplan not run, even one that would raise; a reader of
/// another cell runs its own; and a subplan that raises an error keeps
/// no rows, so the next reader runs its own.
#[test]
fn readers_of_a_cell_read_what_the_first_ran() {
    let cell = SharedRows::default();
    let reader = |input: Plan, rows: &SharedRows| Plan::Shared(Box::new(Shared { input, rows: rows.clone() }));
    let shown = |rows: Vec<Row>| rows.iter().map(|r| r[0].to_string()).collect::<Vec<_>>();
    assert_eq!(shown(rows_of(reader(values(&[1, 1, 2]), &cell))), ["1", "1", "2"], "the first runs its subplan, duplicates kept");
    assert_eq!(shown(rows_of(reader(values(&[7]), &cell))), ["1", "1", "2"], "the second reads the first's rows");
    let state = Arc::new(State::with_builtins(Box::new(MemoryCatalog::default())));
    let broken_plan = || Plan::Project { exprs: vec![Expr::Column(3)], detail: String::new(), input: Box::new(values(&[1])) };
    let read: Vec<Row> = execute(reader(broken_plan(), &cell), &state, NO_TRANSACTION).collect::<Result<_>>().unwrap();
    assert_eq!(shown(read), ["1", "1", "2"], "a later reader's own subplan, which would raise, is not run");
    assert_eq!(shown(rows_of(reader(values(&[7]), &SharedRows::default()))), ["7"], "another cell's reader runs its own");
    let empty = SharedRows::default();
    assert!(rows_of(reader(values(&[]), &empty)).is_empty());
    assert!(rows_of(reader(values(&[7]), &empty)).is_empty(), "no rows are rows kept");
    let failing = SharedRows::default();
    assert!(execute(reader(broken_plan(), &failing), &state, NO_TRANSACTION).any(|r| r.is_err()));
    assert_eq!(shown(rows_of(reader(values(&[7]), &failing))), ["7"], "a failed run keeps nothing");
}
