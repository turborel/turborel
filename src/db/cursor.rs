//! A session's cursors: declaring their use, opening them [X3.135-1986 §8.8],
//! FETCH [X3.135-1986 §8.6], CLOSE [X3.135-1986 §8.1], the cursor position used
//! by positioned UPDATE and DELETE [X3.135-1986 §8.4, §8.11], single-row SELECT
//! [X3.135-1986 §8.10] and the \[EXTENSION\] `open_cursor`. Split from db.rs
//! (RULES.md R-SIZE-2).

use std::sync::{Arc, Mutex, Weak};

use super::position::{Move, Position};
use super::{
    Completion, Cursor, CursorRows, DeclaredCursor, Kept, OpenCursor, Session, State, StatementContext, Stream, Updatable, api,
    assign_targets, candidate, check_option, check_targets, cursor_error, plan, rule, set_columns,
};
use crate::ast::{CursorSpecification, FetchOrientation, ParameterSpecification, QuerySpecification, Updatability, ValueSpecification};
use crate::binder::{self, BoundWrite, OutColumn, bind_cursor, bind_select};
use crate::catalog::{QualifiedName, TableDesc};
use crate::error::{At, Code, Result, SqlState, fail, fail_with};
use crate::exec::{Runner, compare_rows, execute, row_entries};
use crate::logical::{Cond, LogicalPlan, SortKey, and_all};
use crate::params::Params;
use crate::physical::Plan;
use crate::storage::{Row, RowId};
use crate::sync::lock_builtin;
use crate::txn::{TxId, Txn};
use crate::value::datetime::show_in;
use crate::value::{DataType, Value};

/// Rows a single-row SELECT reads: enough to tell one row from more than one
/// [X3.135-1986 §8.10 GR2].
const CARDINALITY_PROBE_ROWS: usize = 2;

