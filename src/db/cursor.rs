//! A session's cursors: declaring their use, opening them [X3.135-1986 §8.8],
//! FETCH [X3.135-1986 §8.6], CLOSE [X3.135-1986 §8.1], the cursor position
//! used by positioned UPDATE and DELETE [X3.135-1986 §8.4, §8.11], and
//! single-row SELECT [X3.135-1986 §8.10]; of a dynamic cursor too [ISO/IEC
//! 9075:1992 §17.12-§17.18] (`db/dynamic.rs`).

use std::sync::{Arc, Weak};

use super::dynamic::Name;
use super::prepared::Keeping;
use super::routines::Hosted;
use super::write::Written;
use super::{
    Completion, CursorRows, DeclaredCursor, Kept, OpenCursor, Session, State, StatementContext, Stream, api, assign_targets, candidate,
    check_option, check_targets, cursor_error, plan_heard, rule, set_columns,
};
use crate::ast::{CursorSpecification, FetchOrientation, ParameterSpecification, QuerySpecification};
use crate::binder::{self, OutColumn, bind_select};
use crate::catalog::QualifiedName;
use crate::error::{At, Code, Result, fail};
use crate::exec::{Db, execute};
use crate::params::Params;
use crate::parser::Parser;
use crate::settings::FUNCTION_CALLS;
use crate::storage::{Row, RowId};
use crate::sync::lock_builtin;
use crate::txn::Txn;
use crate::value::datetime::show_in;

/// The columns of a cursor and the row it is on, if it is on one.
type Fetched = Option<(Vec<OutColumn>, Row)>;

/// Rows a single-row SELECT reads: enough to tell one row from more than one
/// [X3.135-1986 §8.10 GR2].
const CARDINALITY_PROBE_ROWS: usize = 2;

mod ordered;
mod updatability;

use updatability::{Wanted, check_object_columns, nameless_read_only, open_updatable, wanted};
pub(super) use updatability::{declared_updatable, movement};

impl Session {
    /// The row an updatable cursor is on. [X3.135-1986 §8.4 SR2, SR3, SR4, GR1]
    /// and the same-numbered rules of [X3.135-1986 §8.11]: the cursor is
    /// declared, is not read-only, has this table in its FROM, and is
    /// positioned on a row.
    ///
    /// The cursor's own state is used, as recorded at OPEN, rather than its
    /// text: the text may name parameters of the procedure that opened it,
    /// which this statement need not declare [X3.135-1986 §8.3 SR2, §8.8 GR3a].
    pub(super) fn positioned_row(&self, cx: StatementContext<'_>, name: &Name, w: &Written, clause: &str) -> Result<RowId> {
        let (named, table) = (&w.bound.named, &w.bound.table);
        let inner = self.inner.borrow();
        let c = inner.cursors.get(name).ok_or_else(|| rule(format!("[X3.135-1986 {clause} SR2] no cursor {name} is declared")))?;
        let read_only = || rule(format!("[X3.135-1986 {clause} SR3, §8.3 SR4] cursor {name} is read-only"));
        let not_positioned = || cursor_error(&name.name, format!("[X3.135-1986 {clause} GR1] cursor {name} is not positioned on a row"));
        match &c.open {
            Some(OpenCursor { rows: CursorRows::Table(_) | CursorRows::Kept(_), .. }) => Err(read_only()),
            Some(OpenCursor { rows: CursorRows::Updatable(u), .. }) => {
                if *named != u.named {
                    return Err(rule(format!("[X3.135-1986 {clause} SR4] {named} is not the table of cursor {name} ({})", u.named)));
                }
                check_object_columns(u, w, &name.name)?;
                let storage = cx.st.storage_of(table)?;
                // The row of the table `w` was bound for: the cursor's
                // current row's (`Session::positioned_member`).
                let live = match u.current() {
                    Some((of, id)) if of == table => storage.get(cx.tx.id, id)?.map(|_| id),
                    Some(_) | None => None,
                };
                match live {
                    Some(id) => Ok(id),
                    None => not_positioned(),
                }
            }
            None => {
                if c.updatable == Some(false) {
                    Err(read_only())
                } else {
                    not_positioned()
                }
            }
        }
    }

