//! The rows a data-changing statement works on: the candidate rows of INSERT,
//! from a query expression [X3.135-1986 §8.7; ISO/IEC 9075:1992 §13.8], and the object rows of
//! searched UPDATE and DELETE [X3.135-1986 §8.5, §8.12]. Split from db.rs
//! (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::assignment::{candidate, check_option};
use super::defaults::default_row;
use super::integrity::Effects;
use super::{Session, State, StatementContext, plan, rule, set_columns};
use crate::ast::{ColumnName, InsertSource, InsertStatement, QueryExpression, RowElement, SelectList, SelectSublist};
use crate::binder::{self, BoundWrite, INSERT_RULE, ObjectColumn, ViewCheck, no_column, qualify};
use crate::catalog::{Actions, ColumnDesc, QualifiedName, TableDesc};
use crate::error::{At, Result};
use crate::exec::{HeldRows, Runner, execute, row_entries};
use crate::params::Params;
use crate::storage::{Row, RowId};
use crate::txn::Txn;
use crate::value::{Value, assign};

/// The table an INSERT puts rows in, and its object columns [X3.135-1986
/// §8.7 SR3, SR4].
pub(super) struct Target {
    /// The base table: it, or an updatable view's [X3.135-1986 §6.5 GR2].
    name: QualifiedName,
    desc: TableDesc,
    /// The object columns, as columns of the base table.
    object: Vec<usize>,
    /// [X3.135-1986 §8.7 GR2; ISO/IEC 9075:1992 §11.19 GR11] the views
    /// WITH CHECK OPTION the INSERT spans.
    checks: Vec<ViewCheck>,
    /// [ISO/IEC 9075:1992 §13.8 GR4b i] the default of each column of the
    /// base table.
    defaults: Row,
}

impl Target {
    /// The object columns, in order.
    fn columns(&self) -> impl Iterator<Item = &ColumnDesc> {
        self.object.iter().filter_map(|&c| self.desc.columns.get(c))
    }

    /// [X3.135-1986 §8.7 GR1a, b, GR5, GR7] the candidate row for the
    /// insert values `values`: each column's default [ISO/IEC 9075:1992
    /// §13.8 GR4b i], null if it has none, but for the object columns, each
    /// set to its value [GR4b ii].
    fn candidate(&self, values: Row) -> Result<Row> {
        let mut row = self.defaults.clone();
        for (v, &c) in values.into_iter().zip(&self.object) {
            // The object columns are columns of T [X3.135-1986 §8.7 SR3], so both lookups succeed.
            let (Some(col), Some(slot)) = (self.desc.columns.get(c), row.get_mut(c)) else {
                return Err(rule(format!("[X3.135-1986 §8.7 SR3] {} has no column {c}", self.name)));
            };
            *slot = assign(v, &col.data_type, "ISO/IEC 9075:1992 §13.8 GR4b iii")?;
        }
        Ok(row)
    }
}

impl Session {
    /// [X3.135-1986 §8.7] `<insert statement>`
    /// The rows it inserted [ISO/IEC 9075:1992 §18.1 GR1e i].
    pub(super) fn insert_statement(&self, st: &Arc<State>, tx: &Txn, stmt: &InsertStatement, params: &Params) -> Result<usize> {
        let target = self.insert_target(st, stmt)?;
        let mut rows = match &stmt.source {
            InsertSource::Query(query) => self.query_rows(StatementContext { st, tx }, &target, query, params)?,
            // [ISO/IEC 9075:1992 §13.8 SR2] VALUES (DEFAULT, ...): every
            // column its default.
            InsertSource::DefaultValues => {
                let mut one = HeldRows::new(st.list_space());
                one.push(target.defaults.clone())?;
                one
            }
        };
        // [X3.135-1986 §8.7 GR4] R is empty: no row is inserted.
        if rows.is_empty() {
            return Ok(0);
        }
        self.settle_cursors(st, &target.name);
        let mut effects = self.effects();
        rows.visit(&mut |row| {
            effects.inserted(&target.name, st.insert_row(tx, &target.name, row)?); // [X3.135-1986 §8.7 GR1c]
            Ok(())
        })?;
        if !target.checks.is_empty() {
            let runner = Runner::new(st, tx.id);
            rows.visit(&mut |row| check_option(&target.checks, [row], ("§8.7 GR2", &runner)))?;
        }
        st.end_statement(tx, effects)?;
        Ok(rows.len())
    }

