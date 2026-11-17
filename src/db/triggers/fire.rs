//! [ISO/IEC 9075-2:2023 §15.19, §15.20] The triggers a statement's state
//! changes activate, executed: its BEFORE triggers as each update
//! operation has decided its rows, before any is written [§15.8 GR2,
//! §15.11 GR4, §15.14 GR5], each new row then as its BEFORE row triggers
//! left it, its generated columns computed again [§15.11 GR6a, §15.14
//! GR9]; its AFTER triggers once its referential actions are done and its
//! immediate constraints hold [§9.17 GR17b], over every state change it
//! made, its referential actions' too.
//!
//! Triggers execute in the order they were created (Feature T218); a
//! row-level trigger once for each transition, in the order the rows were
//! changed (US048), a statement-level one once for its state change, of
//! no rows too [§15.21 GR4].
//!
//! A statement of a session in a catalog of no trigger has no state
//! changes kept, so its writes do no more than they did.

use std::sync::Arc;

use super::super::generated::Generating;
use super::super::integrity::Effects;
use super::super::{Session, State, StatementContext};
use super::changes::{Event, StateChanges, activates};
use super::run::{Invocation, subject_columns};
use crate::catalog::{ActionTime, Catalog, QualifiedName, TriggerDesc, TriggerLevel, proper_supertables};
use crate::error::Result;
use crate::exec::{HeldRows, Runner};
use crate::storage::{Row, RowId};
use crate::txn::Txn;
use crate::value::Value;

/// A statement's state changes, and the session that executes the
/// triggers they activate, of the catalog of its snapshot.
pub(in crate::db) struct Triggered<'a> {
    session: &'a Session,
    catalog: Arc<dyn Catalog>,
    /// The tables with an AFTER trigger, whose transitions are kept.
    after: Vec<QualifiedName>,
    changes: StateChanges,
}

impl<'a> Triggered<'a> {
    /// Of a statement of `session` in `st`: none if its catalog has no
    /// trigger.
    pub(in crate::db) fn of(session: &'a Session, st: &State) -> Option<Triggered<'a>> {
        if !st.catalog.any_triggers() {
            return None;
        }
        let mut after: Vec<QualifiedName> = Vec::new();
        for t in st.catalog.triggers() {
            if t.time == ActionTime::After && !after.contains(&t.table) {
                after.push(t.table.clone());
            }
        }
        Some(Triggered { session, catalog: Arc::clone(&st.catalog), after, changes: StateChanges::new(st.list_space()) })
    }

    /// [ISO/IEC 9075-2:2023 §15.8 GR3, §15.11 GR5, §15.14 GR8] The
    /// transition `change` of `table`, whose update operation gave values
    /// to `columns`, of an UPDATE, kept if an AFTER trigger may read it;
    /// and, of a typed table, its superrow's in each proper supertable,
    /// of the columns it has [§15.7 GR6, §15.13 GR7, §15.10] (a trigger
    /// reads the first columns of a row, which are its table's).
    ///
    /// # Errors
    ///
    /// Its list's, spilled.
    pub(in crate::db) fn record(&mut self, table: &QualifiedName, columns: &[usize], change: &(RowId, Row, Row)) -> Result<()> {
        let event = event_of(change);
        for of in std::iter::once(table.clone()).chain(proper_supertables(&*self.catalog, table)) {
            if self.after.contains(&of) {
                let names = self.names(&of, columns);
                self.changes.add((&of, event, &names), change.clone())?;
            }
        }
        Ok(())
    }

    /// The names of `columns` of `table`.
    fn names(&self, table: &QualifiedName, columns: &[usize]) -> Vec<String> {
        let desc = self.catalog.table(table);
        columns.iter().filter_map(|&c| desc.and_then(|d| d.columns.get(c)).map(|c| c.name.clone())).collect()
    }

    /// [ISO/IEC 9075-2:2023 §15.20] Executes the AFTER triggers its state
    /// changes activate, in `st` and transaction `tx`.
    ///
    /// # Errors
    ///
    /// As `Session::invoke`.
    pub(in crate::db) fn after(mut self, st: &Arc<State>, tx: &Txn) -> Result<()> {
        let cx = StatementContext { st, tx };
        let catalog = Arc::clone(&self.catalog);
        for t in catalog.triggers() {
            if t.time != ActionTime::After || !self.changes.activate(t) {
                continue;
            }
            let transitions = self.changes.transitions(t)?;
            self.session.execute_trigger(cx, t, transitions)?;
        }
        Ok(())
    }
}

/// The event of a transition: of no old row an insertion, of no new row a
/// deletion.
fn event_of((_, old, new): &(RowId, Row, Row)) -> Event {
    match (old.is_empty(), new.is_empty()) {
        (true, false) | (true, true) => Event::Insert,
        (false, true) => Event::Delete,
        (false, false) => Event::Update,
    }
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §15.21 GR4] Executes trigger `t` for the state
    /// change of `transitions`: a row-level trigger for each, the values
    /// of its transition variables theirs; a statement-level one once.
    /// Its transition tables are their old and new rows. The new rows as a
    /// BEFORE row trigger left them, in order.
    pub(in crate::db) fn execute_trigger(
        &self,
        cx: StatementContext<'_>,
        t: &TriggerDesc,
        mut transitions: HeldRows<(RowId, Row, Row)>,
    ) -> Result<Vec<Option<Vec<Value>>>> {
        let width = subject_columns(cx.st, t)?.len();
        let (mut old_table, mut new_table) = (None, None);
        if t.names.old_table.is_some() || t.names.new_table.is_some() {
            let (mut old, mut new) = (HeldRows::new(cx.st.list_space()), HeldRows::new(cx.st.list_space()));
            let mut tables = copied(&mut transitions, cx.st)?;
            tables.visit(&mut |(_, o, n)| {
                if !o.is_empty() {
                    old.push(fitted(o.clone(), width))?;
                }
                if !n.is_empty() {
                    new.push(fitted(n.clone(), width))?;
                }
                Ok(())
            })?;
            (old_table, new_table) = (t.names.old_table.as_ref().map(|_| old), t.names.new_table.as_ref().map(|_| new));
        }
        if t.level == TriggerLevel::Statement {
            let values = vec![Value::Null; width.saturating_mul(2)];
            return Ok(vec![self.invoke(cx, t, Invocation { values, old_table, new_table })?]);
        }
        let mut news = Vec::new();
        for change in transitions {
            let (_, old, new) = change?;
            let values = [fitted(old, width), fitted(new, width)].concat();
            news.push(self.invoke(cx, t, Invocation { values, old_table: old_table.take(), new_table: new_table.take() })?);
        }
        Ok(news)
    }
}

