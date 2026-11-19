//! \[EXTENSION\] A query that locks the rows it reads, of a `<locking
//! clause>` (RULES.md D185), of base tables, or views of one, joined or
//! not, read without grouping, DISTINCT or windows, so each row it gives
//! is made of one row of each table. OF names the tables whose rows it
//! locks, by the names the FROM clause exposes; without OF, it locks the
//! rows of each. Each row is read with the ids of its rows after the
//! query's columns, sorted, and the row limit applied after the locks,
//! which the session places between them (`db/unnamed/locking.rs`).

use super::order_by::ordered_query;
use super::query::Nesting;
use super::{Bound, Context, OutColumn};
use crate::ast::{CursorSpecification, JoinKind, QueryExpression, QuerySpecification, TableReference};
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError};
use crate::logical::{Expr, LogicalPlan, map_cond_columns};
use crate::storage::ROW_ID;
use crate::value::{DataType, MAX_EXACT_PRECISION, Spelled};

/// \[EXTENSION\] A locking query bound: its rows, each its result's
/// columns, then its sort's, then the ids of the rows it is made of, and
/// what it locks.
pub struct Locking {
    /// The rows, and the result's columns alone.
    pub bound: Bound,
    /// Each table whose rows it locks, and where the id of its row is
    /// among the values of each row.
    pub tables: Vec<(QualifiedName, usize)>,
}

/// The refusal of a locking query of another shape.
fn unlockable(why: &str) -> SqlError {
    SqlError::new(
        Code::Unsupported,
        format!("[EXTENSION] FOR UPDATE and FOR SHARE lock the rows of a query of base tables; {why} (RULES.md D185)"),
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
    let chosen = chosen_tables(spec, of)?;
    let mut ordered = ordered_query(cx, &c.query, (&[], &Nesting::Query), true)?;
    let mut found = Found { chosen, next: 0, tables: Vec::new() };
    let located = found.locate(&mut ordered.plan);
    if located.is_none() || found.next != found.chosen.len() {
        return Err(unlockable("this one groups, aggregates, or reads a view of several tables, a typed table or a derived table"));
    }
    let visible = ordered.visible;
    for _ in &found.tables {
        ordered.all.push(OutColumn {
            name: None,
            collation: None,
            data_type: DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric },
            not_null: false,
            origin: None,
        });
    }
    ordered.visible = ordered.all.len();
    let mut bound = ordered.sorted();
    let first = bound.columns.len().saturating_sub(found.tables.len());
    bound.columns.truncate(visible);
    let tables = found.tables.into_iter().enumerate().map(|(i, table)| (table, first.saturating_add(i))).collect();
    Ok(Locking { bound, tables })
}

/// The one `<query specification>` of `q`, ordered or limited or not.
fn one_specification(q: &QueryExpression) -> Option<&QuerySpecification> {
    match q {
        QueryExpression::Spec(spec) => Some(spec),
        QueryExpression::Ordered(o) => one_specification(&o.body),
        QueryExpression::Values(_) | QueryExpression::SetOp(_) | QueryExpression::With(_) => None,
    }
}

/// For each table `spec` reads, in the order its FROM clause names them,
/// whether its rows are locked, each named by `of`, or every one if `of`
/// names none. `spec` is without DISTINCT, grouping or windows.
///
/// # Errors
///
/// SQLCODE -900 of another shape; SQLCODE -103 if OF names a table the
/// query does not read.
fn chosen_tables(spec: &QuerySpecification, of: Option<&[(String, usize)]>) -> Result<Vec<bool>> {
    let te = &spec.table_expression;
    if spec.distinct || te.group_by.is_some() || te.having.is_some() || !te.window.is_empty() {
        return Err(unlockable("this one has DISTINCT, GROUP BY, HAVING or a WINDOW clause"));
    }
    let mut names = Vec::new();
    for item in &te.from {
        exposed(item, &mut names)?;
    }
    let of = of.unwrap_or_default();
    if let Some((other, at)) = of.iter().find(|(n, _)| !names.contains(n)) {
        return Err(SqlError::at(
            Code::SyntaxRule,
            *at,
            format!("[EXTENSION] FOR UPDATE OF names {other}, which the query does not read (RULES.md D185)"),
        ));
    }
    Ok(names.iter().map(|name| of.is_empty() || of.iter().any(|(n, _)| n == name)).collect())
}