    /// [X3.135-1986 §8.7 SR1-SR4] the table `stmt` inserts into, which is
    /// not read-only, and its object columns: through an updatable view,
    /// its base table's [X3.135-1986 §6.5 GR2].
    pub(super) fn insert_target(&self, st: &State, stmt: &InsertStatement) -> Result<Target> {
        let held = self.auth();
        let auth = held.as_str();
        let name = qualify(&stmt.table, &self.default_schema());
        let target = binder::write_target(st, &name, ("§8.7 SR2", auth)).or_at(stmt.table.at)?;
        let desc = &target.named;
        let object: Vec<usize> = match &stmt.columns {
            None => (0..desc.columns.len()).collect(),
            Some(list) => {
                let mut cols = Vec::new();
                for ColumnName { name: c, at } in list {
                    let missing = || no_column(&name, format!("[X3.135-1986 §8.7 SR3] {name} has no column {c}"), c).or_at(*at);
                    let i = desc.column(c).ok_or_else(missing)?;
                    if cols.contains(&i) {
                        return Err(rule(format!("[X3.135-1986 §8.7 SR3] column {c} is listed twice")).or_at(*at));
                    }
                    cols.push(i);
                }
                cols
            }
        };
        // [ISO/IEC 9075:1992 §13.8 AR1] INSERT on each column named, or on
        // each of the table's.
        let needed = Actions { insert: object.iter().copied().collect(), ..Actions::default() };
        binder::require(st, auth, desc, &needed, "§8.7 SR1").or_at(stmt.table.at)?;
        // [X3.135-1986 §8.7 GR1a] the candidate row is one of the base table.
        let object = object.iter().filter_map(|&i| target.map.get(i).copied()).collect();
        let defaults = default_row(&target.base, auth, self.now.get())?;
        Ok(Target { name: target.base.name.clone(), desc: target.base, object, checks: target.checks, defaults })
    }

    /// [X3.135-1986 §8.7 SR5, SR6] The `<query expression>` `query` of an
    /// INSERT into `target`, bound: its columns those of `target`'s object
    /// columns, NULL and DEFAULT among its values theirs.
    pub(super) fn bound_insert(&self, st: &State, target: &Target, query: &QueryExpression, params: &Params) -> Result<binder::Bound> {
        let objects: Vec<ObjectColumn> = target
            .columns()
            .zip(&target.object)
            .map(|(col, &c)| ObjectColumn { desc: col.clone(), default: target.defaults.get(c).cloned().unwrap_or(Value::Null) })
            .collect();
        self.bound(st, params, |bound| binder::bind_insert_query(bound, query, &objects))
    }

    /// [X3.135-1986 §8.7 GR3, GR4; ISO/IEC 9075:1992 §13.8 GR3, GR4] the
    /// candidate rows of a `<query expression>`, one for each row of its
    /// result, which is evaluated in full before any row is inserted.
    fn query_rows(&self, cx: StatementContext<'_>, target: &Target, query: &QueryExpression, params: &Params) -> Result<HeldRows<Row>> {
        let StatementContext { st, tx } = cx;
        let bound = self.bound_insert(st, target, query, params)?;
        if bound.columns.len() != target.object.len() {
            let (n, m) = (bound.columns.len(), target.object.len());
            let failed = rule(format!("[X3.135-1986 §8.7 SR5; ISO/IEC 9075:1992 §13.8 SR5] a query of degree {n} for {m} columns"));
            return Err(degree_at(query, m).map_or(failed.clone(), |at| failed.or_at(at)));
        }
        // [X3.135-1986 §8.7 SR6] placed at its select list item, if written;
        // a <table value constructor>'s values are checked as bound.
        let written: Vec<usize> = match query {
            QueryExpression::Spec(spec) => match &spec.select_list {
                SelectList::Values(items) => items
                    .iter()
                    .map(|i| if let SelectSublist::Derived(d) = i { Some(d.at) } else { None })
                    .collect::<Option<_>>()
                    .unwrap_or_default(),
                SelectList::Star => Vec::new(),
            },
            QueryExpression::Values(_) | QueryExpression::SetOp(_) => Vec::new(),
        };
        if !matches!(query, QueryExpression::Values(_)) {
            for (n, (out, col)) in bound.columns.iter().zip(target.columns()).enumerate() {
                let checked = binder::check_assignment(col, out.data_type, INSERT_RULE);
                match written.get(n) {
                    Some(&at) => checked.or_at(at)?,
                    None => checked?,
                }
            }
        }
        let (optimized, _, physical) = plan(st, bound.plan);
        for t in optimized.tables() {
            st.read_lock(tx, &t)?;
        }
        // Held up to WORK_MEMORY, the rest spilled (RULES.md D38).
        let mut rows = HeldRows::new(st.list_space());
        for values in execute(physical, st, tx.id) {
            rows.push(target.candidate(values?)?)?;
        }
        Ok(rows)
    }
}

