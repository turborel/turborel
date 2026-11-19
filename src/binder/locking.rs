//! \[EXTENSION\] A query that locks the rows it reads, of a `<locking
//! clause>` (RULES.md D185): of one base table, read without grouping,
//! DISTINCT or windows, so each row it gives is one row of the table.
//! Each row is read with its id after the query's columns, sorted, and
//! the row limit applied after the lock, which the session places between
//! them (`db/unnamed/locking.rs`).

use super::order_by::ordered_query;
use super::query::Nesting;
use super::{Bound, Context, OutColumn};
use crate::ast::{CursorSpecification, QueryExpression, QuerySpecification, TableReference};
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError};
use crate::logical::{Expr, LogicalPlan};
use crate::storage::ROW_ID;
use crate::value::{DataType, MAX_EXACT_PRECISION, Spelled};

/// \[EXTENSION\] A locking query bound: its rows, each its result's
/// columns, then its sort's, then the row's id, and what it locks.
pub struct Locking {
    /// The rows, and the result's columns alone.
    pub bound: Bound,
    /// The table whose rows it locks.
    pub table: QualifiedName,
    /// Where each row's id is among its values.
    pub id: usize,
}

/// The refusal of a locking query of another shape.
fn unlockable(why: &str) -> SqlError {
    SqlError::new(
        Code::Unsupported,
        format!("[EXTENSION] FOR UPDATE and FOR SHARE lock the rows of a query of one base table; {why} (RULES.md D185)"),
    )
}

/// \[EXTENSION\] The cursor specification `c`, whose locking clause names
/// the tables `of` if any, bound to lock the rows it reads (RULES.md D185).
///
/// # Errors
///
/// SQLCODE -900 if it is of another shape; SQLCODE -103 if OF names a
/// table it does not read; as `bind_cursor`.
pub fn bind_locking(cx: Context<'_>, c: &CursorSpecification, of: Option<&[(String, usize)]>) -> Result<Locking> {
    let spec = one_specification(&c.query).ok_or_else(|| unlockable("this one is a set operation, a VALUES or has a WITH clause"))?;
    check_shape(spec, of)?;
    let mut ordered = ordered_query(cx, &c.query, (&[], &Nesting::Query), true)?;
    let (table, _) =
        locate(&mut ordered.plan).ok_or_else(|| unlockable("this one groups, aggregates, or reads a view or a typed table"))?;
    let visible = ordered.visible;
    ordered.all.push(OutColumn {
        name: None,
        collation: None,
        data_type: DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric },
        not_null: true,
        origin: None,
    });
    ordered.visible = ordered.all.len();
    let mut bound = ordered.sorted();
    let id = bound.columns.len().saturating_sub(1);
    bound.columns.truncate(visible);
    Ok(Locking { bound, table, id })
}

/// The one `<query specification>` of `q`, ordered or limited or not.
fn one_specification(q: &QueryExpression) -> Option<&QuerySpecification> {
    match q {
        QueryExpression::Spec(spec) => Some(spec),
        QueryExpression::Ordered(o) => one_specification(&o.body),
        QueryExpression::Values(_) | QueryExpression::SetOp(_) | QueryExpression::With(_) => None,
    }
}

/// That `spec` reads one table, named by `of` if it names any, without
/// DISTINCT, grouping or windows.
fn check_shape(spec: &QuerySpecification, of: Option<&[(String, usize)]>) -> Result<()> {
    let te = &spec.table_expression;
    if spec.distinct || te.group_by.is_some() || te.having.is_some() || !te.window.is_empty() {
        return Err(unlockable("this one has DISTINCT, GROUP BY, HAVING or a WINDOW clause"));
    }
    let [TableReference::Table(named)] = te.from.as_slice() else {
        return Err(unlockable("this one reads more than one, or a derived table"));
    };
    let name = named.correlation.as_deref().unwrap_or(&named.table.ident);
    if let Some((other, at)) = of.unwrap_or_default().iter().find(|(n, _)| n != name) {
        return Err(SqlError::at(
            Code::SyntaxRule,
            *at,
            format!("[EXTENSION] FOR UPDATE OF names {other}, which the query does not read as {name} (RULES.md D185)"),
        ));
    }
    Ok(())
}

/// The table `plan` scans, made to give each row's id after its columns,
/// and where that id is among the values of `plan`'s rows: of a scan, its
/// filters, and projections of them alone.
fn locate(plan: &mut LogicalPlan) -> Option<(QualifiedName, usize)> {
    if let LogicalPlan::Project { exprs, input } = plan {
        let (table, at) = locate(input)?;
        exprs.push(Expr::Column(at));
        return Some((table, exprs.len().saturating_sub(1)));
    }
    if let LogicalPlan::Filter { input, .. } = plan {
        return locate(input);
    }
    if let LogicalPlan::Scan { table, names, located: located @ false, .. } = plan {
        *located = true;
        names.push(ROW_ID.to_owned());
        return Some((table.clone(), names.len().saturating_sub(1)));
    }
    None
}
