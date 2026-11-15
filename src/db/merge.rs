//! [ISO/IEC 9075-2:2023 §14.12] A `<merge statement>` run: its source's
//! rows joined to the target rows they match, all read before any row is
//! changed [GR5], then each WHEN clause's updates, deletes and inserts, in
//! the order written [GR6].
//!
//! It is bound, or taken as a prepared statement keeps it (RULES.md D45),
//! here; each joined row is read and what it does decided in
//! `merge/decide.rs`; the changes are made and checked in
//! `merge/apply.rs`.

mod apply;
mod decide;

use std::sync::Arc;

use self::decide::Merge;
use super::generated::Inserted;
use super::prepared::{Keeping, KeptPlan};
use super::write::{InsertInto, InsertRules, Target};
use super::{Completion, Session, State, StatementContext, api, plan};
use crate::ast::{MergeStatement, MergeWhen};
use crate::binder::{self, BoundAction, BoundMerge, BoundWhen, ObjectColumn};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::exec::Runner;
use crate::logical::{Expr, LogicalPlan, SortKey, filled, filled_cond, filled_expr};
use crate::params::Params;
use crate::physical::Plan;
use crate::value::Value;

/// [ISO/IEC 9075-2:2023 §14.12 SR2, SR14, SR17, AR1b iii] The rules a
/// WHEN NOT MATCHED clause's insert cites.
pub(super) const MERGE_INSERT_RULES: InsertRules = InsertRules {
    read_only: "ISO/IEC 9075-2:2023 §14.12 SR2",
    missing: "ISO/IEC 9075-2:2023 §14.12 SR14",
    twice: "ISO/IEC 9075-2:2023 §14.12 SR17",
    privilege: "ISO/IEC 9075-2:2023 §14.12 AR1b iii",
};

/// What a MERGE did: its row count, and whether its source gave any row,
/// of which none is no data [GR8].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Merged {
    /// Its ROW_COUNT, of the rows of its target joined to its source
    /// before it ran [ISO/IEC 9075-2:2023 §23.1 GR2f ii].
    pub(super) changed: usize,
    /// Whether the source's table Q had a row.
    pub(super) read: bool,
}

