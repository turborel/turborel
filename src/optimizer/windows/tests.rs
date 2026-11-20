//! Unit tests: which conjuncts go below a window, and when none do
//! (RULES.md R-TEST-1, R-TEST-8).

use super::*;
use crate::catalog::MemoryCatalog;
use crate::db::State;
use crate::logical::Expr;
use crate::value::{ArithOp, CompOp, DataType, Decimal, Spelled, Value};

fn number(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

/// The filter of `cond` over a window partitioned by column 0 of two
/// INTEGER columns, rewritten.
fn applied(cond: Cond) -> Option<LogicalPlan> {
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    let rows = LogicalPlan::Values { rows: Vec::new(), types: vec![integer.clone(), integer] };
    let window = Window { partition: vec![0], order: Vec::new(), calls: Vec::new(), input: rows };
    let plan = LogicalPlan::Filter { cond, input: Box::new(LogicalPlan::Window(Box::new(window))) };
    let st = State::with_builtins(Box::new(MemoryCatalog::default()));
    PushFilterThroughWindow.apply(&plan, &RuleContext { db: &st })
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.15 GR3]; [DESIGN] RULES.md D374,
/// D15: a conjunct over the partitioning column goes below the window,
/// one over another column stays above; none goes below beside one left
/// above that can raise, which would then be evaluated of fewer rows.
///
/// Decision `down.is_empty() || keep can raise`. MC/DC: (F F) moved;
/// (T F) left; (F T) left.
#[test]
fn conjuncts_of_the_partition_go_below() {
    let partitioned = Cond::Compare(Expr::Column(0), CompOp::Eq, number(7));
    let other = Cond::Compare(Expr::Column(1), CompOp::Eq, number(3));
    let raising = Cond::Compare(Expr::Arith(Box::new(number(10)), ArithOp::Div, Box::new(Expr::Column(1))), CompOp::Gt, number(1));
    let both = |keep: &Cond| Cond::And(Box::new(partitioned.clone()), Box::new(keep.clone()));
    let Some(LogicalPlan::Filter { input, .. }) = applied(both(&other)) else { panic!("F F") };
    let LogicalPlan::Window(window) = *input else { panic!("a window") };
    assert!(matches!(window.input, LogicalPlan::Filter { .. }));
    assert!(applied(other).is_none(), "T F");
    assert!(applied(both(&raising)).is_none(), "F T");
}
