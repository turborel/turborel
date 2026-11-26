//! Unit tests of the parts a plan runs in parallel (RULES.md R-TEST-1).

use std::sync::Arc;

use super::*;
use crate::value::{Decimal, Value};

/// Verifies: [DESIGN] RULES.md D1128: a plan reads its input whole
/// before its first row if it is a grouping or DISTINCT not streamed,
/// or a sort, under any projections and filters; a streamed grouping
/// or DISTINCT, or a projection of rows as they come, does not, so a
/// run that stops at its first rows keeps it as it is.
#[test]
fn a_plan_reads_whole_first_under_a_grouping_a_distinct_or_a_sort() {
    let values = || Box::new(Plan::Values { rows: Vec::new(), detail: String::new() });
    let distinct = |streamed: bool| Plan::Distinct { input: values(), est: 0.0_f64, streamed };
    let project = |input: Plan| Plan::Project { exprs: Vec::new(), detail: String::new(), input: Box::new(input) };
    let filter = |input: Plan| {
        let cond = Cond::Const(crate::logical::Truth::True);
        Plan::Filter { cond, detail: String::new(), est: 0.0_f64, of_read: false, input: Box::new(input) }
    };
    let aggregate = |streamed: bool| {
        let (groups, collations, aggs, seeds) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        Plan::Aggregate { groups, collations, aggs, seeds, detail: String::new(), input: values(), est: 0.0_f64, streamed }
    };
    assert!(whole_first(&distinct(false), 0));
    assert!(!whole_first(&distinct(true), 0), "streamed, it gives rows as it reads them");
    assert!(whole_first(&aggregate(false), 0));
    assert!(!whole_first(&aggregate(true), 0), "streamed, it gives groups as it reads them");
    assert!(whole_first(&Plan::Sort { keys: Vec::new(), detail: String::new(), input: values(), most: None }, 0));
    assert!(whole_first(&project(filter(distinct(false))), 0), "under a projection and a filter");
    assert!(!whole_first(&project(*values()), 0), "a projection of rows as they come");
    assert!(!whole_first(&project(distinct(false)), MAX_DEPTH), "past the depth walked, none is found");
    // A run that stops at its first rows keeps such a plan as it is.
    let db = crate::db::Database::new();
    let state = db.snapshot().unwrap();
    assert!(matches!(parallel_run((&state, &Seen::default()), project(*values()), false), Plan::Project { .. }));
}

/// Verifies: [DESIGN] RULES.md D1166: an estimate past what a `u32`
/// holds is its own ceiling as a count, the most a count holds only
/// of one reaching it, never of any past 2^32.
#[test]
fn an_estimate_past_a_u32_is_counted_as_itself() {
    assert_eq!(rows_of(4_294_967_295.5_f64), 4_294_967_296);
    assert_eq!(rows_of(5.0e9_f64), 5_000_000_000);
    assert_eq!(rows_of(1.0e18_f64), 1_000_000_000_000_000_000);
    assert_eq!(rows_of(1.0e19_f64), usize::MAX >> 1_u32);
    assert_eq!(rows_of(f64::INFINITY), usize::MAX >> 1_u32);
    assert_eq!(rows_of(f64::NAN), 1);
}

/// An aggregation by `groups` of `input`, computing `aggs`, of
/// `seeds`, streamed or not.
fn grouping(groups: Vec<usize>, (aggs, seeds): (Vec<AggCall>, Vec<Vec<Value>>), input: Plan, streamed: bool) -> Plan {
    let (collations, detail, input, est) = (Vec::new(), String::new(), Box::new(input), 0.0_f64);
    Plan::Aggregate { groups, collations, aggs, seeds, detail, input, est, streamed }
}

/// COUNT of column 0, DISTINCT or not.
fn count(distinct: bool) -> AggCall {
    called("COUNT", distinct)
}

/// The set function `name` of column 0, DISTINCT or not.
fn called(name: &str, distinct: bool) -> AggCall {
    let func = crate::functions::Registry::with_set_functions().aggregate(name).unwrap();
    let result = func.0.result_type(None).unwrap_or(crate::value::DataType::Boolean);
    AggCall { func, arg: Some(Expr::Column(0)), distinct, result, filter: None, distribution: None, label: name.to_owned() }
}

