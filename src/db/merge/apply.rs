//! [ISO/IEC 9075-2:2023 §14.12 GR6a, GR7] What a MERGE decided, done:
//! each clause's updates, deletes or inserts in the order the clauses are
//! written, a target row another transaction changed since the snapshot
//! checked again (RULES.md D29), then each row updated or inserted
//! checked against the views WITH CHECK OPTION the MERGE spans.

use std::collections::HashMap;

use super::super::assignment::{Assigned, check_option, set_columns_of};
use super::super::integrity::Effects;
use super::super::{StatementContext, api};
use super::decide::{Merge, Planned, UPDATE_RULE};
use crate::binder::{BoundAction, BoundWhen};
use crate::error::Result;
use crate::exec::{HeldRows, eval_cond};
use crate::logical::{Cond, Expr, Truth};
use crate::storage::{Row, RowId};
use crate::txn::Again;
use crate::value::Value;

/// [ISO/IEC 9075-2:2023 §14.12 GR7] The rule `check_option` cites.
const CHECK_RULE: &str = "ISO/IEC 9075-2:2023 §14.12 GR7";

/// Of each target row an update wrote, a version another transaction
/// committed since the snapshot, the row written in its place; none of
/// one left alone.
pub(super) type Rewritten = HashMap<RowId, Option<Row>>;

/// [GR6a] Does `planned`, clause by clause in order, into `effects`: the
/// rows updates wrote in place of those planned.
///
/// # Errors
///
/// A row's write fails, or a value of a row checked again.
pub(super) fn applied(
    cx: StatementContext<'_>,
    merge: &Merge<'_>,
    planned: &mut [Planned],
    effects: &mut Effects<'_>,
) -> Result<Rewritten> {
    let mut again = Rewritten::new();
    for (clause, plan) in merge.bound.clauses.iter().zip(planned.iter_mut()) {
        let unplanned = || api("[DESIGN] a clause of a MERGE is not the clause of its plan");
        // [ISO/IEC 9075-2:2023 §15.19] each clause's BEFORE triggers as its
        // rows are decided, before they are written.
        if effects.triggered.is_some() {
            before_clause(cx, (merge, clause), plan, effects)?;
        }
        match plan {
            Planned::Updates(rows) => {
                let BoundWhen::Matched { condition, action: BoundAction::Update(set) } = clause else { return Err(unplanned()) };
                updates(cx, (merge, &mut again), (condition.as_ref(), set), rows, effects)?;
            }
            Planned::Deletes(rows) => {
                let BoundWhen::Matched { condition, .. } = clause else { return Err(unplanned()) };
                deletes(cx, merge, condition.as_ref(), rows, effects)?;
            }
            Planned::Inserts(target, rows) => inserts(cx, merge, *target, rows, effects)?,
        }
    }
    Ok(again)
}