/// The condition the rows of an updatable cursor's table satisfy, over the
/// rows of the table it reads: the conditions of the filters of `plan`
/// below its select list, its WHERE clause's and its view's. A row must
/// still satisfy it when the cursor reaches it (`db/scroll.rs`).
fn row_condition(plan: &LogicalPlan) -> Option<Cond> {
    let mut conds = Vec::new();
    let mut node = plan;
    loop {
        match node {
            LogicalPlan::Project { input, .. } | LogicalPlan::Sort { input, .. } => node = input,
            LogicalPlan::Filter { cond, input } => {
                conds.push(cond.clone());
                node = input;
            }
            LogicalPlan::Scan { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::OuterJoin { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::DefaultRow { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::System { .. } => break,
        }
    }
    and_all(conds)
}

/// [ISO/IEC 9075:1992 §13.1 SR5, SR6] How a cursor's updatability is
/// given: FOR READ ONLY, written or implicit; FOR UPDATE written; or FOR
/// UPDATE implicit, the cursor then updatable if its table is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wanted {
    ReadOnly,
    Written,
    Implicit,
}

/// [ISO/IEC 9075:1992 §13.1 SR5, SR12] How the cursor of `spec`, declared
/// INSENSITIVE and SCROLL as `flags` say, gives its updatability.
///
/// # Errors
///
/// SQLCODE -103 if it is INSENSITIVE and FOR UPDATE \[SR12\].
fn wanted(spec: &CursorSpecification, (insensitive, scroll): (bool, bool)) -> Result<Wanted> {
    match &spec.updatability {
        Some(Updatability::ReadOnly) => Ok(Wanted::ReadOnly),
        Some(Updatability::Update(_)) => {
            if insensitive {
                return Err(rule("[ISO/IEC 9075:1992 §13.1 SR12] an INSENSITIVE cursor is not FOR UPDATE".to_owned()));
            }
            Ok(Wanted::Written)
        }
        // [SR5a] INSENSITIVE, SCROLL or ORDER BY: READ ONLY.
        None => Ok(if insensitive || scroll || !spec.order_by.is_empty() { Wanted::ReadOnly } else { Wanted::Implicit }),
    }
}

/// [ISO/IEC 9075:1992 §13.1 SR6] Whether the cursor of `spec`, declared as
/// `flags` say, is updatable, as far as its text tells: FOR UPDATE, or an
/// updatable table.
///
/// # Errors
///
/// As `wanted`'s.
pub(super) fn declared_updatable(spec: &CursorSpecification, flags: (bool, bool)) -> Result<bool> {
    Ok(match wanted(spec, flags)? {
        Wanted::ReadOnly => false,
        Wanted::Written => true,
        Wanted::Implicit => binder::updatable(spec).is_some(),
    })
}

/// [ISO/IEC 9075:1992 §13.9 SR4] The columns of its table an updatable
/// cursor's ORDER BY reads: the keys of the Sort atop its `plan`, each a
/// column of the cursor, `projection`'s column of the table.
fn ordered_columns(plan: &LogicalPlan, projection: &[usize]) -> Vec<usize> {
    let LogicalPlan::Sort { keys, .. } = plan else { return Vec::new() };
    keys.iter().filter_map(|k| projection.get(k.column).copied()).collect()
}

/// [ISO/IEC 9075:1992 §13.1 SR13, SR14] The columns of `table` (`desc`) a
/// positioned UPDATE through the cursor may set: those its FOR UPDATE
/// names, or, naming none, every one.
///
/// # Errors
///
/// SQLCODE -103 if it names a column `table` has not \[SR14\].
fn settable(desc: &TableDesc, updatability: Option<&Updatability>) -> Result<Vec<usize>> {
    let Some(Updatability::Update(Some(names))) = updatability else { return Ok((0..desc.columns.len()).collect()) };
    names
        .iter()
        .map(|(name, at)| {
            let refused = || rule(format!("[ISO/IEC 9075:1992 §13.1 SR14] {} has no column {name} to update", desc.name)).or_at(*at);
            desc.columns.iter().position(|c| c.name == *name).ok_or_else(refused)
        })
        .collect()
}

/// The rows an updatable cursor may cover, in its order: those of its
/// `plan`'s table access, sorted, if a Sort is atop it, on the keys it
/// has, each a column of the cursor, `projection`'s column of the table.
fn ordered_ids(plan: Plan, st: &Arc<State>, tx: TxId, projection: &[usize]) -> Result<Vec<RowId>> {
    let Plan::Sort { keys, input, .. } = plan else { return Ok(row_entries(plan, st, tx)?.into_iter().map(|(id, _)| id).collect()) };
    let mut entries = row_entries(*input, st, tx)?;
    let on_table: Vec<SortKey> =
        keys.iter().filter_map(|k| projection.get(k.column).map(|&column| SortKey { column, descending: k.descending })).collect();
    entries.sort_by(|(_, a), (_, b)| compare_rows(&on_table, a, b));
    Ok(entries.into_iter().map(|(id, _)| id).collect())
}

/// [ISO/IEC 9075:1992 §13.3 SR3, SR4, GR2] Where FETCH `orientation` moves
/// a cursor, SCROLL if `scroll`, its J's value read from `params` by
/// `auth`.
///
/// # Errors
///
/// SQLCODE -103 if it is not NEXT and the cursor is not SCROLL \[SR3\], or
/// its J is not exact numeric with scale 0 \[SR4\]; -301 if J is null.
fn movement(orientation: &FetchOrientation, scroll: bool, (auth, params): (&str, &Params)) -> Result<Move> {
    let to = match orientation {
        FetchOrientation::Next => Move::Next,
        FetchOrientation::Prior => Move::Prior,
        FetchOrientation::First => Move::First,
        FetchOrientation::Last => Move::Last,
        FetchOrientation::Absolute(j) => Move::Absolute(how_far(j, auth, params)?),
        FetchOrientation::Relative(j) => Move::Relative(how_far(j, auth, params)?),
    };
    if !scroll && !matches!(to, Move::Next) {
        return Err(rule("[ISO/IEC 9075:1992 §13.3 SR3] the cursor is not SCROLL: it fetches NEXT alone".to_owned()));
    }
    Ok(to)
}

/// [ISO/IEC 9075:1992 §13.3 SR4, GR2a] The value J of ABSOLUTE or RELATIVE.
fn how_far(j: &ValueSpecification, auth: &str, params: &Params) -> Result<i128> {
    let not_exact = || rule("[ISO/IEC 9075:1992 §13.3 SR4] ABSOLUTE and RELATIVE take an exact numeric with scale 0".to_owned());
    let (value, data_type) = binder::value_specification(j, auth, params)?;
    if !matches!(data_type, DataType::Exact { scale: 0, .. }) {
        return Err(not_exact());
    }
    match value {
        Value::Exact(d) => Ok(d.units),
        Value::Null => fail_with(
            Code::Data,
            SqlState::NULL_VALUE_NO_INDICATOR,
            "[DESIGN] ABSOLUTE and RELATIVE take a number, not the null value".to_owned(),
        ),
        Value::Char(_) | Value::Approx(_) | Value::Bits(_) | Value::Datetime(_) | Value::Interval(_) | Value::Extension(_) => {
            Err(not_exact())
        }
    }
}

/// An updatable cursor of `spec`, compiled as `c`, its `<from clause>`
/// naming `named`, which reads `table`: its rows may be updated or
/// deleted through it, and its subqueries' tables are read.
fn open_updatable(
    st: &Arc<State>,
    tx: &Txn,
    spec: &CursorSpecification,
    (named, c, table): (QualifiedName, Compiled, QualifiedName),
) -> Result<OpenCursor> {
    st.update_lock(tx, &table)?;
    for t in c.optimized.subquery_tables() {
        st.read_lock(tx, &t)?;
    }
    let projection: Vec<usize> = c
        .columns
        .iter()
        .map(|col| col.origin.ok_or_else(|| api(format!("[DESIGN] a cursor's column is not a column of {table}"))))
        .collect::<Result<_>>()?;
    let settable = settable(&st.base_table(&table)?, spec.updatability.as_ref())?;
    let ordered = ordered_columns(&c.optimized, &projection);
    let condition = row_condition(&c.optimized);
    let ids = ordered_ids(c.physical, st, tx.id, &projection)?;
    let snapshot = Arc::clone(st);
    let at = Position::Before(0);
    let rows = Updatable { snapshot, named, table, ids, projection, condition, at, settable, ordered, changed: Vec::new() };
    Ok(OpenCursor { columns: c.columns, rows: CursorRows::Updatable(Box::new(rows)) })
}

/// [ISO/IEC 9075:1992 §13.9 SR4, SR8] The columns a positioned UPDATE `w`
/// through cursor `name`, updatable as `u`, sets are among those its FOR
/// UPDATE names, and none of them is read by its ORDER BY. A positioned
/// DELETE sets none.
fn check_object_columns(u: &Updatable, w: &BoundWrite, name: &str) -> Result<()> {
    for (column, _) in &w.set {
        let named = || w.desc.columns.get(*column).map_or_else(|| column.to_string(), |c| c.name.clone());
        if !u.settable.contains(column) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §13.9 SR8] cursor {name} is not FOR UPDATE of {}", named())));
        }
        if u.ordered.contains(column) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §13.9 SR4] cursor {name} is ordered by {}, which it does not update", named())));
        }
    }
    Ok(())
}

