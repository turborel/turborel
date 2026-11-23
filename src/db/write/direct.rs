//! \[DESIGN\] A compiled program's searched UPDATE or DELETE of one row by
//! its key, its plan kept a point read, written straight on its table
//! (RULES.md D841).
//!
//! It writes as `update_rows` and `delete_rows` write, by the same row
//! lock, conflict wait and recheck (`State::rewrite_row`), the candidate
//! row made as theirs is, with its casts and generated columns. What they
//! note of each row for the end of the statement is left out: no trigger
//! may read it, no delta table but the one its caller makes of the row it
//! returns, and the constraints its end checks are those of the row it
//! wrote alone (`integrity/straight.rs`). A change whose increments would
//! be reserved (RULES.md D452), or whose plan is not a point read as this
//! execution runs it, is handed back to the general path. An INSERT of
//! the rows of a query is written as `insert_rows` writes them, a batch
//! at a time where it may, and its end checks the constraints of those
//! rows alone, each as it is held, as its end would (RULES.md D901).

use super::{ObjectRows, Written};
use crate::catalog::ColumnDefault;
use crate::db::assignment::candidate;
use crate::db::escrow::reserving;
use crate::db::integrity::Straight;
use crate::db::prepared::Keeping;
use crate::db::routines::Hosted;
use crate::db::{Session, StatementContext};
use crate::error::Result;
use crate::exec::Slots;
use crate::logical::LogicalPlan;
use crate::params::Params;
use crate::storage::{Row, RowId};
use crate::txn::Again;
use crate::txn::writing::Writing;

/// What a point change did: no row found [X3.135-1986 §7.3 GR3a iv]; or
/// the row it found written, as it was before and is now, none of a row
/// deleted, or nothing if another transaction deleted it first.
pub(in crate::db) enum Pointed {
    Unfound,
    Wrote(Option<(Row, Option<Row>)>),
}