/// [ISO/IEC 9075-2:2023 §15.8 GR2, §15.11 GR4, §15.14 GR5] The BEFORE
/// triggers of `clause`, planned as `plan`, executed: its rows as they
/// left them.
fn before_clause(
    cx: StatementContext<'_>,
    (merge, clause): (&Merge<'_>, &BoundWhen),
    plan: &mut Planned,
    effects: &mut Effects<'_>,
) -> Result<()> {
    let bound = merge.bound;
    let target = |joined: &Row| joined.get(bound.source_width..).map(<[Value]>::to_vec).unwrap_or_default();
    match plan {
        Planned::Updates(rows) => {
            let BoundWhen::Matched { action: BoundAction::Update(set), .. } = clause else { return Ok(()) };
            let columns = set_columns_of(set, &bound.generated);
            let mut changes = HeldRows::new(cx.st.list_space());
            rows.visit(&mut |(id, joined, new)| changes.push((*id, target(joined), new.clone())))?;
            let altered = effects.before_update(cx, (&bound.table, &columns), changes)?;
            let mut out = HeldRows::new(cx.st.list_space());
            for (row, change) in std::mem::replace(rows, HeldRows::new(cx.st.list_space())).into_iter().zip(altered) {
                let ((id, joined, _), (_, _, new)) = (row?, change?);
                out.push((id, joined, new))?;
            }
            *rows = out;
        }
        Planned::Deletes(rows) => {
            let mut olds = HeldRows::new(cx.st.list_space());
            rows.visit(&mut |(id, joined)| olds.push((*id, target(joined))))?;
            effects.before_delete(cx, &bound.table, &mut olds)?;
        }
        Planned::Inserts(at, rows) => {
            let table = merge.targets.get(*at).ok_or_else(|| api("[DESIGN] a WHEN NOT MATCHED clause of a MERGE has no target"))?.name();
            let given = std::mem::replace(rows, HeldRows::new(cx.st.list_space()));
            *rows = effects.before_insert(cx, table, given)?;
        }
    }
    Ok(())
}

/// [GR6a i 5] A WHEN MATCHED clause's updates, of search condition
/// `condition` and SET values `set`. A row another
/// transaction changed is written in the version it committed, if it still
/// matches by the clause, its values computed from it.
fn updates(
    StatementContext { st, tx }: StatementContext<'_>,
    (merge, again): (&Merge<'_>, &mut Rewritten),
    (condition, set): (Option<&Cond>, &[(usize, Option<Expr>)]),
    rows: &mut HeldRows<(RowId, Row, Row)>,
    effects: &mut Effects<'_>,
) -> Result<()> {
    let bound = merge.bound;
    let columns = set_columns_of(set, &bound.generated);
    let assigned = Assigned { desc: &bound.desc, set, generated: &bound.generated };
    rows.visit(&mut |(id, joined, new)| {
        // [GR6a i 4] a row changed through a cursor: a cursor operation conflict.
        effects.cursors.acted_on(&bound.table, *id);
        let mut recomputed = None;
        let mut recheck = |newest: &Row| {
            let Some(rejoined) = still_matched(merge, condition, newest, joined)? else { return Ok(Again::Skip) };
            let row = assigned.candidate((newest, &rejoined), (UPDATE_RULE, merge.runner))?;
            recomputed = Some(row.clone());
            Ok(Again::Update(row))
        };
        match st.rewrite_row(tx, (&bound.table, *id), Some(new), &mut recheck)? {
            Some(old) => {
                effects.updated(&bound.table, *id, (old, recomputed.as_ref().unwrap_or(new)), &columns)?;
                if let Some(row) = recomputed {
                    again.insert(*id, Some(row));
                }
            }
            None => {
                again.insert(*id, None);
            }
        }
        Ok(())
    })
}

/// [GR6a i 7] A WHEN MATCHED clause's deletes, of search condition
/// `condition`. A row another transaction changed is
/// deleted in the version it committed, if it still matches by the clause.
fn deletes(
    StatementContext { st, tx }: StatementContext<'_>,
    merge: &Merge<'_>,
    condition: Option<&Cond>,
    rows: &mut HeldRows<(RowId, Row)>,
    effects: &mut Effects<'_>,
) -> Result<()> {
    let table = &merge.bound.table;
    rows.visit(&mut |(id, joined)| {
        // [GR6a i 4] a row deleted through a cursor: a cursor operation conflict.
        effects.cursors.acted_on(table, *id);
        let mut recheck =
            |newest: &Row| Ok(if still_matched(merge, condition, newest, joined)?.is_some() { Again::Delete } else { Again::Skip });
        if let Some(old) = st.rewrite_row(tx, (table, *id), None, &mut recheck)? {
            effects.deleted(table, *id, old)?;
        }
        Ok(())
    })
}

/// [GR6a ii 8] A WHEN NOT MATCHED clause's candidate rows inserted into
/// the `target`th of the MERGE's targets.
fn inserts(
    StatementContext { st, tx }: StatementContext<'_>,
    merge: &Merge<'_>,
    target: usize,
    rows: &mut HeldRows<Row>,
    effects: &mut Effects<'_>,
) -> Result<()> {
    let target = merge.targets.get(target).ok_or_else(|| api("[DESIGN] a WHEN NOT MATCHED clause of a MERGE has no target"))?;
    let table = target.name();
    rows.visit(&mut |row| effects.inserted(table, st.insert_row(tx, table, row)?, row))
}

/// [GR7] Each row `planned` updated or inserted, once all are written,
/// of the views WITH CHECK OPTION its update or insert spans: of a row
/// another transaction changed since the snapshot, the row written in
/// its place (`again`), none of one left alone.
///
/// # Errors
///
/// SQLSTATE 44000 if a row is not in such a view; a held row cannot be
/// read back.
pub(super) fn checked(merge: &Merge<'_>, planned: &mut [Planned], again: &Rewritten) -> Result<()> {
    let checks = &merge.bound.checks;
    for plan in planned {
        match plan {
            Planned::Updates(rows) => {
                if checks.is_empty() {
                    continue;
                }
                let check = |row: &Row| check_option(checks, [row], (CHECK_RULE, merge.runner));
                rows.visit(&mut |(id, _, new)| match again.get(id) {
                    Some(None) => Ok(()),
                    Some(Some(row)) => check(row),
                    None => check(new),
                })?;
            }
            Planned::Inserts(target, rows) => {
                let target =
                    merge.targets.get(*target).ok_or_else(|| api("[DESIGN] a WHEN NOT MATCHED clause of a MERGE has no target"))?;
                if !target.checks().is_empty() {
                    rows.visit(&mut |row| check_option(target.checks(), [row], (CHECK_RULE, merge.runner)))?;
                }
            }
            Planned::Deletes(_) => {}
        }
    }
    Ok(())
}

/// The version `newest` of a target row, committed by another transaction
/// since the snapshot, joined to the source row of `joined`, if the ON
/// condition and `condition` both still hold of it (RULES.md D29).
fn still_matched(merge: &Merge<'_>, condition: Option<&Cond>, newest: &Row, joined: &Row) -> Result<Option<Row>> {
    let width = merge.bound.desc.columns.len();
    let rejoined: Row = newest.iter().chain(joined.iter().skip(width)).cloned().collect();
    let holds = |c: &Cond| eval_cond(c, &rejoined, merge.runner).map(|t| t == Truth::True);
    if !holds(&merge.bound.on)? {
        return Ok(None);
    }
    let chosen = match condition {
        Some(c) => holds(c)?,
        None => true,
    };
    Ok(chosen.then_some(rejoined))
}