/// A copy of `rows`, held in `st`'s space.
fn copied(rows: &mut HeldRows<(RowId, Row, Row)>, st: &State) -> Result<HeldRows<(RowId, Row, Row)>> {
    let mut copy = HeldRows::new(st.list_space());
    rows.visit(&mut |change| copy.push(change.clone()))?;
    Ok(copy)
}

/// `row`, its first `width` columns, those the trigger sees, or the null
/// value for each if it is empty.
fn fitted(mut row: Row, width: usize) -> Row {
    if row.is_empty() {
        return vec![Value::Null; width];
    }
    row.truncate(width);
    row
}

impl Effects<'_> {
    /// [ISO/IEC 9075-2:2023 §15.11 GR4] The BEFORE triggers of inserting
    /// `rows` into `table`, in `cx`, executed: the rows as they left them.
    ///
    /// # Errors
    ///
    /// As `Session::invoke`.
    pub(in crate::db) fn before_insert(
        &mut self,
        cx: StatementContext<'_>,
        table: &QualifiedName,
        rows: HeldRows<Row>,
    ) -> Result<HeldRows<Row>> {
        if self.triggered.is_none() {
            return Ok(rows);
        }
        let mut changes = HeldRows::new(cx.st.list_space());
        for row in rows {
            changes.push((0, Row::new(), row?))?;
        }
        let mut out = HeldRows::new(cx.st.list_space());
        for change in self.before(cx, (table, Event::Insert, &[]), changes)? {
            out.push(change?.2)?;
        }
        Ok(out)
    }

    /// [ISO/IEC 9075-2:2023 §15.14 GR5] The BEFORE triggers of the changes
    /// `changes` to `table`, whose update operation gives values to
    /// `columns`, in `cx`, executed: the changes as they left them.
    ///
    /// # Errors
    ///
    /// As `Session::invoke`.
    pub(in crate::db) fn before_update(
        &mut self,
        cx: StatementContext<'_>,
        (table, columns): (&QualifiedName, &[usize]),
        changes: HeldRows<(RowId, Row, Row)>,
    ) -> Result<HeldRows<(RowId, Row, Row)>> {
        let Some(t) = &self.triggered else { return Ok(changes) };
        let names = t.names(table, columns);
        self.before(cx, (table, Event::Update, &names), changes)
    }

    /// [ISO/IEC 9075-2:2023 §15.8 GR2] The BEFORE triggers of deleting
    /// `rows` of `table`, in `cx`, executed.
    ///
    /// # Errors
    ///
    /// As `Session::invoke`.
    pub(in crate::db) fn before_delete(
        &mut self,
        cx: StatementContext<'_>,
        table: &QualifiedName,
        rows: &mut HeldRows<(RowId, Row)>,
    ) -> Result<()> {
        if self.triggered.is_none() {
            return Ok(());
        }
        let mut changes = HeldRows::new(cx.st.list_space());
        rows.visit(&mut |(id, old)| changes.push((*id, old.clone(), Row::new())))?;
        self.before(cx, (table, Event::Delete, &[]), changes).map(drop)
    }

    /// [ISO/IEC 9075-2:2023 §15.14 GR5] `before_update`, of the one change
    /// of row `id` of `table` from `old` to `new`: the new row as the
    /// triggers left it.
    ///
    /// # Errors
    ///
    /// As `Session::invoke`.
    pub(in crate::db) fn before_updated_row(
        &mut self,
        cx: StatementContext<'_>,
        of: (&QualifiedName, &[usize]),
        (id, old, new): (RowId, &Row, Row),
    ) -> Result<Row> {
        if self.triggered.is_none() {
            return Ok(new);
        }
        let mut one = HeldRows::new(cx.st.list_space());
        one.push((id, old.clone(), new))?;
        let mut changed = self.before_update(cx, of, one)?;
        Ok(changed.get(0)?.map(|(_, _, altered)| altered).unwrap_or_default())
    }

    /// [ISO/IEC 9075-2:2023 §15.8 GR2] `before_delete`, of the one row `id`
    /// of `table`, `old`.
    ///
    /// # Errors
    ///
    /// As `Session::invoke`.
    pub(in crate::db) fn before_deleted_row(
        &mut self,
        cx: StatementContext<'_>,
        table: &QualifiedName,
        (id, old): (RowId, &Row),
    ) -> Result<()> {
        if self.triggered.is_none() {
            return Ok(());
        }
        let mut one = HeldRows::new(cx.st.list_space());
        one.push((id, old.clone()))?;
        self.before_delete(cx, table, &mut one)
    }

    /// [ISO/IEC 9075-2:2023 §15.19] The state change of `of` arisen, the
    /// BEFORE triggers it activates executed for `changes`, its
    /// transitions: they, as the triggers left their new rows, each one's
    /// generated columns computed again.
    fn before(
        &mut self,
        cx: StatementContext<'_>,
        of: (&QualifiedName, Event, &[String]),
        mut changes: HeldRows<(RowId, Row, Row)>,
    ) -> Result<HeldRows<(RowId, Row, Row)>> {
        let Some(t) = &mut self.triggered else { return Ok(changes) };
        let (session, catalog) = (t.session, Arc::clone(&t.catalog));
        // [§15.7 GR6, §15.13 GR7] the state changes of the proper
        // supertables of a typed table, of the columns each has.
        let mut tables = vec![(of.0.clone(), of.2.to_vec())];
        for s in proper_supertables(&*catalog, of.0) {
            let has = |n: &String| catalog.table(&s).is_some_and(|d| d.column(n).is_some());
            let names = of.2.iter().filter(|n| has(n)).cloned().collect();
            tables.push((s, names));
        }
        for (s, names) in &tables {
            t.changes.arise((s, of.1, names));
        }
        let activated = |tr: &TriggerDesc| tables.iter().any(|(s, names)| activates(tr, (s, of.1, names)));
        let mut altered = false;
        for trigger in catalog.triggers().iter().filter(|tr| tr.time == ActionTime::Before && activated(tr)) {
            if trigger.level == TriggerLevel::Statement {
                if !t.changes.executed(trigger) {
                    session.execute_trigger(cx, trigger, HeldRows::new(cx.st.list_space()))?;
                }
                continue;
            }
            let news = session.execute_trigger(cx, trigger, copied(&mut changes, cx.st)?)?;
            changes = altered_rows(cx.st, changes, news)?;
            altered = true;
        }
        if altered { regenerated(cx, of.0, changes) } else { Ok(changes) }
    }
}

