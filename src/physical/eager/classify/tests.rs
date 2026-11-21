//! Unit tests of the roles of set functions (`eager/classify.rs`).

use super::super::chain::Chain;
use super::*;
use crate::catalog::MemoryCatalog;
use crate::db::State;
use crate::value::{ArithOp, CompOp, Decimal, Spelled};

/// INTEGER-like exact numbers.
const WHOLE: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric };
/// DOUBLE PRECISION.
const DOUBLE: DataType = DataType::Approximate { precision: 53, spelled: Spelled::Float };

/// An operand of one row of `width` columns.
fn operand(width: usize) -> LogicalPlan {
    LogicalPlan::Values { rows: vec![vec![Expr::Const(Value::Exact(Decimal::new(1, 0))); width]], types: vec![WHOLE; width] }
}

/// The chain of two relations of two columns each, X's 0 and 1, the
/// other's 2 and 3, joined by 0 = 2.
fn chain() -> Chain {
    let product = LogicalPlan::Product { left: Box::new(operand(2)), right: Box::new(operand(2)) };
    let cond = Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Column(2));
    Chain::of(&LogicalPlan::Filter { cond, input: Box::new(product) }).unwrap()
}

/// The call of `name` of `arg`, of result `result`.
fn call(st: &State, name: &str, arg: Option<usize>, result: DataType) -> AggCall {
    AggCall {
        func: st.functions.aggregate(name).unwrap(),
        arg: arg.map(Expr::Column),
        distinct: false,
        filter: None,
        distribution: None,
        result,
        label: format!("{name}({arg:?})"),
    }
}

/// The shape of grouping X first of `aggs` grouped by `groups`, the
/// columns of the types `types` gives.
fn shape_of(aggs: &[AggCall], groups: &[usize], types: Types<'_>) -> Option<Shape> {
    let (chain, catalog) = (chain(), MemoryCatalog::default());
    Cx { chain: &chain, x: 0, groups, aggs, types, catalog: &catalog }.shape()
}

fn state() -> State {
    State::with_builtins(Box::new(MemoryCatalog::default()))
}

/// Verifies: [DESIGN] RULES.md D660: COUNT(*) is pushed; a sum of X's
/// column that may be null is summed above, scaled, its column passed up;
/// MAX of the other's column is plain; a sum of the other's is scaled; X
/// is grouped by its join column, and its grouping and passed columns.
#[test]
fn functions_are_pushed_plain_or_scaled() {
    let st = state();
    let aggs = [
        call(&st, "COUNT", None, WHOLE),
        call(&st, "SUM", Some(1), WHOLE),
        call(&st, "MAX", Some(3), WHOLE),
        call(&st, "SUM", Some(3), WHOLE),
    ];
    let shape = shape_of(&aggs, &[3], &|_| Some(WHOLE)).unwrap();
    let roles = [Role::Pushed, Role::Scaled(ColumnarKind::Sum), Role::Plain, Role::Scaled(ColumnarKind::Sum)];
    assert_eq!(shape.roles, roles);
    assert_eq!(shape.keys, [0, 1]);
    assert_eq!((shape.own.len(), shape.rest.len()), (0, 1));
}

/// Verifies: [DESIGN] RULES.md D660: nothing is grouped first of a sum
/// of approximate numbers, of a function reading both relations, or of
/// keys of a type whose equal values may differ; nor of no key.
///
/// Decision `alike && !keys.is_empty()`. MC/DC: (T T) shaped, (F T) keys
/// of approximate numbers, (T F) no key (a product).
#[test]
fn shapes_refused() {
    let st = state();
    let float = [call(&st, "SUM", Some(3), DOUBLE)];
    assert!(shape_of(&float, &[], &|_| Some(WHOLE)).is_none());
    let mut both = call(&st, "SUM", Some(1), WHOLE);
    both.arg = Some(Expr::Arith(Box::new(Expr::Column(1)), ArithOp::Add, Box::new(Expr::Column(2))));
    assert!(shape_of(&[both], &[], &|_| Some(WHOLE)).is_none());
    let count = [call(&st, "COUNT", None, WHOLE)];
    assert!(shape_of(&count, &[], &|_| Some(WHOLE)).is_some());
    assert!(shape_of(&count, &[], &|_| Some(DOUBLE)).is_none());
    let product = LogicalPlan::Product { left: Box::new(operand(2)), right: Box::new(operand(2)) };
    let (unjoined, catalog) = (Chain::of(&product).unwrap(), MemoryCatalog::default());
    let cx = Cx { chain: &unjoined, x: 0, groups: &[], aggs: &count, types: &|_| Some(WHOLE), catalog: &catalog };
    assert!(cx.shape().is_none());
}

/// Verifies: [DESIGN] RULES.md D660: a function is pushed only of an
/// argument never null that cannot raise; a constant is never null, so
/// SUM of one is pushed; DISTINCT SUM is plain; MIN with DISTINCT pushed.
///
/// Decision `(a.distinct && !blind) || (summed && !exact) || !state`.
/// MC/DC: SUM(DISTINCT) true by the first alone; SUM of approximate
/// numbers by the second (refused whole); SUM of a constant false.
#[test]
fn pushed_takes_what_cannot_raise_or_warn() {
    let st = state();
    let mut constant = call(&st, "SUM", None, WHOLE);
    constant.arg = Some(Expr::Const(Value::Exact(Decimal::new(2, 0))));
    let mut distinct = call(&st, "SUM", Some(1), WHOLE);
    distinct.distinct = true;
    let mut least = call(&st, "MIN", None, WHOLE);
    least.arg = Some(Expr::Const(Value::Exact(Decimal::new(3, 0))));
    least.distinct = true;
    let shape = shape_of(&[constant, distinct, least], &[], &|_| Some(WHOLE)).unwrap();
    assert_eq!(shape.roles, [Role::Pushed, Role::Plain, Role::Pushed]);
    assert_eq!(shape.keys, [0, 1]);
}
