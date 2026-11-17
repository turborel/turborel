//! [ISO/IEC 9075-2:2023 §4.17.9, §7.6 GR3b] A typed table read with its
//! proper subtables. Each table of a hierarchy keeps its own rows, and a
//! row of a proper subtable has its superrow in the table: its values of
//! the table's columns, which are the subtable's first columns, an
//! inherited column at the place of its supercolumn [§11.3 SR11d vi, SR11i].
//! The table named without ONLY is read as its own rows and, after them,
//! each proper subtable's rows cut to those columns; with ONLY, as its own
//! rows alone [GR3b i] (RULES.md D81).

use super::FromItem;
use super::views::Source;
use crate::catalog::{QualifiedName, TableDesc, Who, applicable, proper_subtables, proper_supertables};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::logical::{Expr, LogicalPlan, SetOp};

/// [ISO/IEC 9075-2:2023 §7.6 AR1b ii 2, §14.9 AR1b ii, §14.14 AR1b ii] That
/// `who` may name the table `desc` with ONLY, as `clause` requires: of a
/// typed table, its privileges include SELECT WITH HIERARCHY OPTION on
/// the table or a proper supertable. \[DESIGN\] Of a table not typed,
/// which has no subtable and on which that option is never held, ONLY is
/// its name alone, of no further privilege (RULES.md D81).
///
/// # Errors
///
/// SQLCODE -104 if they do not.
pub fn only_allowed(db: &State, who: Who<'_>, desc: &TableDesc, clause: &str) -> Result<()> {
    if desc.typed.is_none() {
        return Ok(());
    }
    let mut tables = std::iter::once(desc.name.clone()).chain(proper_supertables(&*db.catalog, &desc.name));
    if tables.any(|t| applicable(&*db.catalog, who, &t).0.typed.hierarchy) {
        return Ok(());
    }
    Err(SqlError::new(
        Code::Privilege,
        format!(
            "[ISO/IEC 9075-2:2023 {clause}] {} holds SELECT WITH HIERARCHY OPTION on no supertable of {}, so it names it with ONLY",
            who.auth, desc.name
        ),
    ))
}

/// Of the table `name` of `desc`, whose rows come from `source`, named
/// with ONLY if `only`: its proper subtables, if it is a typed base table
/// read without ONLY, else none.
pub(super) fn family_of(db: &State, (name, desc, source): (&QualifiedName, &TableDesc, &Source), only: bool) -> Vec<QualifiedName> {
    if only || desc.typed.is_none() || !matches!(source, Source::Table) {
        return Vec::new();
    }
    proper_subtables(&*db.catalog, name)
}

/// The rows of `it`, a typed table with proper subtables, its columns
/// named `names`, the query using those at `needed`: its own, then each
/// subtable's superrows.
pub(super) fn family_plan(db: &State, it: &FromItem, names: &[String], needed: &[usize]) -> LogicalPlan {
    let width = names.len();
    let own = LogicalPlan::Scan { table: it.table.clone(), names: names.to_vec(), needed: needed.to_vec(), located: false };
    it.family.iter().fold(own, |plan, sub| {
        let columns = db.catalog.table(sub).map_or_else(|| names.to_vec(), |d| d.columns.iter().map(|c| c.name.clone()).collect());
        let scan = LogicalPlan::Scan { table: sub.clone(), names: columns, needed: needed.to_vec(), located: false };
        let superrows = LogicalPlan::Project { exprs: (0..width).map(Expr::Column).collect(), input: Box::new(scan) };
        LogicalPlan::SetOp { op: SetOp::Union, all: true, compared: None, left: Box::new(plan), right: Box::new(superrows) }
    })
}

/// [ISO/IEC 9075-2:2023 §7.16 SR25, §15.6 GR9] Of the plan of an updatable
/// cursor over a typed table read with its proper subtables, the tables of
/// the family it reads, the typed table first: the members whose rows the
/// cursor covers, each read by a plan of its own (`member_plan`). None of
/// a plan that reads no such family.
#[must_use]
pub fn family_in(db: &State, plan: &LogicalPlan) -> Option<Vec<QualifiedName>> {
    if let Some(tables) = family_tables(plan)
        && tables.first().is_some_and(|t| db.catalog.table(t).is_some_and(|d| d.typed.is_some()))
        && tables.get(1..) == tables.first().map(|t| proper_subtables(&*db.catalog, t)).as_deref()
    {
        return Some(tables);
    }
    plan.inputs().find_map(|input| family_in(db, input))
}

/// The tables a UNION ALL that `family_plan` made reads, in its order: its
/// own scan's, then each subtable's under its projection.
fn family_tables(plan: &LogicalPlan) -> Option<Vec<QualifiedName>> {
    if let LogicalPlan::Scan { table, .. } = plan {
        return Some(vec![table.clone()]);
    }
    let LogicalPlan::SetOp { op: SetOp::Union, all: true, compared: None, left, right } = plan else { return None };
    let LogicalPlan::Project { input, .. } = &**right else { return None };
    let LogicalPlan::Scan { table, .. } = &**input else { return None };
    let mut tables = family_tables(left)?;
    tables.push(table.clone());
    Some(tables)
}

/// `plan` with the UNION ALL that reads the family `members` (`family_in`)
/// replaced by its arm of member `k`: the typed table's own rows, or a
/// subtable's superrows.
#[must_use]
pub fn member_plan(plan: LogicalPlan, members: &[QualifiedName], k: usize) -> LogicalPlan {
    if family_tables(&plan).is_some_and(|t| t.len() > 1 && t == members) {
        return arm(plan, members.len(), k);
    }
    plan.map_inputs(|input| member_plan(input, members, k))
}

/// Arm `k` of the `n` of a UNION ALL `family_plan` made.
fn arm(plan: LogicalPlan, n: usize, k: usize) -> LogicalPlan {
    let LogicalPlan::SetOp { left, right, .. } = plan else { return plan };
    if n < 2 {
        return LogicalPlan::SetOp { op: SetOp::Union, all: true, compared: None, left, right };
    }
    if k.saturating_add(1) == n { *right } else { arm(*left, n.saturating_sub(1), k) }
}