/// A cursor's specification bound and planned: its columns, its bound plan
/// shown, the optimizer's trace, and its plans.
pub(super) struct Compiled {
    pub(super) columns: Vec<OutColumn>,
    pub(super) bound: String,
    pub(super) trace: Vec<&'static str>,
    pub(super) optimized: LogicalPlan,
    pub(super) physical: Plan,
}

impl Session {
    /// The row an updatable cursor is on. [X3.135-1986 §8.4 SR2, SR3, SR4, GR1]
    /// and the same-numbered rules of [X3.135-1986 §8.11]: the cursor is
    /// declared, is not read-only, has this table in its FROM, and is
    /// positioned on a row.
    ///
    /// The cursor's own state is used, as recorded at OPEN, rather than its
    /// text: the text may name parameters of the procedure that opened it,
    /// which this statement need not declare [X3.135-1986 §8.3 SR2, §8.8 GR3a].
    pub(super) fn positioned_row(&self, cx: StatementContext<'_>, name: &str, w: &BoundWrite, clause: &str) -> Result<RowId> {
        let (named, table) = (&w.named, &w.table);
        let inner = self.inner.borrow();
        let c = inner.cursors.get(name).ok_or_else(|| rule(format!("[X3.135-1986 {clause} SR2] no cursor {name} is declared")))?;
        let read_only = || rule(format!("[X3.135-1986 {clause} SR3, §8.3 SR4] cursor {name} is read-only"));
        let not_positioned = || cursor_error(name, format!("[X3.135-1986 {clause} GR1] cursor {name} is not positioned on a row"));
        match &c.open {
            Some(OpenCursor { rows: CursorRows::Table(_) | CursorRows::Kept(_), .. }) => Err(read_only()),
            Some(OpenCursor { rows: CursorRows::Updatable(u), .. }) => {
                if *named != u.named {
                    return Err(rule(format!("[X3.135-1986 {clause} SR4] {named} is not the table of cursor {name} ({})", u.named)));
                }
                check_object_columns(u, w, name)?;
                let storage = cx.st.storage_of(table)?;
                let live = match u.current() {
                    Some(id) => storage.get(cx.tx.id, id)?.map(|_| id),
                    None => None,
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

    pub(super) fn parse_cursor(&self, st: &State, sql: &str, params: &Params) -> Result<CursorSpecification> {
        params.parser(sql, self.extensions, &st.catalog_name())?.cursor_specification()
    }

    pub(super) fn compile(&self, st: &State, spec: &CursorSpecification, params: &Params) -> Result<Compiled> {
        let bound = self.bound(st, params, |cx| bind_cursor(cx, spec))?;
        let text = bound.plan.explain();
        let (optimized, trace, physical) = plan(st, bound.plan);
        Ok(Compiled { columns: bound.columns, bound: text, trace, optimized, physical })
    }

    /// [X3.135-1986 §8.3 SR4b; ISO/IEC 9075:1992 §13.1 SR5, §7.9 SR12] The
    /// compiled specification of a cursor its text makes updatable, and
    /// the one table it reads, unless a subquery of its WHERE clause reads
    /// that table too \[SR12d\]: then it is read-only.
    fn updatable_plan(&self, st: &State, spec: &CursorSpecification, params: &Params) -> Result<Option<(Compiled, QualifiedName)>> {
        let c = self.compile(st, spec, params)?;
        let read = c.optimized.subquery_tables();
        let own: Vec<QualifiedName> = c.optimized.tables().into_iter().filter(|t| !read.contains(t)).collect();
        let [table] = own.as_slice() else { return Ok(None) };
        let table = table.clone();
        Ok(Some((c, table)))
    }

    /// The rows of a read-only cursor's specification, after taking
    /// shared locks on the tables it reads [X3.135-1986 §8.8 GR3b].
    fn open_rows(&self, st: &Arc<State>, tx: &Txn, spec: &CursorSpecification, params: &Params) -> Result<(Vec<OutColumn>, Stream)> {
        let c = self.compile(st, spec, params)?;
        let reads = c.optimized.tables();
        for t in &reads {
            st.read_lock(tx, t)?;
        }
        Ok((c.columns, Stream::open(c.physical, reads, st, tx.id)?))
    }

    /// [ISO/IEC 9075:1992 §11.8 GR8] The rows changed through each cursor
    /// still open: its name, table and row.
    pub(super) fn cursor_changes(&self) -> Vec<(String, QualifiedName, RowId)> {
        let inner = self.inner.borrow();
        let mut rows = Vec::new();
        for (name, c) in &inner.cursors {
            if let Some(OpenCursor { rows: CursorRows::Updatable(u), .. }) = c.open.as_ref() {
                rows.extend(u.changed.iter().map(|id| (name.clone(), u.table.clone(), *id)));
            }
        }
        rows
    }

    /// Notes row `id` changed through cursor `name` [ISO/IEC 9075:1992
    /// §11.8 GR8].
    fn changed_through(&self, name: &str, id: RowId) {
        if let Some(DeclaredCursor { open: Some(OpenCursor { rows: CursorRows::Updatable(u), .. }), .. }) =
            self.inner.borrow_mut().cursors.get_mut(name)
        {
            u.changed.push(id);
        }
    }

    /// Whether an open cursor of the session reads `table`.
    pub(super) fn cursor_reads(&self, table: &QualifiedName) -> bool {
        let inner = self.inner.borrow();
        let named = inner.cursors.values().filter_map(|c| c.open.as_ref()).any(|open| match &open.rows {
            CursorRows::Table(rows) => rows.reads(table),
            CursorRows::Kept(kept) => kept.stream.reads(table),
            CursorRows::Updatable(u) => u.table == *table,
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

    /// [X3.135-1986 §8.8; ISO/IEC 9075:1992 §13.2] OPEN
    pub(super) fn open(&self, st: &Arc<State>, tx: &Txn, name: &str, params: &Params) -> Result<Completion> {
        let (text, flags) = {
            let inner = self.inner.borrow();
            let c = inner.cursors.get(name).ok_or_else(|| rule(format!("[X3.135-1986 §8.8 SR1] no cursor {name} is declared")))?;
            if c.open.is_some() {
                return cursor_error(name, format!("[X3.135-1986 §8.8 GR1] cursor {name} is already open"));
            }
            (c.specification.clone(), (c.insensitive, c.scroll))
        };
        // [X3.135-1986 §8.8 GR3a] a copy of S with each parameter replaced by its value.
        let spec = self.parse_cursor(st, &text, params)?;
        let wanted = wanted(&spec, flags)?;
        let updatable = match (wanted, binder::updatable_in(&spec, &*st.catalog, &self.default_schema())) {
            (Wanted::Written | Wanted::Implicit, Some(named)) => {
                self.updatable_plan(st, &spec, params)?.map(|(c, table)| (named, c, table))
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
            let (columns, rows) = self.open_rows(st, tx, &spec, params)?;
            OpenCursor { columns, rows: if flags.1 { CursorRows::Kept(Kept::new(rows)) } else { CursorRows::Table(rows) } }
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
        name: &str,
        (orientation, targets): (&FetchOrientation, &[ParameterSpecification]),
        params: &mut Params,
    ) -> Result<Completion> {
        let values = {
            let mut inner = self.inner.borrow_mut();
            let c = inner.cursors.get_mut(name).ok_or_else(|| rule(format!("[X3.135-1986 §8.6 SR1] no cursor {name} is declared")))?;
            let to = movement(orientation, c.scroll, (&self.auth(), params))?;
            let Some(open) = c.open.as_mut() else {
                return cursor_error(name, format!("[X3.135-1986 §8.6 GR1] cursor {name} is not open"));
            };
            check_targets(&open.columns, targets, params, "§8.6 SR2", "§8.6 SR3")?;
            match &mut open.rows {
                // [§13.3 SR3] Not SCROLL: NEXT.
                CursorRows::Table(rows) => rows.next()?,
                CursorRows::Kept(kept) => kept.fetch(to)?,
                CursorRows::Updatable(u) => u.fetch(cx.tx.id, to)?,
            }
        };
        match values {
            // [X3.135-1986 §8.6 GR2] positioned after the last row; SQLCODE 100.
            None => Ok(Completion::NoData),
            Some(mut values) => {
                show_in(&mut values, self.now.get().zone);
                assign_targets(&values, targets, params, "§8.6 GR8", "§8.6 GR10")?;
                Ok(Completion::Success)
            }
        }
    }

    /// [X3.135-1986 §8.1] CLOSE
    pub(super) fn close(&self, name: &str) -> Result<Completion> {
        let mut inner = self.inner.borrow_mut();
        let c = inner.cursors.get_mut(name).ok_or_else(|| rule(format!("[X3.135-1986 §8.1 SR1] no cursor {name} is declared")))?;
        if c.open.take().is_none() {
            return cursor_error(name, format!("[X3.135-1986 §8.1 GR1] cursor {name} is not open"));
        }
        Ok(Completion::Success)
    }

    /// [X3.135-1986 §8.10] SELECT ... INTO
    pub(super) fn select_into(
        &self,
        cx: StatementContext<'_>,
        query: &QuerySpecification,
        targets: &[ParameterSpecification],
        params: &mut Params,
    ) -> Result<Completion> {
        let bound = self.bound(cx.st, params, |b| bind_select(b, query))?;
        check_targets(&bound.columns, targets, params, "§8.10 SR3", "§8.10 SR4")?;
        let (optimized, _, physical) = plan(cx.st, bound.plan);
        for t in optimized.tables() {
            cx.st.read_lock(cx.tx, &t)?;
        }
        let rows: Vec<Row> = execute(physical, cx.st, cx.tx.id).take(CARDINALITY_PROBE_ROWS).collect::<Result<_>>()?;
        match rows.as_slice() {
            // [X3.135-1986 §8.10 GR2] no row; the targets are not assigned.
            [] => Ok(Completion::NoData),
            [row] => {
                let mut row = row.clone();
                show_in(&mut row, self.now.get().zone);
                assign_targets(&row, targets, params, "§8.10 GR8", "§8.10 GR9")?;
                Ok(Completion::Success)
            }
            [_, _, ..] => fail(Code::Cardinality, "[X3.135-1986 §8.10 GR2] the result has more than one row"),
        }
    }

    /// \[DESIGN\] Read a `<cursor specification>` [X3.135-1986 §8.3] in one call: an
    /// unnamed cursor's rows, as FETCH would return them but without assigning
    /// them to parameters. It runs in the session's transaction, and is
    /// closed when that transaction ends.
    ///
    /// # Errors
    ///
    /// Any error of the cursor specification or of reading its rows.
    pub fn open_cursor(&self, sql: &str) -> Result<Cursor> {
        // [ISO/IEC 9075:1992 §18.1 Table 22] a <direct select statement:
        // multiple rows>.
        self.open_with(sql, &Params::new(), ("SELECT", ""))
    }

    /// An unnamed cursor of `sql`, as `open_cursor` opens it, with the
    /// parameters `params`: a prepared statement's dynamic ones; the
    /// diagnostics area naming it as `command` says.
    pub(super) fn open_with(&self, sql: &str, params: &Params, command: (&'static str, &'static str)) -> Result<Cursor> {
        let call = self.enter()?;
        self.emptied(command);
        let opened = self.open_in(&call, sql, params);
        self.diagnosed(opened, |_| SqlState::SUCCESSFUL_COMPLETION)
    }

    /// `open_with`'s cursor, in the call `call`.
    fn open_in(&self, call: &super::Call<'_>, sql: &str, params: &Params) -> Result<Cursor> {
        let mut timed = super::slow::Timed::of(call.state(), sql.to_owned(), &self.auth());
        let spec = self.parse_cursor(call.state(), sql, params)?;
        if let Some(Updatability::Update(_)) = spec.updatability {
            return Err(rule(
                "[DESIGN] an unnamed cursor is read-only: no positioned statement names it, so it is not FOR UPDATE".to_owned(),
            ));
        }
        let st = call.state();
        let tx = self.ensure_transaction(st);
        call.runs(&tx);
        tx.begin_statement(|| !self.cursors_open())?;
        let (columns, rows) = self.open_rows(st, &tx, &spec, params).map_err(|e| st.fail_statement(&tx, &e).err().unwrap_or(e))?;
        let rows = Arc::new(Mutex::new(Some(rows)));
        self.inner.borrow_mut().anonymous.push(Arc::downgrade(&rows));
        if let Some(opened) = timed.as_mut() {
            opened.paused();
        }
        Ok(Cursor { columns, rows, db: self.db.clone(), stop: Arc::clone(&self.stop), timed, zone: self.now.get().zone })
    }

    /// [X3.135-1986 §8.11] the positioned UPDATE bound as `w`, through
    /// cursor `cursor`.
    pub(super) fn update_positioned(&self, cx: StatementContext<'_>, (cursor, at): (&str, usize), w: &BoundWrite) -> Result<Completion> {
        let StatementContext { st, tx } = cx;
        let id = self.positioned_row(cx, cursor, w, "§8.11").or_at(at)?;
        let old = st.storage_of(&w.table)?.get(tx.id, id)?.ok_or_else(|| api(format!("[DESIGN] row {id} of {} vanished", w.table)))?;
        for t in w.set_tables() {
            st.read_lock(tx, &t)?;
        }
        let runner = Runner::new(st, tx.id);
        let new = candidate(w, &old, ("ISO/IEC 9075:1992 §13.9 GR9", &runner))?;
        st.update_row(tx, &w.table, id, &new)?;
        check_option(&w.checks, [&new], ("§8.11 GR5", &runner))?;
        let mut effects = self.effects();
        effects.updated(&w.table, id, old, &set_columns(w));
        st.end_statement(tx, effects)?;
        self.changed_through(cursor, id);
        Ok(Completion::Success)
    }

    /// [X3.135-1986 §8.4] the positioned DELETE bound as `w`, through
    /// cursor `cursor`.
    pub(super) fn delete_positioned(&self, cx: StatementContext<'_>, (cursor, at): (&str, usize), w: &BoundWrite) -> Result<Completion> {
        let StatementContext { st, tx } = cx;
        let id = self.positioned_row(cx, cursor, w, "§8.4").or_at(at)?;
        let old = st.storage_of(&w.table)?.get(tx.id, id)?.ok_or_else(|| api(format!("[DESIGN] row {id} of {} vanished", w.table)))?;
        st.delete_row(tx, &w.table, id)?;
        let mut effects = self.effects();
        effects.deleted(&w.table, id, old);
        st.end_statement(tx, effects)?;
        // [X3.135-1986 §4.12] the cursor is now before the next row.
        if let Some(DeclaredCursor { open: Some(OpenCursor { rows: CursorRows::Updatable(u), .. }), .. }) =
            self.inner.borrow_mut().cursors.get_mut(cursor)
        {
            u.deleted();
            u.changed.push(id);
        }
        Ok(Completion::Success)
    }
}