/// `w`, an UPDATE, or DELETE if `delete`, of `session`, run straight on
/// its table, its statement ended with the checks of `straight`, its delta
/// table's reader, if any, showing the columns of `shown`: what it did; or
/// `w` again, if it is to run the general path.
///
/// # Errors
///
/// As `update_rows` and `delete_rows`, and the statement's end.
pub(in crate::db) fn point_change(
    (session, cx): (&Session, StatementContext<'_>),
    (w, straight, delete, shown): (Written, &Straight, bool, u64),
) -> Result<std::result::Result<Pointed, Written>> {
    let StatementContext { st, tx } = cx;
    let ObjectRows::Point(kept, reads) = &w.rows else { return Ok(Err(w)) };
    let Some(point) = kept.point(st) else { return Ok(Err(w)) };
    if !delete && reserving(st, tx, &w, shown).is_some() {
        return Ok(Err(w));
    }
    // T, to be written, and the tables its condition and SET values read.
    st.update_lock(tx, &w.bound.table)?;
    for t in reads.iter().chain(straight.set_tables()) {
        st.read_lock(tx, t)?;
    }
    let Some((id, seen)) = point.entry(st, tx.id, w.slots.as_deref().unwrap_or_default())? else {
        // Of a catalog with triggers, its statement ends with no row
        // changed, as `update_rows` ends it.
        if st.catalog.any_triggers() {
            st.end_straight(tx, (&w.bound.table, straight), &[], None)?;
        }
        return Ok(Ok(Pointed::Unfound));
    };
    let db = w.db(st);
    let hosted = Hosted::in_db(Some(session), cx, &db);
    // What its write needs of its table, found once (RULES.md D872).
    let writing = st.writing(&w.bound.table)?;
    let at = (&writing, id);
    let rows = if delete { deleted(cx, (&w, &hosted), at)? } else { updated(cx, (&w, &hosted), (at, &seen), straight.set())? };
    let wrote = rows.as_ref().map(|_| id);
    // The row an UPDATE wrote, as it holds it (RULES.md D880).
    let held = rows.as_ref().and_then(|(_, new)| new.as_ref());
    st.end_straight(tx, (&w.bound.table, straight), wrote.as_slice(), held)?;
    Ok(Ok(Pointed::Wrote(rows)))
}

/// Row `id` deleted by `w`, its condition read by `hosted`: the row it
/// was, none if another transaction deleted it first.
fn deleted(
    cx: StatementContext<'_>,
    (w, hosted): (&Written, &Hosted<'_>),
    at: (&Writing<'_>, RowId),
) -> Result<Option<(Row, Option<Row>)>> {
    // A row another transaction changed is deleted in the version it
    // committed, if it is still one to delete.
    let mut recheck = |newest: &Row| Ok(if w.holds_of(newest, hosted.runner())? { Again::Delete } else { Again::Skip });
    Ok(cx.st.rewrite_row_in(cx.tx, at, None, &mut recheck)?.map(|gone| (gone, None)))
}

/// Row `id`, seen as `seen`, updated by `w`, its values computed by
/// `hosted`: the row it was and the row it is, none if another
/// transaction deleted it first.
fn updated(
    cx: StatementContext<'_>,
    (w, hosted): (&Written, &Hosted<'_>),
    (at, seen): ((&Writing<'_>, RowId), &Row),
    set: Option<u64>,
) -> Result<Option<(Row, Option<Row>)>> {
    let new = candidate(w, seen, ("ISO/IEC 9075:1992 §13.10 GR8", hosted))?;
    // A row another transaction changed is written in the version it
    // committed, if it is still one to change, its values computed from it.
    let mut recomputed = None;
    let mut recheck = |newest: &Row| {
        if !w.holds_of(newest, hosted.runner())? {
            return Ok(Again::Skip);
        }
        let row = candidate(w, newest, ("ISO/IEC 9075:1992 §13.10 GR8", hosted))?;
        recomputed = Some(row.clone());
        Ok(Again::Update(row))
    };
    // [DESIGN] RULES.md D875: the one write to be told the columns set,
    // where its statement reads nothing of the row before (`Straight::set`).
    let written = match set {
        Some(_) | None => cx.st.rewrite_row_in(cx.tx, at, Some(&new), &mut recheck)?,
    };
    Ok(written.map(|before| (before, Some(recomputed.unwrap_or(new)))))
}

impl super::Target {
    /// Whether each column's default is the same for every user at every
    /// instant: a value, or none. Its defaults as it was kept are then
    /// those of any statement (`Target::refreshed`), and an execution
    /// takes it as it is, with no copy.
    pub(in crate::db) fn fixed_defaults(&self) -> bool {
        self.desc.columns.iter().all(|c| {
            let of_domain = c.domain.as_ref().and_then(|d| d.default.as_ref());
            match c.default.as_ref().or(of_domain).filter(|_| c.generated.is_none()) {
                Some(ColumnDefault::Value(_)) | None => true,
                Some(ColumnDefault::User(_) | ColumnDefault::Datetime(..)) => false,
            }
        })
    }

    /// Of an INSERT into it, the constraints its statement checks of each
    /// row it inserts, if it may write them straight: none through a view
    /// WITH CHECK OPTION, of a table with a system-time period, whose
    /// rows the general path stamps before they go in [ISO/IEC
    /// 9075-2:2023 §15.11 GR3b, GR3c], or if its end would check more.
    pub(in crate::db) fn straight(&self, st: &crate::db::State) -> Option<Straight> {
        if !self.checks.is_empty() || super::system_time::system_time(&self.desc).is_some() {
            return None;
        }
        st.straight_insert(&self.name)
    }
}

impl Session {
    /// The rows of `query`, every one found before any is inserted
    /// [X3.135-1986 §8.7 GR1], inserted into `target` straight as
    /// `insert_rows` inserts them, past the notes it keeps of each for
    /// its statement's end, which checks the constraints of `straight`
    /// of each alone (RULES.md D901). The rows it inserted.
    ///
    /// # Errors
    ///
    /// As `insert_rows`.
    pub(in crate::db) fn insert_straight(
        &self,
        cx: StatementContext<'_>,
        (target, straight): (&super::Target, &Straight),
        (query, slots, kept): (&LogicalPlan, Option<Slots>, Option<(&Keeping<'_>, &Params)>),
    ) -> Result<usize> {
        let StatementContext { st, tx } = cx;
        let mut rows = super::inserted::query_rows((self, cx), target, (query, slots, kept))?;
        // [X3.135-1986 §8.7 GR4] R is empty: no row is inserted.
        if rows.is_empty() && !st.catalog.any_triggers() {
            return Ok(0);
        }
        // Rows enough to be a load of their own are one (RULES.md D532),
        // which the general path begins and ends.
        if super::bulk::loads_alone(rows.len()) {
            return self.insert_found(cx, target, rows, None);
        }
        self.settle_cursors(st, &target.name);
        let gone = super::bulk::insert_straight((st, tx), &target.name, &mut rows)?;
        st.end_straight_rows(tx, (&target.name, straight), &gone, &mut rows)?;
        self.note_drawn(target.generating.drawn_identity(), &mut rows)?;
        self.note_generated(target.generating.identity_column(), &mut rows)?;
        Ok(rows.len())
    }
}
