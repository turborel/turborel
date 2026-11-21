//! [ISO/IEC 9075-2:2023 §14.12] A `<merge statement>` run: its source's
//! rows joined to the target rows they match, all read before any row is
//! changed [GR5], then each WHEN clause's updates, deletes and inserts, in
//! the order written [GR6].
//!
//! It is bound, or taken as a prepared statement keeps it (RULES.md D45),
//! here; each joined row is read and what it does decided in
//! `merge/decide.rs`; the changes are made and checked in
//! `merge/apply.rs`; a clause a view's INSTEAD OF trigger takes is given
//! to it in `merge/routed.rs`.

mod apply;
mod decide;
mod routed;

use std::sync::Arc;

use self::decide::Merge;
pub(super) use self::routed::Inserting;
use self::routed::{Routing, held_rows, routing, view_rows_of};
use super::generated::Inserted;
use super::prepared::{Keeping, KeptPlan};
use super::routines::Hosted;
use super::write::{InsertInto, InsertRules, insert_target_in};
use super::{Completion, Session, State, StatementContext, api, plan};
use crate::ast::{MergeStatement, MergeWhen};
use crate::binder::qualify;
use crate::binder::{self, BoundAction, BoundMerge, BoundWhen, ObjectColumn};
use crate::catalog::{Principal, QualifiedName};
use crate::error::Result;
use crate::exec::TransitionRows;
use crate::logical::{Expr, LogicalPlan, SortKey, filled, filled_cond, filled_expr};
use crate::params::Params;
use crate::physical::Plan;
use crate::value::Value;
use crate::value::datetime::Now;

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
        if let Some(p) = &stmt.property {
            return self.property_written(cx, stmt, p, params);
        }
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
        let routed = routing(st, stmt, &self.default_schema());
        // [ISO/IEC 9075-2:2023 §15.10 GR3a, §15.13 GR3a, §15.16 GR3a] of a
        // view its triggers take whole, its rows read first.
        let rows_held = if routed.held { Some(held_rows(cx, &qualify(&stmt.target, &self.default_schema()))?) } else { None };
        let (bound, targets) = self.merge_kept(st, stmt, params, (keeping, &routed, rows_held.as_ref()))?;
        let physical = locked_rows(cx, &bound, routed.held)?;
        if !routed.held {
            for table in std::iter::once(&bound.table).chain(&bound.members) {
                self.settle_cursors(st, table);
            }
        }
        let runner = Hosted::new(Some(self), cx);
        let id_column = row_id_column(&bound)?;
        let merge = Merge { bound: &bound, targets: &targets, runner: &runner, id_column, routes: &routed.routes, held: routed.held };
        // [GR5, GR6a i 1 A, GR6a ii 6] Every row read and what it does
        // decided before any row of T is changed.
        let (mut planned, read) = decide::planned(cx, &merge, physical)?;
        let mut effects = self.effects(st);
        let again = apply::applied((self, cx), &merge, &mut planned, &mut effects)?;
        // [GR7] Each row updated or inserted checked once all are written.
        apply::checked(&merge, &mut planned, &again)?;
        st.end_statement(tx, effects)?;
        Ok(Merged { changed: read.row_count(&bound.clauses), read: read.rows > 0 })
    }

    /// `stmt`, bound as `params` say, and where each of its WHEN NOT
    /// MATCHED clauses inserts, in order: the table (`insert_target_of`),
    /// or the INSTEAD OF trigger that takes it (`merge/routed.rs`). T is
    /// read as its rows `held`, if given; of a view its triggers take
    /// whole, as no rows if not.
    ///
    /// # Errors
    ///
    /// A rule of §14.12, or of what it reads, is broken, or a privilege it
    /// needs is not held.
    pub(super) fn merge_bound(
        &self,
        st: &State,
        stmt: &MergeStatement,
        params: &Params,
        held: Option<&TransitionRows>,
    ) -> Result<(BoundMerge, Vec<Inserting>)> {
        let (authorization, schema) = (self.authorization(), self.default_schema());
        let user = authorization.of(st.catalog.roles());
        let (targets, routed) = inserting(st, stmt, ((user, &schema, self.now.get()), params))?;
        let objects: Vec<Vec<ObjectColumn>> = targets.iter().map(Inserting::objects).collect();
        let none = routed.then(|| TransitionRows::new(st.list_space()));
        let rows = held.or(none.as_ref()).filter(|_| routed);
        let bound = self.bound(st, params, |cx| binder::bind_merge(cx, stmt, &objects, rows))?;
        Ok((bound, targets))
    }

    /// `stmt` bound, and where it inserts: of a prepared statement, as it
    /// keeps them, filled with its values and its defaults those of now
    /// (RULES.md D45). Of a view its triggers take whole, as `routed` says,
    /// bound each time over its rows `held`, as those are of the time.
    fn merge_kept(
        &self,
        st: &State,
        stmt: &MergeStatement,
        params: &Params,
        (keeping, routed, held): (Option<&Keeping<'_>>, &Routing, Option<&TransitionRows>),
    ) -> Result<(BoundMerge, Vec<Inserting>)> {
        let values = params.dynamic_values();
        if routed.held {
            let (bound, targets) = self.merge_bound(st, stmt, params, held)?;
            return Ok((filled_merge(&bound, values)?, targets));
        }
        if let Some(KeptPlan::Merge(bound, targets)) = keeping.map(Keeping::plan).transpose()?.flatten() {
            let (now, authorization) = (self.now.get(), self.authorization());
            let user = authorization.of(st.catalog.roles());
            let targets = targets.iter().map(|t| t.clone().refreshed(user, now)).collect::<Result<Vec<_>>>()?;
            return Ok((filled_merge(&bound, values)?, targets));
        }
        let kept = keeping.map(|_| params.keeping());
        let binding = kept.as_ref().unwrap_or(params);
        let (bound, targets) = self.merge_bound(st, stmt, binding, None)?;
        let Some(k) = keeping else { return Ok((bound, targets)) };
        k.made((!binding.held()).then(|| KeptPlan::Merge(Arc::new(bound.clone()), targets.clone().into())))?;
        Ok((filled_merge(&bound, values)?, targets))
    }
}