/// `changes`, each new row's first columns those `news` gives in their
/// place, if any: those the trigger sees.
fn altered_rows(st: &State, changes: HeldRows<(RowId, Row, Row)>, news: Vec<Option<Vec<Value>>>) -> Result<HeldRows<(RowId, Row, Row)>> {
    let mut out = HeldRows::new(st.list_space());
    for (change, new) in changes.into_iter().zip(news.into_iter().chain(std::iter::repeat(None))) {
        let (id, old, was) = change?;
        let row = match new {
            Some(seen) if !was.is_empty() => {
                let mut row = was;
                row.splice(..seen.len().min(row.len()), seen);
                row
            }
            Some(_) | None => was,
        };
        out.push((id, old, row))?;
    }
    Ok(out)
}

/// [ISO/IEC 9075-2:2023 §15.11 GR6a, §15.14 GR9] `changes` to `table`,
/// each new row's generated columns computed again.
fn regenerated(
    cx: StatementContext<'_>,
    table: &QualifiedName,
    changes: HeldRows<(RowId, Row, Row)>,
) -> Result<HeldRows<(RowId, Row, Row)>> {
    let Some(desc) = cx.st.catalog.table(table) else { return Ok(changes) };
    let generating = Generating::of(cx.st, desc, false)?;
    if generating.columns().next().is_none() {
        return Ok(changes);
    }
    let runner = Runner::new(cx.st, cx.tx.id);
    let mut out = HeldRows::new(cx.st.list_space());
    for change in changes {
        let (id, old, mut new) = change?;
        if !new.is_empty() {
            generating.give(cx.st, &mut new, &runner)?;
        }
        out.push((id, old, new))?;
    }
    Ok(out)
}
