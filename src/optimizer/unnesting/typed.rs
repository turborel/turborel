//! \[DESIGN\] Whether a binding's values group and hash as they compare
//! (RULES.md D319). Unnesting groups, makes distinct and hash joins the
//! outer rows' bindings, which compare them by their forms. A large object
//! is grouped by its identity, not its value [ISO/IEC 9075-2:2023 §9.12
//! SR3], and a value of a constructed or user-defined type may hold one,
//! so a binding is taken only if each value of it is of a predefined type
//! other than a large object, as the plan's tables declare it.

use crate::catalog::Catalog;
use crate::logical::{Expr, LogicalPlan, expr_columns};
use crate::value::DataType;

/// Whether `t` groups, hashes and compares alike.
fn plain(t: &DataType) -> bool {
    matches!(
        t,
        DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Exact { .. }
            | DataType::Approximate { .. }
            | DataType::DecFloat { .. }
            | DataType::Binary { .. }
            | DataType::VarBinary { .. }
            | DataType::Boolean
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Interval(_)
    )
}

/// Whether `e`, over the rows of `plan`, is known to be of a plain type:
/// of columns known to be, by arithmetic, casts to plain types and sort
/// keys alone.
pub(super) fn plain_key(e: &Expr, plan: &LogicalPlan, catalog: &dyn Catalog) -> bool {
    if !plain_form(e) {
        return false;
    }
    let mut columns = Vec::new();
    expr_columns(e, &mut columns);
    columns.iter().all(|&c| plain_column(plan, c, catalog))
}

/// Whether `e` is made of the forms `plain_key` takes.
fn plain_form(e: &Expr) -> bool {
    let own = match e {
        Expr::Column(_) | Expr::Neg(_) | Expr::Arith(..) | Expr::SortKey(..) => true,
        Expr::Const(v) => !matches!(v, crate::value::Value::Lob(_)),
        Expr::Cast(_, t) => plain(t),
        // A function whose type its arguments' do not decide, as storing
        // a value in a parameter's type.
        Expr::Call(f, _) => f.0.return_type(&[]).is_ok_and(|t| plain(&t)),
        Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Domain(..)
        | Expr::Subquery(..)
        | Expr::Case(..)
        | Expr::Truth(_)
        | Expr::Warned(..)
        | Expr::NextValue(_)
        | Expr::Invoked(_) => false,
    };
    let mut children = true;
    e.for_each_child(&mut |child| children = children && plain_form(child));
    own && children
}

/// Whether column `column` of `plan` is known to be of a plain type.
fn plain_column(plan: &LogicalPlan, column: usize, catalog: &dyn Catalog) -> bool {
    match plan {
        LogicalPlan::Scan { table, .. } => catalog.table(table).and_then(|t| t.columns.get(column)).is_some_and(|c| plain(&c.data_type)),
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Distinct { input }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Limit { input, .. } => plain_column(input, column, catalog),
        LogicalPlan::Product { left, right } | LogicalPlan::OuterJoin { left, right, .. } => match column.checked_sub(left.width()) {
            Some(of_right) => plain_column(right, of_right, catalog),
            None => plain_column(left, column, catalog),
        },
        LogicalPlan::Project { exprs, input } => exprs.get(column).is_some_and(|e| projected(e, input, catalog)),
        LogicalPlan::Aggregate { groups, aggs, input, .. } => match column.checked_sub(groups.len()) {
            Some(n) => aggs.get(n).is_some_and(|a| plain(&a.result)),
            None => groups.get(column).is_some_and(|&g| plain_column(input, g, catalog)),
        },
        LogicalPlan::Values { types, .. } => types.get(column).is_some_and(plain),
        LogicalPlan::SetOp { left, .. } => plain_column(left, column, catalog),
        LogicalPlan::System { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(_)
        | LogicalPlan::Window(_)
        | LogicalPlan::Recognize(_)
        | LogicalPlan::Polymorphic(_) => false,
    }
}

/// Whether the projected expression `e` over `input` is of a plain type:
/// a column of one, or a constant of a value that is not a large object.
fn projected(e: &Expr, input: &LogicalPlan, catalog: &dyn Catalog) -> bool {
    match e {
        Expr::Column(c) => plain_column(input, *c, catalog),
        Expr::Const(v) => !matches!(v, crate::value::Value::Lob(_)),
        Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::SortKey(..)
        | Expr::Subquery(..)
        | Expr::Case(..)
        | Expr::Truth(_)
        | Expr::Warned(..)
        | Expr::NextValue(_)
        | Expr::Invoked(_) => false,
    }
}