/// The object rows of a searched UPDATE or DELETE, found before any is
/// changed [X3.135-1986 §8.5 GR1, §8.12 GR1], held up to WORK_MEMORY, the
/// rest spilled (RULES.md D38).
pub(super) fn object_rows(st: &Arc<State>, tx: &Txn, w: &BoundWrite) -> Result<HeldRows<(RowId, Row)>> {
    // T, to be written, and the tables its subqueries read.
    st.update_lock(tx, &w.table)?;
    // The tables its condition's and its SET values' subqueries read, the
    // object table among them perhaps [ISO/IEC 9075:1992 §13.7, §13.10,
    // Leveling Rules].
    for t in w.rows.tables().into_iter().chain(w.set_tables()) {
        st.read_lock(tx, &t)?;
    }
    let (_, _, physical) = plan(st, w.rows.clone());
    row_entries(physical, st, tx.id)
}

/// [X3.135-1986 §8.12] the searched UPDATE bound as `w`: the rows it
/// updated [ISO/IEC 9075:1992 §18.1 GR1e], none for no data [X3.135-1986
/// §7.3 GR3a iv].
pub(super) fn update_searched(
    StatementContext { st, tx }: StatementContext<'_>,
    w: &BoundWrite,
    mut effects: Effects<'_>,
) -> Result<usize> {
    let objects = object_rows(st, tx, w)?;
    if objects.is_empty() {
        return Ok(0);
    }
    let count = objects.len();
    let columns = set_columns(w);
    let runner = Runner::new(st, tx.id);
    // [ISO/IEC 9075:1992 §13.10 GR6] every value computed before any row
    // is updated: a subquery of one may read the table.
    let mut changes = HeldRows::new(st.list_space());
    for object in objects {
        let (id, old) = object?;
        let new = candidate(w, &old, ("ISO/IEC 9075:1992 §13.10 GR8", &runner))?;
        changes.push((id, old, new))?;
    }
    changes.visit(&mut |(id, old, new)| {
        st.update_row(tx, &w.table, *id, new)?;
        effects.updated(&w.table, *id, old.clone(), &columns);
        Ok(())
    })?;
    if !w.checks.is_empty() {
        changes.visit(&mut |(_, _, new)| check_option(&w.checks, [new], ("§8.12 GR4", &runner)))?;
    }
    st.end_statement(tx, effects)?;
    Ok(count)
}

/// [X3.135-1986 §8.5] the searched DELETE bound as `w`: the rows it
/// deleted [ISO/IEC 9075:1992 §18.1 GR1e], none for no data [X3.135-1986
/// §7.3 GR3a iv].
pub(super) fn delete_searched(
    StatementContext { st, tx }: StatementContext<'_>,
    w: &BoundWrite,
    mut effects: Effects<'_>,
) -> Result<usize> {
    let objects = object_rows(st, tx, w)?;
    if objects.is_empty() {
        return Ok(0);
    }
    let count = objects.len();
    for object in objects {
        let (id, old) = object?;
        st.delete_row(tx, &w.table, id)?;
        effects.deleted(&w.table, id, old);
    }
    st.end_statement(tx, effects)?;
    Ok(count)
}

/// Where an INSERT's `query` of another degree than its `m` columns is
/// placed: at its first value or select list item too many, or, of too
/// few, at its first row or item.
fn degree_at(query: &QueryExpression, m: usize) -> Option<usize> {
    match query {
        QueryExpression::Values(rows) => {
            let row = rows.first()?;
            Some(row.elements.get(m).map_or(row.at, |e| match e {
                RowElement::Value(_, at) | RowElement::Null(at) | RowElement::Default(at) => *at,
            }))
        }
        QueryExpression::Spec(spec) => {
            let SelectList::Values(items) = &spec.select_list else { return None };
            let at = |i: &SelectSublist| if let SelectSublist::Derived(d) = i { Some(d.at) } else { None };
            items.get(m).or(items.first()).and_then(at)
        }
        QueryExpression::SetOp(_) => None,
    }
}