/// Verifies: [DESIGN] RULES.md D1113: a grouping over another is a
/// pair a part holds whole if it groups by some of the first's
/// grouping columns and no others, an analyzed plan's count between
/// them or none; not of no grouping column, of a column past the
/// first's grouping columns, of a collation, of groups of empty
/// grouping sets of either, of a first streamed, of a DISTINCT set
/// function in the second, or of the first's states that do not merge.
#[test]
fn a_grouping_over_one_holding_its_columns_is_a_pair() {
    let values = || Plan::Values { rows: Vec::new(), detail: String::new() };
    let none = || (Vec::new(), Vec::new());
    let first = |seeds: Vec<Vec<Value>>, streamed: bool| grouping(vec![0, 1], (Vec::new(), seeds), values(), streamed);
    let pair = |groups: Vec<usize>, aggs: Vec<AggCall>, input: Plan| grouping(groups, (aggs, Vec::new()), input, false);
    assert!(first_level(&pair(vec![0], vec![count(false)], first(Vec::new(), false))).is_some());
    assert!(first_level(&pair(vec![1, 0], Vec::new(), first(Vec::new(), false))).is_some(), "of every column, in any order");
    let count_node = Arc::new(crate::physical::NodeCount::default());
    let counted = Plan::Counted(Box::new(crate::physical::Counted { count: count_node, input: first(Vec::new(), false) }));
    assert!(first_level(&pair(vec![0], Vec::new(), counted)).is_some(), "an analyzed plan's count between");
    assert!(first_level(&pair(Vec::new(), Vec::new(), first(Vec::new(), false))).is_none(), "of no grouping column");
    let max = || vec![called("MAX", false)];
    assert!(first_level(&pair(vec![2], max(), first(Vec::new(), false))).is_none(), "of a set function's column, of MAX");
    assert!(first_level(&pair(vec![0], Vec::new(), first(vec![vec![Value::Null, Value::Null]], false))).is_none(), "of seeds");
    assert!(first_level(&grouping(vec![0], (Vec::new(), vec![vec![Value::Null]]), first(Vec::new(), false), false)).is_none());
    assert!(first_level(&pair(vec![0], Vec::new(), first(Vec::new(), true))).is_none(), "of a first streamed");
    assert!(first_level(&grouping(vec![0], none(), first(Vec::new(), false), true)).is_none(), "of a second streamed");
    assert!(first_level(&pair(vec![0], vec![count(true)], first(Vec::new(), false))).is_none(), "of DISTINCT in the second");
    let unmerged = grouping(vec![0, 1], (vec![count(true)], Vec::new()), values(), false);
    assert!(first_level(&pair(vec![0], Vec::new(), unmerged)).is_none(), "of the first's states that do not merge");
    let mut collated = pair(vec![0], Vec::new(), first(Vec::new(), false));
    if let Plan::Aggregate { collations, .. } = &mut collated {
        collations.push(None);
    }
    assert!(first_level(&collated).is_none(), "of a collation");
    assert!(first_level(&first(Vec::new(), false)).is_none(), "of a grouping over no grouping");
}

/// Verifies: [DESIGN] RULES.md D1133: a grouping over another crosses
/// the first's partitions, as a pair a part holds, if it counts alone
/// and groups by a column past the first's grouping columns or over a
/// projection between them; not of another set function, nor of no
/// grouping column; one by the first's grouping columns does not cross.
#[test]
fn a_count_over_a_set_function_s_column_crosses_the_partitions() {
    let values = || Plan::Values { rows: Vec::new(), detail: String::new() };
    let first = || grouping(vec![0, 1], (vec![count(false)], Vec::new()), values(), false);
    let pair = |groups: Vec<usize>, aggs: Vec<AggCall>, input: Plan| grouping(groups, (aggs, Vec::new()), input, false);
    let projected =
        |input: Plan| Plan::Project { exprs: vec![Expr::Column(2), Expr::Column(0)], detail: String::new(), input: Box::new(input) };
    let crossing = pair(vec![2], vec![count(false)], first());
    assert!(crosses(&crossing) && first_level(&crossing).is_some(), "by the first's count");
    let over = pair(vec![0], vec![count(false)], projected(first()));
    assert!(crosses(&over) && first_level(&over).is_some(), "over a projection");
    assert!(!crosses(&pair(vec![2], vec![called("SUM", false)], first())), "of SUM");
    assert!(!crosses(&pair(Vec::new(), vec![count(false)], first())), "of no grouping column");
    assert!(!crosses(&pair(vec![0], vec![count(false)], first())), "by the first's grouping column");
}

/// Verifies: [DESIGN] RULES.md D405, D682: once a database starts, the
/// system's threads are kept, and EXECUTOR.WORKERS of 0 is planned by
/// them, however many times and from whichever thread it is asked.
#[test]
fn workers_of_none_set_are_the_threads_kept_as_a_database_starts() {
    let db = crate::db::Database::new();
    let kept = SYSTEM_THREADS.get().copied();
    assert!(kept.is_some_and(|n| n >= 1), "kept as it started");
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(0, 0))).unwrap();
    let state = db.snapshot().unwrap();
    let asked = std::thread::scope(|s| s.spawn(|| workers_of(&state)).join().unwrap());
    assert_eq!(Some(asked), kept);
    assert_eq!(Some(workers_of(&state)), kept);
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(3, 0))).unwrap();
    assert_eq!(workers_of(&db.snapshot().unwrap()), 3, "a setting is what it says");
}

/// Verifies: [DESIGN] RULES.md D1089: whether a grouping's exchange
/// outweighs its threads counts its columns by its table's statistics as
/// its statement first saw them. A statement that asked before they were
/// gathered counts none, though they are gathered before it asks again,
/// so on one thread the exchange does not outweigh; one asking after
/// counts them, and on one thread it does.
#[test]
fn a_grouping_s_exchange_counts_by_the_statistics_its_statement_saw() {
    let (db, st) = crate::physical::seen::ungathered(&[("T", 1_000, 1_000)]);
    let seen = Seen::default();
    assert!(seen.statistics(&st, &crate::physical::seen::named("T")).is_none());
    db.analyze("X.T").unwrap();
    let mut grouped = grouping(vec![0], (Vec::new(), Vec::new()), crate::physical::seen::scanned("T"), false);
    if let Plan::Aggregate { est, .. } = &mut grouped {
        *est = 1_000.0_f64;
    }
    assert!(!exchange::outweighs(&st, &seen, &grouped, 1_000, (100, 1)), "gathered since its statement asked");
    assert!(exchange::outweighs(&st, &Seen::default(), &grouped, 1_000, (100, 1)), "a statement asking afresh");
}