/// Adds the names `item` exposes, a table's or a join's operands', to
/// `names`, in order.
fn exposed(item: &TableReference, names: &mut Vec<String>) -> Result<()> {
    if let TableReference::Table(named) = item {
        names.push(named.correlation.clone().unwrap_or_else(|| named.table.ident.clone()));
        return Ok(());
    }
    let TableReference::Joined(joined) = item else { return Err(unlockable("this one reads a derived table or a table function")) };
    if joined.kind == JoinKind::Union {
        return Err(unlockable("this one has a union join"));
    }
    exposed(&joined.left, names)?;
    exposed(&joined.right, names)
}

/// How a locking query's plan is made to give the ids of its rows, for
/// each table it reads, whether its rows are locked, the next to be met,
/// and the tables met whose rows are.
struct Found {
    chosen: Vec<bool>,
    next: usize,
    tables: Vec<QualifiedName>,
}

impl Found {
    /// Makes `plan` give, after its columns, the id of the row of each
    /// table whose rows are locked that it reads, in the order met, left to
    /// right, of scans, filters, projections, products and outer joins of
    /// them alone. Its new width, or None of another node.
    fn locate(&mut self, plan: &mut LogicalPlan) -> Option<usize> {
        if let LogicalPlan::Scan { table, names, located, .. } = plan {
            let chosen = *self.chosen.get(self.next)?;
            self.next = self.next.saturating_add(1);
            if chosen && !*located {
                *located = true;
                names.push(ROW_ID.to_owned());
                self.tables.push(table.clone());
            }
            return Some(names.len());
        }
        if let LogicalPlan::Filter { input, .. } = plan {
            return self.locate(input);
        }
        if let LogicalPlan::Project { exprs, input } = plan {
            let before = self.tables.len();
            let width = self.locate(input)?;
            let ids = width.saturating_sub(self.tables.len().saturating_sub(before));
            exprs.extend((ids..width).map(Expr::Column));
            return Some(exprs.len());
        }
        self.joined(plan)
    }

    /// As `locate`, of a product or an outer join, the ids its operands
    /// give put after the columns of both.
    fn joined(&mut self, plan: &mut LogicalPlan) -> Option<usize> {
        let placeholder = LogicalPlan::Values { rows: Vec::new(), types: Vec::new() };
        let (mut left, mut right, rebuild) = split(std::mem::replace(plan, placeholder))?;
        let (left_width, right_width) = (left.width(), right.width());
        let new_left = self.locate(&mut left)?;
        let new_right = self.locate(&mut right)?;
        let left_ids = new_left.saturating_sub(left_width);
        let right_end = new_left.saturating_add(new_right);
        let right_columns = new_left..new_left.saturating_add(right_width);
        let shift = |c: usize| if c >= left_width { c.saturating_add(left_ids) } else { c };
        let order = (0..left_width).chain(right_columns).chain(left_width..new_left).chain(new_left.saturating_add(right_width)..right_end);
        let exprs: Vec<Expr> = order.map(Expr::Column).collect();
        let width = exprs.len();
        *plan = LogicalPlan::Project { exprs, input: Box::new(rebuild(left, right, &shift)) };
        Some(width)
    }
}

/// How a product or an outer join is made again of its operands, its
/// condition's columns renumbered by the function given.
type Rebuild = Box<dyn FnOnce(Box<LogicalPlan>, Box<LogicalPlan>, &dyn Fn(usize) -> usize) -> LogicalPlan>;

/// The operands of `plan`, if a product or an outer join, and how to make
/// it again of them.
fn split(plan: LogicalPlan) -> Option<(Box<LogicalPlan>, Box<LogicalPlan>, Rebuild)> {
    if let LogicalPlan::Product { left, right } = plan {
        return Some((left, right, Box::new(|l, r, _| LogicalPlan::Product { left: l, right: r })));
    }
    let LogicalPlan::OuterJoin { kind, cond, left, right } = plan else { return None };
    let rebuild: Rebuild =
        Box::new(move |l, r, shift| LogicalPlan::OuterJoin { kind, cond: map_cond_columns(cond, &shift), left: l, right: r });
    Some((left, right, rebuild))
}