    /// [ISO/IEC 9075-2:2023 §14.8 SR9, §14.13 SR9] Of a positioned UPDATE
    /// or DELETE of the typed table `table` through the updatable cursor
    /// `name`, whose `<target table>` is ONLY if `only`: the proper subtable
    /// whose row the cursor is on, of which it is bound (`bind_member_write`),
    /// if it is on one [§15.6 GR9, §15.7 GR9].
    ///
    /// # Errors
    ///
    /// SQLCODE -900, SQLSTATE 42000, if the target is ONLY and the cursor's
    /// table reference is not, or the other way about.
    pub(super) fn positioned_member(
        &self,
        st: &State,
        (name, table, only): (&Name, &QualifiedName, bool),
    ) -> Result<Option<QualifiedName>> {
        let inner = self.inner.borrow();
        let Some(DeclaredCursor { open: Some(OpenCursor { rows: CursorRows::Updatable(u), .. }), .. }) = inner.cursors.get(name) else {
            return Ok(None);
        };
        if u.table != *table {
            return Ok(None);
        }
        if st.catalog.table(table).is_some_and(|d| d.typed.is_some()) && u.only != only {
            let target = if only { "ONLY" } else { "not ONLY" };
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §14.8 SR9, §14.13 SR9] the target table {table} is {target}, as the table reference of cursor {name} is not"
            )));
        }
        Ok(u.current().filter(|(of, _)| *of != table).map(|(of, _)| of.clone()))
    }

    pub(super) fn parse_cursor(st: &State, sql: &str, params: &Params) -> Result<CursorSpecification> {
        params.parser(sql, st.settings.is_on(FUNCTION_CALLS), &st.catalog_name())?.cursor_specification()
    }

    /// The rows of a read-only cursor's specification, after taking
    /// shared locks on the tables it reads [X3.135-1986 §8.8 GR3b]: of a
    /// prepared statement, from the plans it keeps (`keeping`, RULES.md
    /// D45), its physical plan filled if its paths follow no value, else
    /// its bound plan filled and planned.
    pub(super) fn open_rows(
        &self,
        st: &Arc<State>,
        tx: &Txn,
        spec: &CursorSpecification,
        (params, keeping): (&Params, Option<&Keeping<'_>>),
    ) -> Result<(Vec<OutColumn>, Stream)> {
        // [ISO/IEC 9075-2:2023 §7.6 GR5] a delta table's statement run
        // first, and the query bound over its rows, all read now, so that
        // an error of the query undoes the statement with it (RULES.md
        // D174).
        if self.run_delta(st, tx, spec, params)? {
            let (columns, logical) = self.cursor_bound(st, spec, params, None)?;
            return Ok((columns, Stream::planned(st, tx, logical)?.read_now()?));
        }
        if let Some((columns, physical, reads, slots)) = keeping.map(|k| k.physical(params)).transpose()?.flatten() {
            let db = slots.map_or_else(|| Db::of(st), |slots| Db::slotted(st, slots));
            return Ok((columns, Stream::kept(&db, tx, physical, reads)?));
        }
        let (columns, logical) = self.cursor_bound(st, spec, params, keeping)?;
        Ok((columns, Stream::planned(st, tx, logical)?))
    }

    /// [ISO/IEC 9075:1992 §11.8 GR8] The rows changed through each cursor
    /// still open: its name, table and row.
    pub(super) fn cursor_changes(&self) -> Vec<(String, QualifiedName, RowId)> {
        let inner = self.inner.borrow();
        let mut rows = Vec::new();
        for (name, c) in &inner.cursors {
            if let Some(OpenCursor { rows: CursorRows::Updatable(u), .. }) = c.open.as_ref() {
                rows.extend(u.changed.iter().map(|(table, id)| (name.name.clone(), table.clone(), *id)));
            }
        }
        rows
    }

    /// Notes row `id` changed through cursor `name` [ISO/IEC 9075:1992
    /// §11.8 GR8].
    fn changed_through(&self, name: &Name, (table, id): (&QualifiedName, RowId)) {
        if let Some(DeclaredCursor { open: Some(OpenCursor { rows: CursorRows::Updatable(u), .. }), .. }) =
            self.inner.borrow_mut().cursors.get_mut(name)
        {
            u.changed.push((table.clone(), id));
        }
    }

    /// Whether an open cursor of the session reads `table`.
    pub(super) fn cursor_reads(&self, table: &QualifiedName) -> bool {
        let inner = self.inner.borrow();
        let named = inner.cursors.values().filter_map(|c| c.open.as_ref()).any(|open| match &open.rows {
            CursorRows::Table(rows) => rows.reads(table),
            CursorRows::Kept(kept) => kept.stream.reads(table),
            CursorRows::Updatable(u) => u.reads(table),
        });
        named || inner.anonymous.iter().filter_map(Weak::upgrade).any(|rows| lock_builtin(&rows).as_ref().is_some_and(|r| r.reads(table)))
    }

    /// Whether a cursor of the session is open: a transaction reading at a
    /// snapshot for each statement keeps the one it has meanwhile, which
    /// the cursor's rows are read at (`Txn::begin_statement`).
    pub(super) fn cursors_open(&self) -> bool {
        let inner = self.inner.borrow();
        inner.cursors.values().any(|c| c.open.is_some())
            || inner.anonymous.iter().filter_map(Weak::upgrade).any(|rows| lock_builtin(&rows).is_some())
    }

    /// [X3.135-1986 §8.8 GR3b] Before the transaction changes `table`,
    /// every open read-only cursor that reads it reads the rest of its rows
    /// (`Stream::settle`).
    pub(super) fn settle_cursors(&self, st: &State, table: &QualifiedName) {
        for written in st.written_by(table) {
            self.settle_cursors_of(&written);
        }
    }

    /// As `settle_cursors`, for `table` alone.
    fn settle_cursors_of(&self, table: &QualifiedName) {
        let mut inner = self.inner.borrow_mut();
        for c in inner.cursors.values_mut() {
            match c.open.as_mut().map(|open| &mut open.rows) {
                Some(CursorRows::Table(rows)) => rows.settle(table),
                Some(CursorRows::Kept(kept)) => kept.stream.settle(table),
                Some(CursorRows::Updatable(_)) | None => {}
            }
        }
        inner.anonymous.retain(|rows| rows.strong_count() > 0);
        for rows in inner.anonymous.iter().filter_map(Weak::upgrade) {
            if let Some(rows) = lock_builtin(&rows).as_mut() {
                rows.settle(table);
            }
        }
    }

    /// [X3.135-1986 §8.8; ISO/IEC 9075:1992 §13.2] OPEN; of a dynamic
    /// cursor, OPEN with no `<using clause>` [§17.14].
    pub(super) fn open(&self, cx: StatementContext<'_>, name: &Name, params: &Params) -> Result<Completion> {
        let (text, flags, dynamic) = {
            let inner = self.inner.borrow();
            let c = inner.cursors.get(name).ok_or_else(|| rule(format!("[X3.135-1986 §8.8 SR1] no cursor {name} is declared")))?;
            (c.specification.clone(), (c.insensitive, c.scroll), c.statement.is_some())
        };
        if dynamic {
            return self.open_dynamic(cx, name, None, params);
        }
        self.open_text(cx, name, (&text, flags, None, false), params)
    }

    /// OPEN of the cursor `name`, of the `<cursor specification>` `text`,
    /// INSENSITIVE and SCROLL as `flags` say, with the parameters `params`:
    /// of a prepared statement's, a dynamic cursor's, from the
    /// specification and plan it keeps (`keeping`, RULES.md D45). A cursor
    /// no statement can name and through which no change can be seen,
    /// `nameless`, is read only unless its text says FOR UPDATE
    /// (`nameless_read_only`).
    pub(super) fn open_text(
        &self,
        cx: StatementContext<'_>,
        name: &Name,
        (text, flags, keeping, nameless): (&str, (bool, bool), Option<&Keeping<'_>>, bool),
        params: &Params,
    ) -> Result<Completion> {
        let StatementContext { st, tx } = cx;
        if self.inner.borrow().cursors.get(name).is_some_and(|c| c.open.is_some()) {
            return cursor_error(&name.name, format!("[X3.135-1986 §8.8 GR1] cursor {name} is already open"));
        }
        // [X3.135-1986 §8.8 GR3a] a copy of S with each parameter replaced by its value.
        let spec = Self::cursor_specification(st, text, params, keeping)?;
        let wanted = nameless_read_only(wanted(&spec, flags)?, nameless);
        let updatable = match (wanted, binder::updatable_in(&spec, &*st.catalog, &self.default_schema())) {
            (Wanted::Written | Wanted::Implicit, Some(named)) => {
                self.updatable_plan(st, &spec, params, keeping)?.map(|updating| (named, updating))
            }
            (Wanted::ReadOnly, _) | (Wanted::Written | Wanted::Implicit, None) => None,
        };
        if updatable.is_none() && wanted == Wanted::Written {
            return Err(rule(format!("[ISO/IEC 9075:1992 §13.1 SR12] cursor {name} is FOR UPDATE, but its table is not updatable")));
        }
        let is_updatable = updatable.is_some();
        let open = if let Some(found) = updatable {
            open_updatable(st, tx, &spec, found)?
        } else {
            // [X3.135-1986 §8.8 GR3b] "that table ... is effectively created".
            let (columns, rows) = self.open_rows(st, tx, &spec, (params, keeping))?;
            OpenCursor { columns, rows: if flags.1 { CursorRows::Kept(Kept::new(rows, st)) } else { CursorRows::Table(rows) } }
        };
        // [X3.135-1986 §8.8 GR3c] open, positioned before the first row.
        let mut inner = self.inner.borrow_mut();
        let c = inner.cursors.get_mut(name).ok_or_else(|| rule(format!("[X3.135-1986 §8.8 SR1] no cursor {name} is declared")))?;
        c.updatable = Some(is_updatable);
        c.open = Some(open);
        Ok(Completion::Success)
    }

    /// [X3.135-1986 §8.6; ISO/IEC 9075:1992 §13.3] FETCH, moving the cursor
    /// as `orientation` says, into `targets`.
    pub(super) fn fetch(
        &self,
        cx: StatementContext<'_>,
        name: &Name,
        (orientation, targets): (&FetchOrientation, &[ParameterSpecification]),
        params: &mut Params,
    ) -> Result<Completion> {
        let ((), fetched) =
            self.fetch_row(cx, (name, orientation), params, |columns, p| check_targets(columns, targets, p, "§8.6 SR2", "§8.6 SR3"))?;
        match fetched {
            // [X3.135-1986 §8.6 GR2] positioned after the last row; SQLCODE 100.
            None => Ok(Completion::NoData),
            Some((_, values)) => assign_targets(&values, targets, params, "X3.135-1986 §8.6 GR8", "X3.135-1986 §8.6 GR10"),
        }
    }

    /// [X3.135-1986 §8.6; ISO/IEC 9075:1992 §13.3] The cursor `name` moved as
    /// `orientation` says, once `check` has accepted its columns: what
    /// `check` gave, and the columns and the row it is on, shown in the
    /// session's time zone, if it is on one.
    pub(super) fn fetch_row<T>(
        &self,
        cx: StatementContext<'_>,
        (name, orientation): (&Name, &FetchOrientation),
        params: &Params,
        check: impl FnOnce(&[OutColumn], &Params) -> Result<T>,
    ) -> Result<(T, Fetched)> {
        let (to, columns) = {
            let inner = self.inner.borrow();
            let c = inner.cursors.get(name).ok_or_else(|| rule(format!("[X3.135-1986 §8.6 SR1] no cursor {name} is declared")))?;
            let to = movement(orientation, c.scroll, &|v| self.value_now(v, params, None))?;
            let Some(open) = c.open.as_ref() else {
                return cursor_error(&name.name, format!("[X3.135-1986 §8.6 GR1] cursor {name} is not open"));
            };
            (to, open.columns.clone())
        };
        // Run with no borrow of the session held: it may read the session.
        let checked = check(&columns, params)?;
        let row = {
            let mut inner = self.inner.borrow_mut();
            let open = inner.cursors.get_mut(name).and_then(|c| c.open.as_mut());
            let open = open.ok_or_else(|| api(format!("[DESIGN] cursor {name} closed as it was fetched from")))?;
            match &mut open.rows {
                // [§13.3 SR3] Not SCROLL: NEXT.
                CursorRows::Table(rows) => rows.next()?,
                CursorRows::Kept(kept) => kept.fetch(to)?,
                CursorRows::Updatable(u) => u.fetch(cx.tx.id, to)?,
            }
        };
        let zone = self.now.get().zone;
        Ok((
            checked,
            row.map(|mut values| {
                show_in(&mut values, zone);
                (columns, values)
            }),
        ))
    }

    /// [X3.135-1986 §8.1] CLOSE
    pub(super) fn close(&self, name: &Name) -> Result<Completion> {
        let mut inner = self.inner.borrow_mut();
        let c = inner.cursors.get_mut(name).ok_or_else(|| rule(format!("[X3.135-1986 §8.1 SR1] no cursor {name} is declared")))?;
        if c.open.take().is_none() {
            return cursor_error(&name.name, format!("[X3.135-1986 §8.1 GR1] cursor {name} is not open"));
        }
        Ok(Completion::Success)
    }

    /// The cursor a positioned statement's `<cursor name>` `name` names:
    /// the one a prepared statement running found for it [ISO/IEC
    /// 9075:1992 §17.6 GR2], an extended cursor's too, or the one of the
    /// name.
    pub(super) fn positioned_key(&self, name: &str) -> Name {
        self.inner.borrow().dynamic.positioned.clone().filter(|k| k.name == name).unwrap_or_else(|| Name::plain(name))
    }

    /// [X3.135-1986 §8.10] SELECT ... INTO; of a statement of a program
    /// kept, its delta table's statement by the plans `keeping` keeps of
    /// it (RULES.md D792).
    pub(super) fn select_into(
        &self,
        cx: StatementContext<'_>,
        (query, targets): (&QuerySpecification, &[ParameterSpecification]),
        params: &mut Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<Completion> {
        // [ISO/IEC 9075-2:2023 §7.6 GR5] a delta table's statement run
        // first (RULES.md D174).
        if let [crate::ast::TableReference::Delta(d)] = query.table_expression.from.as_slice() {
            self.run_delta_table(cx.st, cx.tx, d, (params, keeping))?;
        }
        let bound = self.bound(cx.st, params, |b| bind_select(b, query))?;
        check_targets(&bound.columns, targets, params, "§8.10 SR3", "§8.10 SR4")?;
        let (optimized, _, physical) = plan_heard(cx.st, bound.plan, Some(cx.tx.stop.warnings()));
        for (t, needed) in optimized.columns_read() {
            cx.st.read_lock_of(cx.tx, (&t, needed))?;
        }
        let rows: Vec<Row> = execute(physical, cx.st, cx.tx.id).take(CARDINALITY_PROBE_ROWS).collect::<Result<_>>()?;
        match rows.as_slice() {
            // [X3.135-1986 §8.10 GR2] no row; the targets are not assigned.
            [] => Ok(Completion::NoData),
            [row] => {
                let mut row = row.clone();
                show_in(&mut row, self.now.get().zone);
                assign_targets(&row, targets, params, "X3.135-1986 §8.10 GR8", "X3.135-1986 §8.10 GR9")
            }
            [_, _, ..] => fail(Code::Cardinality, "[X3.135-1986 §8.10 GR2] the result has more than one row"),
        }
    }

    /// [X3.135-1986 §8.11] the positioned UPDATE bound as `w`, through
    /// cursor `cursor`.
    pub(super) fn update_positioned(&self, cx: StatementContext<'_>, (cursor, at): (&Name, usize), w: &Written) -> Result<Completion> {
        let StatementContext { st, tx } = cx;
        let id = self.positioned_row(cx, cursor, w, "§8.11").or_at(at)?;
        let table = &w.bound.table;
        let old = st.storage_of(table)?.get(tx.id, id)?.ok_or_else(|| api(format!("[DESIGN] row {id} of {table} vanished")))?;
        for t in w.bound.set_tables() {
            st.read_lock(tx, &t)?;
        }
        let runner = Hosted::in_db(Some(self), cx, &w.db(cx.st));
        let new = candidate(w, &old, ("ISO/IEC 9075:1992 §13.9 GR9", &runner))?;
        let mut effects = self.effects(st);
        // [ISO/IEC 9075-2:2023 §15.14 GR5] its BEFORE triggers first.
        let new = effects.before_updated_row(cx, (table, &set_columns(w)), (id, &old, new))?;
        // [EXTENSION] a row its key takes out of its partition, moved to the
        // one it is routed to (RULES.md D155, D156).
        if st.moves((&w.bound.named, table), &new) {
            let mut again = std::collections::HashMap::new();
            super::partitions::move_row(cx, (w, runner.runner()), (id, &new), (&mut effects, &mut again))?;
            check_option(&w.bound.checks, [&new], ("X3.135-1986 §8.11 GR5", &runner))?;
        } else {
            st.update_row(tx, table, id, &new)?;
            check_option(&w.bound.checks, [&new], ("X3.135-1986 §8.11 GR5", &runner))?;
            effects.updated(table, id, (old, &new), &set_columns(w))?;
        }
        st.end_statement(tx, effects)?;
        self.changed_through(cursor, (table, id));
        Ok(Completion::Success)
    }

    /// [X3.135-1986 §8.4] the positioned DELETE bound as `w`, through
    /// cursor `cursor`.
    pub(super) fn delete_positioned(&self, cx: StatementContext<'_>, (cursor, at): (&Name, usize), w: &Written) -> Result<Completion> {
        let StatementContext { st, tx } = cx;
        let id = self.positioned_row(cx, cursor, w, "§8.4").or_at(at)?;
        let table = &w.bound.table;
        let old = st.storage_of(table)?.get(tx.id, id)?.ok_or_else(|| api(format!("[DESIGN] row {id} of {table} vanished")))?;
        let mut effects = self.effects(st);
        // [ISO/IEC 9075-2:2023 §15.8 GR2] its BEFORE triggers first.
        effects.before_deleted_row(cx, table, (id, &old))?;
        st.delete_row(tx, table, id)?;
        effects.deleted(table, id, old)?;
        st.end_statement(tx, effects)?;
        // [X3.135-1986 §4.12] the cursor is now before the next row.
        if let Some(DeclaredCursor { open: Some(OpenCursor { rows: CursorRows::Updatable(u), .. }), .. }) =
            self.inner.borrow_mut().cursors.get_mut(cursor)
        {
            u.deleted();
            u.changed.push((table.clone(), id));
        }
        Ok(Completion::Success)
    }

    /// [X3.135-1986 §8.3] `<declare cursor>`. In SQL-86 cursors belong to the
    /// `<module>` [X3.135-1986 §7.1]; here, to the session.
    ///
    /// # Errors
    ///
    /// The declaration does not parse, or the cursor is already declared.
    pub fn declare_cursor(&self, sql: &str) -> Result<()> {
        let call = self.enter()?;
        let d = Parser::new(sql)?.in_catalog(&call.state().catalog_name()).declare_cursor()?;
        let mut inner = self.inner.borrow_mut();
        let name = Name::plain(&d.name);
        if inner.cursors.contains_key(&name) {
            return Err(rule(format!("[X3.135-1986 §8.3 SR1] cursor {} is already declared", d.name)));
        }
        // [ISO/IEC 9075:1992 §17.12] of a <statement name>: a dynamic cursor.
        let statement = super::dynamic::declared_statement(&d.specification).map(|s| Name::plain(&s));
        // Known now only if the text parses without the opening procedure's
        // parameters; otherwise it is parsed, and any error reported, at OPEN.
        let parsed = Parser::for_stored(&d.specification).and_then(|mut p| p.cursor_specification()).ok();
        let updatable = parsed.map(|c| declared_updatable(&c, (d.insensitive, d.scroll))).transpose()?;
        let (insensitive, scroll) = (d.insensitive, d.scroll);
        inner
            .cursors
            .insert(name, DeclaredCursor { specification: d.specification, insensitive, scroll, updatable, open: None, statement });
        Ok(())
    }
}
