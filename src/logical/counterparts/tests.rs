//! The counterparts of a plan's columns, node by node.

use super::super::{Expr, LogicalPlan, SetOp};
use super::BaseColumn;
use crate::catalog::QualifiedName;
use crate::value::Value;

/// Table `ident` of schema `S`.
fn table(ident: &str) -> QualifiedName {
    QualifiedName { auth: "S".to_owned(), ident: ident.to_owned() }
}

/// A scan of table `ident` of `width` columns, its row id after them if
/// `located`.
fn scan(ident: &str, width: usize, located: bool) -> LogicalPlan {
    let names = (0..width.saturating_add(usize::from(located))).map(|k| format!("C{k}")).collect();
    LogicalPlan::Scan { table: table(ident), names, needed: Vec::new(), located }
}

/// Column `k` of table `ident`.
fn of(ident: &str, k: usize) -> BaseColumn {
    (table(ident), k)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.2] each column of a
/// base table read is its own counterpart; the row id a MERGE's target
/// reads has none.
#[test]
fn a_scan_is_its_own_table_but_for_its_row_id() {
    assert_eq!(scan("T", 2, false).counterparts(), vec![Some(of("T", 0)), Some(of("T", 1))]);
    assert_eq!(scan("T", 2, true).counterparts(), vec![Some(of("T", 0)), Some(of("T", 1)), None]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.2] a projection's
/// column is the counterpart of the column a column reference of it
/// references; any other value has none.
#[test]
fn a_projection_keeps_only_its_column_references() {
    let exprs = vec![Expr::Column(1), Expr::Const(Value::Null), Expr::Column(0)];
    let plan = LogicalPlan::Project { exprs, input: Box::new(scan("T", 2, false)) };
    assert_eq!(plan.counterparts(), vec![Some(of("T", 1)), None, Some(of("T", 0))]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.2] a product's
/// columns are its operands', the first's first.
#[test]
fn a_product_is_both_operands_in_order() {
    let plan = LogicalPlan::Product { left: Box::new(scan("A", 1, false)), right: Box::new(scan("B", 2, false)) };
    assert_eq!(plan.counterparts(), vec![Some(of("A", 0)), Some(of("B", 0)), Some(of("B", 1))]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.2] a grouping column
/// is the counterpart of the column it groups by.
#[test]
fn a_grouping_column_is_its_column_and_a_set_function_none() {
    let plan = LogicalPlan::Aggregate {
        groups: vec![1],
        collations: vec![None],
        aggs: Vec::new(),
        seeds: Vec::new(),
        input: Box::new(scan("T", 2, false)),
    };
    assert_eq!(plan.counterparts(), vec![Some(of("T", 1))]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.2] a set operation's
/// column, specified by no column reference, has no counterpart.
#[test]
fn a_set_operation_has_none() {
    let plan = LogicalPlan::SetOp {
        op: SetOp::Union,
        all: true,
        compared: None,
        left: Box::new(scan("A", 1, false)),
        right: Box::new(scan("A", 1, false)),
    };
    assert_eq!(plan.counterparts(), vec![None]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.2] counterparts are
/// transitive, through a view bound into the plan that reads it.
#[test]
fn a_view_bound_into_a_plan_passes_its_columns_through() {
    let view = LogicalPlan::Project { exprs: vec![Expr::Column(1)], input: Box::new(scan("T", 2, false)) };
    let plan = LogicalPlan::Distinct { input: Box::new(LogicalPlan::Project { exprs: vec![Expr::Column(0)], input: Box::new(view) }) };
    assert_eq!(plan.counterparts(), vec![Some(of("T", 1))]);
}
