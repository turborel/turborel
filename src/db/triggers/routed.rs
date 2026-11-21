//! [ISO/IEC 9075-2:2023 §15.10, §15.13, §15.15, §15.16] Which INSTEAD OF
//! trigger takes a change of a view. A view named by a statement that has
//! an INSTEAD OF trigger for its event takes it. One that has none is
//! changed through the table its query names, if it is updatable, so the
//! first view down that chain with such a trigger takes it, the view's
//! rows being those of the table under it [§15.15 GR3c, §15.12 GR2b]. A
//! view WITH CHECK OPTION reads no view with such a trigger [§11.32 SR19],
//! so no row taken by one is checked.
//!
//! Down an updatable chain each column of a view is a column of the table
//! under it, so a row of the base table at its foot is a row of each view
//! of the chain. The change is decided over those rows (`base`), and each
//! row given to the trigger is the base row's columns its view has. A
//! positioned UPDATE or DELETE reads through an updatable cursor, so it is
//! always so.

use super::super::assignment::Assigned;
use super::super::dynamic::Name;
use super::super::write::{Written, object_rows};
use super::super::{Completion, CursorRows, DeclaredCursor, OpenCursor, Session, State, StatementContext, api};
use super::changes::Event;
use crate::catalog::{QualifiedName, TriggerDesc, TriggerEvent};
use crate::error::{At, Result};
use crate::exec::{HeldRows, Runner};
use crate::storage::{Row, RowId};
use crate::value::Value;

/// [ISO/IEC 9075-2:2023 §15.5 GR3] The rule a SET value's store
/// assignment cites, of a change an INSTEAD OF trigger takes.
const SET_RULE: &str = "ISO/IEC 9075-2:2023 §15.5 GR3";

/// The INSTEAD OF trigger that takes a change of a view, and how the rows
/// of its view are made.
#[derive(Debug, Clone)]
pub(in crate::db) struct Instead {
    /// The trigger.
    pub(in crate::db) trigger: TriggerDesc,
    /// The view it is on: the one named, or one under it.
    pub(in crate::db) view: QualifiedName,
    /// The column of the view each column of the one named is.
    pub(in crate::db) from_named: Vec<usize>,
    /// The base table, and the column of it each column of the view is,
    /// if the view named is updatable down to a base table.
    pub(in crate::db) base: Option<(QualifiedName, Vec<usize>)>,
    /// How many columns the view has.
    pub(in crate::db) width: usize,
}

impl Instead {
    /// The row of its view that the base table's row `row` is.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its view is not updatable down to a base table, or
    /// `row` is not one of that table's.
    pub(in crate::db) fn of_base(&self, row: &Row) -> Result<Row> {
        let (_, base) = self.base.as_ref().ok_or_else(|| api(format!("[DESIGN] view {} is read through no base table", self.view)))?;
        base.iter()
            .map(|&c| row.get(c).cloned().ok_or_else(|| api(format!("[DESIGN] a row of view {} has no column {c}", self.view))))
            .collect()
    }

    /// The row of its view that the row `row` of the view named makes:
    /// each of its columns the view named has given that column's value,
    /// each other its value of `defaults`, its view's columns' defaults, as
    /// a column not named in an INSERT [ISO/IEC 9075-2:2023 §15.12 GR2b iii
    /// 1]. Of a view the view named itself, `defaults` may be empty.
    pub(in crate::db) fn of_named(&self, row: Row, defaults: &[Value]) -> Row {
        let mut out: Row = (0..self.width).map(|j| defaults.get(j).cloned().unwrap_or(Value::Null)).collect();
        for (v, &k) in row.into_iter().zip(&self.from_named) {
            if let Some(slot) = out.get_mut(k) {
                *slot = v;
            }
        }
        out
    }
}

/// [ISO/IEC 9075-2:2023 §15.10 GR3, §15.13 GR3, §15.16 GR3] The INSTEAD OF
/// trigger that takes a change of event `event` of `named`: of `named`,
/// if it has one, else of the first view with one down the chain of
/// updatable views under it; none if no view of that chain has one.
pub(in crate::db) fn instead_for(st: &State, named: &QualifiedName, event: Event) -> Option<Instead> {
    if !st.catalog.any_triggers() {
        return None;
    }
    let mut at = named.clone();
    let mut from_named: Vec<usize> = (0..st.catalog.view(named)?.columns.len()).collect();
    loop {
        let view = st.catalog.view(&at)?;
        if let Some(trigger) = instead_trigger(st, &at, event) {
            let (base, width) = (base_columns(st, &at), view.columns.len());
            return Some(Instead { trigger, view: at, from_named, base, width });
        }
        let (under, columns) = view.updates.as_ref()?;
        from_named = from_named.iter().filter_map(|&k| columns.get(k).copied()).collect();
        at = under.clone();
    }
}