/// Where each WHEN NOT MATCHED clause of `stmt` inserts, in order, of a
/// statement `user`'s, its unqualified names in `schema`, its
/// defaults those of `now`, bound with `params`: the table, or the INSTEAD
/// OF trigger that takes it (`merge/routed.rs`); and whether T is a view
/// its triggers take whole, read as its rows.
///
/// # Errors
///
/// As `insert_target_in` and `view_rows_of`.
pub(super) fn inserting(
    st: &State,
    stmt: &MergeStatement,
    ((user, schema, now), params): ((Principal<'_>, &str, Now), &Params),
) -> Result<(Vec<Inserting>, bool)> {
    let routed = routing(st, stmt, schema);
    let name = qualify(&stmt.target, schema);
    let mut targets = Vec::new();
    for (clause, route) in stmt.clauses.iter().zip(&routed.routes) {
        let MergeWhen::NotMatched { insert, .. } = clause else { continue };
        if let Some(instead) = route {
            let into = (&name, insert, &MERGE_INSERT_RULES);
            targets.push(Inserting::Instead(view_rows_of(st, into, (instead, routed.held), (user, now))?));
            continue;
        }
        let into = InsertInto {
            table: &stmt.target,
            columns: insert.columns.as_deref(),
            given: (Inserted::Row(&insert.values), insert.overriding),
            rules: &MERGE_INSERT_RULES,
            horizon: params.horizon(),
        };
        targets.push(Inserting::Table(Box::new(insert_target_in(st, &into, (user, schema, now), params.noted())?)));
    }
    Ok((targets, routed.held))
}

/// The plan of the joined rows of `bound`, sorted by their target rows'
/// row ids so the pairs of each target row come together, its tables
/// locked: T to be written, unless it is a view `held`, and every table
/// it or its clauses read.
///
/// # Errors
///
/// A lock is refused.
fn locked_rows(StatementContext { st, tx }: StatementContext<'_>, bound: &BoundMerge, held: bool) -> Result<Plan> {
    // [DESIGN] A sort holds what passes the work area in runs of a spill
    // area (RULES.md D38), so the rows may outgrow memory.
    // Each target row's pairs together: of T's family, by its table too.
    let id = row_id_column(bound)?;
    let mut keys = vec![SortKey::ascending(id)];
    if !bound.members.is_empty() {
        keys.push(SortKey::ascending(id.saturating_add(1)));
    }
    let sorted = LogicalPlan::Sort { keys, input: Box::new(bound.rows.clone()) };
    let (optimized, _, physical) = plan(st, sorted);
    if !held {
        st.update_lock(tx, &bound.table)?;
        for member in &bound.members {
            st.update_lock(tx, member)?;
        }
    }
    for t in optimized.tables().into_iter().chain(clause_tables(bound)) {
        st.read_lock(tx, &t)?;
    }
    Ok(super::explain::analyzing(tx, st, (physical, Some(&optimized))))
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
    let BoundMerge { named, table, desc, source_width, rows, on, clauses, checks, auth, generated, members } = bound.clone();
    let clauses = clauses.into_iter().map(|c| filled_when(c, values)).collect::<Result<_>>()?;
    let (rows, on) = (filled(rows, values)?, filled_cond(on, values)?);
    Ok(BoundMerge { named, table, desc, source_width, rows, on, clauses, checks, auth, generated, members })
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
