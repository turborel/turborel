//! Unit tests of the pruning of the columns no one reads (RULES.md
//! D1167): each operator's, over plans built by hand of tables T (A, B,
//! C), U (D, E, F) and V (G, H).

use std::sync::Arc;

use super::super::super::{JoinKey, PhysicalPlanner, Plan, SemiJoin, SemiMethod};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::logical::{AggCall, Cond, Expr, OuterJoin, SortKey};
use crate::value::{DataType, Spelled, Value};

/// A database's state with the tables.
fn state() -> Arc<State> {
    let db = crate::db::Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (A INTEGER, B INTEGER, C INTEGER) \
         CREATE TABLE U (D INTEGER, E INTEGER, F INTEGER) CREATE TABLE V (G INTEGER, H INTEGER)",
    )
    .unwrap();
    db.snapshot().unwrap()
}

/// A scan of table `t` reading `columns`.
fn scan(t: &str, columns: &[usize]) -> Plan {
    let table = QualifiedName { auth: "S".into(), ident: t.into() };
    Plan::SeqScan {
        table,
        columns: columns.to_vec(),
        rows: 10,
        cost: 1.0_f64,
        order: Vec::new(),
        backward: false,
        most: None,
        located: false,
    }
}

/// The rows of `input` whose column `c` is not null.
fn filter(c: usize, input: Plan) -> Plan {
    let cond = Cond::IsNull(Expr::Column(c), true);
    Plan::Filter { cond, detail: format!("#{c} IS NOT NULL"), est: 1.0_f64, of_read: false, input: Box::new(input) }
}

/// The columns `columns` of `input`.
fn project(columns: &[usize], input: Plan) -> Plan {
    Plan::Project { exprs: columns.iter().map(|&c| Expr::Column(c)).collect(), detail: String::new(), input: Box::new(input) }
}

/// The keys of a join: pairs of a left and a right column.
fn keys(pairs: &[(usize, usize)]) -> Vec<JoinKey> {
    pairs.iter().map(|&(left, right)| JoinKey { left, right, nulls: false }).collect()
}

/// An inner hash join of `left` and `right` on `on`.
fn join(left: Plan, right: Plan, on: &[(usize, usize)]) -> Plan {
    Plan::HashJoin { left: Box::new(left), right: Box::new(right), keys: keys(on), detail: String::new(), est: 1.0_f64 }
}

/// `plan` pruned, as EXPLAIN shows it.
fn pruned(plan: Plan) -> String {
    let st = state();
    PhysicalPlanner::describing(&st).pruned(plan).explain()
}

/// The columns the first scan of table `t` in `plan` reads.
fn read(plan: &Plan, t: &str) -> Option<Vec<usize>> {
    if let Plan::SeqScan { table, columns, .. } = plan
        && table.ident == t
    {
        return Some(columns.clone());
    }
    let mut found = None;
    let mut plan = plan.clone();
    plan.each_input(&mut |input| found = found.take().or_else(|| read(input, t)));
    found
}

/// Verifies: [DESIGN] RULES.md D1167: a scan reads only the columns read
/// above it, and the plan's own columns are as they were.
#[test]
fn a_scan_reads_only_what_is_read_above_it() {
    let st = state();
    let planner = PhysicalPlanner::new(&st);
    let plan = planner.pruned(project(&[2, 0], scan("T", &[0, 1, 2])));
    assert_eq!(read(&plan, "T"), Some(vec![0, 2]));
    assert_eq!(planner.sized(&plan).width, Some(2));
}

/// Verifies: [DESIGN] RULES.md D1167: a column its filter alone reads is
/// given null to the join above it, on the built side and on the probing
/// side; a join's key is read of both.
#[test]
fn a_filters_own_column_is_given_null_to_the_join_above_it() {
    let plan = project(&[0, 3], join(filter(1, scan("T", &[0, 1, 2])), filter(2, scan("U", &[0, 1, 2])), &[(2, 1)]));
    let shown = pruned(plan);
    assert!(shown.contains("Project T.A, NULL, T.C"), "{shown}");
    assert!(shown.contains("Project U.D, U.E, NULL"), "{shown}");
}

/// Verifies: [DESIGN] RULES.md D1167: a join's keys read by nothing above
/// it are given null to the join above, which carries only the columns
/// read of it.
#[test]
fn a_joins_keys_are_given_null_after_it() {
    let inner = join(scan("T", &[0, 1, 2]), scan("U", &[0, 1, 2]), &[(2, 1)]);
    let shown = pruned(project(&[0, 6], join(inner, scan("V", &[0, 1]), &[(5, 1)])));
    assert!(shown.contains("Project T.A, NULL, NULL, NULL, NULL, U.F"), "{shown}");
}