impl Session {
    /// `merge_statement`, its completion placed in the diagnostics area:
    /// no data only of a source of no rows [ISO/IEC 9075-2:2023 §14.12
    /// GR8], so one whose rows change none completes, its ROW_COUNT as
    /// §23.1 GR2f ii says.
    ///
    /// # Errors
    ///
    /// As `merge_statement`.
    pub(super) fn merge_completed(
        &self,
        cx: StatementContext<'_>,
        stmt: &MergeStatement,
        params: &Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<Completion> {
        let m = self.merge_statement(cx, stmt, params, keeping)?;
        let completion = self.affected(m.changed);
        Ok(if m.read { Completion::Success } else { completion })
    }

    /// [ISO/IEC 9075-2:2023 §14.12] `<merge statement>` `stmt`; of a
    /// prepared statement, with the plans it keeps (RULES.md D45).
    ///
    /// # Errors
    ///
    /// As `merge_bound`; a row's search condition or value fails; a source
    /// row is the matching row of more than one target row of a clause
    /// (SQLSTATE 21000); a constraint or a view's CHECK OPTION is broken.
    fn merge_statement(
        &self,
        cx: StatementContext<'_>,
        stmt: &MergeStatement,
        params: &Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<Merged> {
        let StatementContext { st, tx } = cx;
        let (bound, targets) = self.merge_kept(st, stmt, params, keeping)?;
        let physical = locked_rows(cx, &bound)?;
        self.settle_cursors(st, &bound.table);
        let runner = Runner::new(st, tx.id);
        let merge = Merge { bound: &bound, targets: &targets, runner: &runner, id_column: row_id_column(&bound)? };
        // [GR5, GR6a i 1 A, GR6a ii 6] Every row read and what it does
        // decided before any row of T is changed.
        let (mut planned, read) = decide::planned(cx, &merge, physical)?;
        let mut effects = self.effects(st);
        let again = apply::applied(cx, &merge, &mut planned, &mut effects)?;
        // [GR7] Each row updated or inserted checked once all are written.
        apply::checked(&merge, &mut planned, &again)?;
        st.end_statement(tx, effects)?;
        Ok(Merged { changed: read.row_count(&bound.clauses), read: read.rows > 0 })
    }

    /// `stmt`, bound as `params` say, and the table each of its WHEN NOT
    /// MATCHED clauses inserts into, in order (`insert_target_of`).
    ///
    /// # Errors
    ///
    /// A rule of §14.12, or of what it reads, is broken, or a privilege it
    /// needs is not held.
    pub(super) fn merge_bound(&self, st: &State, stmt: &MergeStatement, params: &Params) -> Result<(BoundMerge, Vec<Target>)> {
        let mut targets = Vec::new();
        for clause in &stmt.clauses {
            match clause {
                MergeWhen::NotMatched { insert, .. } => {
                    let into = InsertInto {
                        table: &stmt.target,
                        columns: insert.columns.as_deref(),
                        given: (Inserted::Row(&insert.values), insert.overriding),
                        rules: &MERGE_INSERT_RULES,
                    };
                    targets.push(self.insert_target_of(st, &into, params.noted())?);
                }
                MergeWhen::Matched { .. } => {}
            }
        }
        let objects: Vec<Vec<ObjectColumn>> = targets.iter().map(Target::objects).collect();
        let bound = self.bound(st, params, |cx| binder::bind_merge(cx, stmt, &objects))?;
        Ok((bound, targets))
    }

    /// `stmt` bound, and the tables it inserts into: of a prepared
    /// statement, as it keeps them, filled with its values and its
    /// defaults those of now (RULES.md D45).
    fn merge_kept(
        &self,
        st: &State,
        stmt: &MergeStatement,
        params: &Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<(BoundMerge, Vec<Target>)> {
        let values = params.dynamic_values();
        if let Some(KeptPlan::Merge(bound, targets)) = keeping.map(Keeping::plan).transpose()?.flatten() {
            let now = self.now.get();
            let targets = targets.iter().map(|t| t.clone().refreshed(&self.auth(), now)).collect::<Result<Vec<_>>>()?;
            return Ok((filled_merge(&bound, values)?, targets));
        }
        let kept = keeping.map(|_| params.keeping());
        let binding = kept.as_ref().unwrap_or(params);
        let (bound, targets) = self.merge_bound(st, stmt, binding)?;
        let Some(k) = keeping else { return Ok((bound, targets)) };
        k.made((!binding.held()).then(|| KeptPlan::Merge(Arc::new(bound.clone()), targets.clone().into())))?;
        Ok((filled_merge(&bound, values)?, targets))
    }
}

/// The plan of the joined rows of `bound`, sorted by their target rows'
/// row ids so the pairs of each target row come together, its tables
/// locked: T to be written, and every table it or its clauses read.
///
/// # Errors
///
/// A lock is refused.
fn locked_rows(StatementContext { st, tx }: StatementContext<'_>, bound: &BoundMerge) -> Result<Plan> {
    // [DESIGN] A sort holds what passes the work area in runs of a spill
    // area (RULES.md D38), so the rows may outgrow memory.
    let sorted = LogicalPlan::Sort { keys: vec![SortKey::ascending(row_id_column(bound)?)], input: Box::new(bound.rows.clone()) };
    let (optimized, _, physical) = plan(st, sorted);
    st.update_lock(tx, &bound.table)?;
    for t in optimized.tables().into_iter().chain(clause_tables(bound)) {
        st.read_lock(tx, &t)?;
    }
    Ok(physical)
}

/// The tables the subqueries of the clauses of `bound` read: of their
/// search conditions, SET values and insert values.
fn clause_tables(bound: &BoundMerge) -> Vec<QualifiedName> {
    let mut tables = bound.on.tables();
    for clause in &bound.clauses {
        match clause {
            BoundWhen::Matched { condition, action } => {
                tables.extend(condition.iter().flat_map(crate::logical::Cond::tables));
                match action {
                    BoundAction::Update(set) => tables.extend(set.iter().filter_map(|(_, e)| e.as_ref()).flat_map(Expr::tables)),
                    BoundAction::Delete => {}
                }
            }
            BoundWhen::NotMatched { values, .. } => tables.extend(values.iter().flat_map(Expr::tables)),
        }
    }
    tables
}

/// Where a joined row of `bound` holds its target row's row id: after the
/// target's columns, the source's and each WHEN NOT MATCHED clause's flag
/// (`BoundMerge`).
///
/// # Errors
///
/// SQLCODE -910 if the count overflows.
fn row_id_column(bound: &BoundMerge) -> Result<usize> {
    let flags = bound.clauses.iter().filter(|c| matches!(c, BoundWhen::NotMatched { flag: Some(_), .. })).count();
    bound
        .desc
        .columns
        .len()
        .checked_add(bound.source_width)
        .and_then(|n| n.checked_add(flags))
        .ok_or_else(|| api("[DESIGN] a joined row of a MERGE is too wide"))
}

/// `bound`, a kept plan, its slots filled with `values`: its rows, its ON
/// condition, and each clause's condition and values.
///
/// # Errors
///
/// As `filled`.
fn filled_merge(bound: &BoundMerge, values: &[Value]) -> Result<BoundMerge> {
    let BoundMerge { named, table, desc, source_width, rows, on, clauses, checks, auth, generated } = bound.clone();
    let clauses = clauses.into_iter().map(|c| filled_when(c, values)).collect::<Result<_>>()?;
    let (rows, on) = (filled(rows, values)?, filled_cond(on, values)?);
    Ok(BoundMerge { named, table, desc, source_width, rows, on, clauses, checks, auth, generated })
}

/// The clause `clause`, its slots filled with `values`.
fn filled_when(clause: BoundWhen, values: &[Value]) -> Result<BoundWhen> {
    let fill = |e: Expr| filled_expr(e, values);
    Ok(match clause {
        BoundWhen::Matched { condition, action } => {
            let condition = condition.map(|c| filled_cond(c, values)).transpose()?;
            let action = match action {
                BoundAction::Update(set) => {
                    BoundAction::Update(set.into_iter().map(|(c, e)| Ok((c, e.map(fill).transpose()?))).collect::<Result<_>>()?)
                }
                BoundAction::Delete => BoundAction::Delete,
            };
            BoundWhen::Matched { condition, action }
        }
        BoundWhen::NotMatched { flag, object, values: given } => {
            BoundWhen::NotMatched { flag, object, values: given.into_iter().map(fill).collect::<Result<_>>()? }
        }
    })
}

// Unit tests: the choice of each row's clause (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