/// [ISO/IEC 9075-2:2023 §15.10, §15.13, §15.16] The INSTEAD OF trigger of
/// view `name` for `event`, if it has one [§11.49 SR5a: one at most].
pub(in crate::db) fn instead_trigger(st: &State, name: &QualifiedName, event: Event) -> Option<TriggerDesc> {
    let of = |t: &&TriggerDesc| match (&t.event, event) {
        (TriggerEvent::Insert, Event::Insert) | (TriggerEvent::Delete, Event::Delete) | (TriggerEvent::Update(_), Event::Update) => true,
        (TriggerEvent::Insert, Event::Delete | Event::Update)
        | (TriggerEvent::Delete, Event::Insert | Event::Update)
        | (TriggerEvent::Update(_), Event::Insert | Event::Delete) => false,
    };
    st.catalog.triggers_on(name).into_iter().filter(|t| t.instead()).find(of).cloned()
}

/// The base table under view `view`, and the column of it each column of
/// the view is, if it is updatable down to one [X3.135-1986 §6.5 GR2].
fn base_columns(st: &State, view: &QualifiedName) -> Option<(QualifiedName, Vec<usize>)> {
    let mut columns: Vec<usize> = (0..st.catalog.view(view)?.columns.len()).collect();
    let mut at = view.clone();
    while let Some(v) = st.catalog.view(&at) {
        let (under, map) = v.updates.as_ref()?;
        columns = columns.iter().map(|&k| map.get(k).copied()).collect::<Option<_>>()?;
        at = under.clone();
    }
    Some((at, columns))
}

/// Whether `name` is a base table, or a view updatable down to one
/// [X3.135-1986 §6.5 GR2].
pub(in crate::db) fn updatable(st: &State, name: &QualifiedName) -> bool {
    st.catalog.view(name).is_none() || base_columns(st, name).is_some()
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §15.10, §15.16] The searched UPDATE or DELETE
    /// bound as `w`, its object rows decided as if written, taken by
    /// `instead` in place of the change: the trigger given each row of its
    /// view the change identifies, before and, of an UPDATE, after its SET
    /// clauses [§15.5]. How many rows it would have changed.
    ///
    /// # Errors
    ///
    /// The reads', a value's and the trigger's.
    pub(in crate::db) fn searched_instead(&self, cx: StatementContext<'_>, w: Written, instead: &Instead) -> Result<usize> {
        let StatementContext { st, tx } = cx;
        let updates = instead.trigger.event != TriggerEvent::Delete;
        let (w, objects) = object_rows(st, tx, w)?;
        let runner = Runner::new(&w.db(st), tx.id);
        let mut transitions = HeldRows::new(st.list_space());
        for object in objects {
            let (id, old) = object?;
            let new = if updates { instead.of_base(&replaced(&w, &old, &runner)?)? } else { Row::new() };
            transitions.push((id, instead.of_base(&old)?, new))?;
        }
        let count = transitions.len();
        self.execute_trigger(cx, &instead.trigger, transitions)?;
        Ok(count)
    }

    /// [ISO/IEC 9075-2:2023 §15.6 GR11b, §15.7 GR17b] The positioned UPDATE
    /// or DELETE bound as `w` through cursor `cursor`, written at `at`,
    /// taken by `instead`: its trigger given the row of its view the
    /// cursor is on, before and, of an UPDATE, after its SET clauses. A
    /// DELETE leaves the cursor before the next row [§15.6 GR13]; an UPDATE
    /// leaves it on its row [§15.7 GR12].
    ///
    /// # Errors
    ///
    /// As `positioned_row`; a value's and the trigger's.
    pub(in crate::db) fn positioned_instead(
        &self,
        cx: StatementContext<'_>,
        (cursor, at): (&Name, usize),
        (w, instead, clause): (&Written, &Instead, &str),
    ) -> Result<Completion> {
        let StatementContext { st, tx } = cx;
        let id = self.positioned_row(cx, cursor, w, clause).or_at(at)?;
        let table = &w.bound.table;
        let old = st.storage_of(table)?.get(tx.id, id)?.ok_or_else(|| api(format!("[DESIGN] row {id} of {table} vanished")))?;
        let deletes = instead.trigger.event == TriggerEvent::Delete;
        if !deletes {
            for t in w.bound.set_tables() {
                st.read_lock(tx, &t)?;
            }
        }
        let new = if deletes { Row::new() } else { instead.of_base(&replaced(w, &old, &Runner::new(&w.db(st), tx.id))?)? };
        let mut transitions: HeldRows<(RowId, Row, Row)> = HeldRows::new(st.list_space());
        transitions.push((id, instead.of_base(&old)?, new))?;
        self.execute_trigger(cx, &instead.trigger, transitions)?;
        if deletes {
            self.left_row(cursor);
        }
        Ok(Completion::Success)
    }

    /// [ISO/IEC 9075-2:2023 §15.6 GR13] Cursor `name`, its row deleted
    /// through it, before the next row.
    fn left_row(&self, name: &Name) {
        if let Some(DeclaredCursor { open: Some(OpenCursor { rows: CursorRows::Updatable(u), .. }), .. }) =
            self.inner.borrow_mut().cursors.get_mut(name)
        {
            u.deleted();
        }
    }
}

/// [ISO/IEC 9075-2:2023 §15.5] The candidate new row of the object row
/// `old` of `w`: its SET values assigned, no generated column computed
/// again, as no base table is written [§15.16 GR3a].
fn replaced(w: &Written, old: &Row, runner: &Runner) -> Result<Row> {
    let assigned = Assigned { desc: &w.bound.desc, set: w.set(), generated: &[] };
    assigned.candidate((old, old), (SET_RULE, runner))
}