/// Verifies: [DESIGN] RULES.md D1167: a sort carries only its keys and
/// the columns read above it.
#[test]
fn a_sort_carries_only_its_keys_and_what_is_read_above_it() {
    let sort = Plan::Sort {
        keys: vec![SortKey::ascending(2)],
        detail: String::new(),
        input: Box::new(filter(1, scan("T", &[0, 1, 2]))),
        most: None,
    };
    let shown = pruned(project(&[0], sort));
    assert!(shown.contains("Project T.A, NULL, T.C"), "{shown}");
}

/// Verifies: [DESIGN] RULES.md D1167: a grouping's input carries only its
/// groups' columns and its set functions'.
#[test]
fn a_groupings_input_carries_only_what_it_reads() {
    let st = state();
    let count = crate::functions::Registry::with_set_functions().aggregate("COUNT").unwrap();
    let agg = AggCall {
        func: count,
        arg: None,
        distinct: false,
        filter: None,
        distribution: None,
        result: DataType::Exact { precision: 18, scale: 0, spelled: Spelled::Numeric },
        label: "N".into(),
    };
    let input = Box::new(scan("T", &[0, 1, 2]));
    let grouping = Plan::Aggregate {
        groups: vec![1],
        collations: Vec::new(),
        aggs: vec![agg],
        seeds: Vec::new(),
        detail: String::new(),
        input,
        est: 1.0_f64,
        streamed: false,
        first: None,
    };
    assert_eq!(read(&PhysicalPlanner::new(&st).pruned(grouping), "T"), Some(vec![1]));
}

/// Verifies: [DESIGN] RULES.md D1167: an outer join's, a product's and a
/// semi join's operands carry only the columns read of them.
#[test]
fn every_joins_operands_carry_only_what_is_read_of_them() {
    let (left, right) = (filter(1, scan("T", &[0, 1, 2])), scan("U", &[0, 1, 2]));
    let outer = Plan::OuterJoin {
        kind: OuterJoin::Left,
        left: Box::new(left.clone()),
        right: Box::new(filter(2, right.clone())),
        keys: keys(&[(2, 1)]),
        cond: None,
        own: None,
        widths: (3, 3),
        detail: String::new(),
        est: 1.0_f64,
    };
    let outer_shown = pruned(project(&[0, 3], outer));
    assert!(outer_shown.contains("Project T.A, NULL, T.C") && outer_shown.contains("Project U.D, U.E, NULL"), "{outer_shown}");
    let product = Plan::Product { left: Box::new(left.clone()), right: Box::new(right.clone()), est: 1.0_f64 };
    let product_shown = pruned(project(&[0, 3], product));
    assert!(product_shown.contains("Project T.A, NULL, NULL"), "{product_shown}");
    let semi = SemiJoin {
        anti: false,
        method: SemiMethod::Hash,
        left,
        right: filter(2, right),
        keys: keys(&[(2, 1)]),
        cond: None,
        widths: (3, 3),
        detail: String::new(),
        est: 1.0_f64,
    };
    let semi_shown = pruned(project(&[0], Plan::SemiJoin(Box::new(semi))));
    assert!(semi_shown.contains("Project T.A, NULL, T.C") && semi_shown.contains("Project NULL, U.E, NULL"), "{semi_shown}");
}

/// Verifies: [DESIGN] RULES.md D1167: an expression not read above still
/// reads its columns, so no error it raises is lost, and a column already
/// read above is not nulled; nothing is pruned that is read.
#[test]
fn a_computed_column_unread_above_still_reads_its_columns() {
    let st = state();
    let exprs = vec![Expr::Column(0), Expr::Neg(Box::new(Expr::Column(1))), Expr::Const(Value::Null)];
    let inner = Plan::Project { exprs, detail: String::new(), input: Box::new(scan("T", &[0, 1, 2])) };
    let plan = PhysicalPlanner::new(&st).pruned(project(&[0], inner));
    assert_eq!(read(&plan, "T"), Some(vec![0, 1]));
    let whole = PhysicalPlanner::describing(&st).pruned(join(scan("T", &[0, 1, 2]), scan("U", &[0, 1, 2]), &[(0, 0)]));
    assert!(!whole.explain().contains("NULL"), "{}", whole.explain());
}
