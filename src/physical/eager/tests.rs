//! Unit tests of eager aggregation's plans (`physical/eager.rs`).

use super::chain::Chain;
use super::classify::{Cx, Role};
use super::rewrite::{Grouping, rewritten};
use crate::catalog::MemoryCatalog;
use crate::db::State;
use crate::logical::{AggCall, Cond, Expr, LogicalPlan};
use crate::value::{CompOp, DataType, Decimal, Spelled, Value};

/// INTEGER-like exact numbers.
const WHOLE: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric };

/// An operand of one row of `width` columns.
fn operand(width: usize) -> LogicalPlan {
    LogicalPlan::Values { rows: vec![vec![Expr::Const(Value::Exact(Decimal::new(1, 0))); width]], types: vec![WHOLE; width] }
}

/// The filter `l = r` over `input`.
fn joined(l: usize, r: usize, input: LogicalPlan) -> LogicalPlan {
    LogicalPlan::Filter { cond: Cond::Compare(Expr::Column(l), CompOp::Eq, Expr::Column(r)), input: Box::new(input) }
}

fn product(left: LogicalPlan, right: LogicalPlan) -> LogicalPlan {
    LogicalPlan::Product { left: Box::new(left), right: Box::new(right) }
}

/// Verifies: [DESIGN] RULES.md D660: a chain's relations come in the
/// order of their columns, at their first columns, and its filters'
/// conjuncts over its columns; a relation alone, or a guard, is no chain.
#[test]
fn chains_are_taken_apart() {
    let plan = joined(1, 4, product(joined(0, 2, product(operand(2), operand(2))), operand(3)));
    let chain = Chain::of(&plan).unwrap();
    let starts: Vec<(usize, usize)> = chain.leaves.iter().map(|l| (l.start, l.width)).collect();
    assert_eq!(starts, [(0, 2), (2, 2), (4, 3)]);
    assert_eq!(chain.conds.len(), 2);
    assert!(Chain::of(&operand(2)).is_none());
    let guard = joined(0, 0, product(operand(1), operand(0)));
    assert!(Chain::of(&guard).is_none());
}

/// Verifies: [DESIGN] RULES.md D660-D662: of X grouped first by its join
/// column, the early grouping gives the key, the pushed part and the
/// count, in that order; the other relation's columns follow them; the
/// pushed function merges its part and the scaled one pairs its value
/// with the count; the grouping is by the other's column moved.
#[test]
fn the_plan_grouped_first_is_laid_out() {
    let st = State::with_builtins(Box::new(MemoryCatalog::default()));
    let call = |name: &str, arg: Option<usize>| AggCall {
        func: st.functions.aggregate(name).unwrap(),
        arg: arg.map(Expr::Column),
        distinct: false,
        filter: None,
        distribution: None,
        result: WHOLE,
        label: format!("{name}({arg:?})"),
    };
    let aggs = [call("COUNT", None), call("SUM", Some(3))];
    let chain = Chain::of(&joined(0, 2, product(operand(2), operand(2)))).unwrap();
    let (groups, catalog) = ([3_usize], MemoryCatalog::default());
    let cx = Cx { chain: &chain, x: 0, groups: &groups, aggs: &aggs, types: &|_| Some(WHOLE), catalog: &catalog };
    let shape = cx.shape().unwrap();
    assert_eq!(shape.roles, [Role::Pushed, Role::Scaled(crate::functions::ColumnarKind::Sum)]);
    let grouping = Grouping { groups: &groups, collations: &[], aggs: &aggs };
    let count = st.functions.aggregate("COUNT").unwrap();
    let LogicalPlan::Aggregate { groups: moved, aggs: finals, input, .. } = rewritten(&grouping, (&chain, 0), &shape, &count).unwrap()
    else {
        panic!("no grouping")
    };
    assert_eq!(moved, [4]);
    let first = finals.first().map(|a| (a.func.0.name().to_owned(), format!("{:?}", a.arg)));
    assert_eq!(first, Some(("MERGED".into(), "Some(Column(1))".into())));
    assert_eq!(finals.get(1).map(|a| a.func.0.name().to_owned()), Some("SCALED".into()));
    let early = Chain::of(&input).unwrap();
    let LogicalPlan::Aggregate { groups: keys, aggs: parts, .. } = &early.leaves[0].plan else { panic!("not grouped first") };
    assert_eq!((keys.clone(), parts.len(), early.leaves[1].start), (vec![0], 2, 3));
}

/// Verifies: [DESIGN] RULES.md D660: a chain made again puts each
/// conjunct over the lowest product that gives its columns, so a chain
/// planned as written joins each relation by its own conjuncts, never by
/// a product filtered above the last relation.
#[test]
fn chains_made_again_place_each_conjunct_lowest() {
    let plan = joined(1, 4, product(joined(0, 2, product(operand(2), operand(2))), operand(3)));
    let chain = Chain::of(&plan).unwrap();
    let leaves = chain.leaves.iter().map(|l| l.plan.clone()).collect();
    assert_eq!(format!("{:?}", Chain::made(leaves, chain.conds).unwrap()), format!("{plan:?}"));
}
