//! The rows a data-changing statement works on: the candidate rows of INSERT,
//! from a query expression [X3.135-1986 §8.7; ISO/IEC 9075:1992 §13.8], and
//! the object rows of searched UPDATE and DELETE [X3.135-1986 §8.5, §8.12].

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;

use super::assignment::{candidate, check_option};
use super::defaults::default_row;
use super::integrity::Effects;
use super::prepared::{Keeping, KeptPlan};
use super::{Session, State, StatementContext, plan, rule, set_columns};
use crate::ast::{ColumnName, InsertSource, InsertStatement, QueryExpression, RowElement, SelectList, SelectSublist};
use crate::binder::{self, BoundWrite, INSERT_RULE, ObjectColumn, ViewCheck, no_column, qualify};
use crate::catalog::{Actions, ColumnDesc, QualifiedName, TableDesc};
use crate::error::{At, Result};
use crate::exec::{HeldRows, Runner, eval_cond, execute, row_entries};
use crate::logical::{Expr, LogicalPlan, Truth, filled, filled_cond, filled_expr};
use crate::params::Params;
use crate::physical::{Plan, filled_plan};
use crate::storage::{Row, RowId};
use crate::txn::{Again, Txn};
use crate::value::datetime::Now;
use crate::value::{Value, assign};

/// The table an INSERT puts rows in, and its object columns [X3.135-1986
/// §8.7 SR3, SR4].
#[derive(Debug, Clone)]
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

    /// It, its defaults those of a statement run under `auth` at `now`: as
    /// a prepared statement's kept plan takes it again (RULES.md D45).
    ///
    /// # Errors
    ///
    /// As `default_row`.
    pub(super) fn refreshed(self, auth: &str, now: Now) -> Result<Target> {
        Ok(Target { defaults: default_row(&self.desc, auth, now)?, ..self })
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

/// An UPDATE or DELETE as one execution runs it: as bound, shared with
/// the plan a prepared statement keeps (RULES.md D45), its SET values and
/// the plan of its object rows filled with the execution's values.
pub(super) struct Written {
    /// As bound; of a kept plan, its slots unfilled.
    pub(super) bound: Arc<BoundWrite>,
    /// Its SET values filled, of a kept plan; else none, and the bound
    /// ones are.
    set: Option<Vec<(usize, Option<Expr>)>>,
    rows: ObjectRows,
    /// The values it was filled with, of a kept plan: its search condition
    /// is filled with them only if a row is checked again (`condition`).
    values: Option<Vec<Value>>,
}

/// The plan of an UPDATE's or DELETE's object rows.
enum ObjectRows {
    /// As bound, planned as the statement runs.
    Bound,
    /// A kept plan's, filled, planned as the statement runs.
    Filled(LogicalPlan),
    /// Planned once with its slots and kept, filled; the tables its
    /// object rows' plan reads.
    Kept(Plan, Arc<[QualifiedName]>),
}

impl Written {
    /// `bound`, as a statement not prepared runs it.
    pub(super) fn of(bound: BoundWrite) -> Written {
        Written { bound: Arc::new(bound), set: None, rows: ObjectRows::Bound, values: None }
    }

    /// `bound`, a kept plan, filled with `values`: its object rows by
    /// `kept`, its plan made once with its slots and the tables it reads,
    /// if given; else by its logical plan, filled.
    ///
    /// # Errors
    ///
    /// As `filled` and `physical::filled_plan`.
    pub(super) fn filled(bound: Arc<BoundWrite>, kept: Option<(Plan, Arc<[QualifiedName]>)>, values: &[Value]) -> Result<Written> {
        let fill = |(c, e): &(usize, Option<Expr>)| Ok((*c, e.clone().map(|e| filled_expr(e, values)).transpose()?));
        let set = bound.set.iter().map(fill).collect::<Result<_>>()?;
        let rows = match kept {
            Some((plan, reads)) => ObjectRows::Kept(filled_plan(plan, values)?, reads),
            None => ObjectRows::Filled(filled(bound.rows.clone(), values)?),
        };
        Ok(Written { bound, set: Some(set), rows, values: Some(values.to_vec()) })
    }

    /// Its SET values: column position and new value (`None` is NULL), in
    /// the order written.
    pub(super) fn set(&self) -> &[(usize, Option<Expr>)] {
        self.set.as_deref().unwrap_or(&self.bound.set)
    }

    /// Whether row `row` of the object table, a version another
    /// transaction committed after the statement's snapshot, is one it
    /// changes: its views' conditions and its search condition hold of it
    /// [ISO/IEC 9075:1992 §4.28] (RULES.md D29).
    ///
    /// # Errors
    ///
    /// As `eval_cond`.
    fn holds_of(&self, row: &Row, runner: &Runner) -> Result<bool> {
        let LogicalPlan::Filter { cond, .. } = &self.bound.rows else { return Ok(true) };
        let truth = match &self.values {
            Some(values) => eval_cond(&filled_cond(cond.clone(), values)?, row, runner)?,
            None => eval_cond(cond, row, runner)?,
        };
        Ok(truth == Truth::True)
    }
}

impl Session {
    /// [X3.135-1986 §8.7] `<insert statement>`; of a prepared statement,
    /// with the plans it keeps (RULES.md D45).
    /// The rows it inserted [ISO/IEC 9075:1992 §18.1 GR1e i].
    pub(super) fn insert_statement(
        &self,
        cx: StatementContext<'_>,
        stmt: &InsertStatement,
        params: &Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<usize> {
        let StatementContext { st, tx } = cx;
        let (target, query) = self.insert_bound(st, stmt, params, keeping)?;
        let mut rows = if let Some(logical) = query {
            query_rows(cx, &target, logical)?
        } else {
            // [ISO/IEC 9075:1992 §13.8 SR2] VALUES (DEFAULT, ...): every
            // column its default.
            let mut one = HeldRows::new(st.list_space());
            one.push(target.defaults.clone())?;
            one
        };
        // [X3.135-1986 §8.7 GR4] R is empty: no row is inserted.
        if rows.is_empty() {
            return Ok(0);
        }
        self.settle_cursors(st, &target.name);
        let mut effects = self.effects(st);
        rows.visit(&mut |row| {
            effects.inserted(&target.name, st.insert_row(tx, &target.name, row)?) // [X3.135-1986 §8.7 GR1c]
        })?;
        if !target.checks.is_empty() {
            let runner = Runner::new(st, tx.id);
            rows.visit(&mut |row| check_option(&target.checks, [row], ("§8.7 GR2", &runner)))?;
        }
        st.end_statement(tx, effects)?;
        Ok(rows.len())
    }

    /// The table `stmt` inserts into and the plan of the rows its query
    /// gives, if it has one: of a prepared statement, as it keeps them,
    /// its values filled in (RULES.md D45).
    fn insert_bound(
        &self,
        st: &State,
        stmt: &InsertStatement,
        params: &Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<(Target, Option<LogicalPlan>)> {
        let values = params.dynamic_values();
        if let Some(KeptPlan::Insert(target, query)) = keeping.map(Keeping::plan).transpose()?.flatten() {
            let target = target.refreshed(&self.auth(), self.now.get())?;
            return Ok((target, query.map(|q| filled(q, values)).transpose()?));
        }
        let kept = keeping.map(|_| params.keeping());
        let binding = kept.as_ref().unwrap_or(params);
        let target = self.insert_target(st, stmt, binding.noted())?;
        let query = match &stmt.source {
            InsertSource::Query(query) => Some(self.insert_query(st, &target, query, binding)?),
            InsertSource::DefaultValues => None,
        };
        let Some(k) = keeping else { return Ok((target, query)) };
        k.made((!binding.held()).then(|| KeptPlan::Insert(Box::new(target.clone()), query.clone())))?;
        Ok((target, query.map(|q| filled(q, values)).transpose()?))
    }

    /// [X3.135-1986 §8.7 SR1-SR4] the table `stmt` inserts into, which is
    /// not read-only, and its object columns: through an updatable view,
    /// its base table's [X3.135-1986 §6.5 GR2]. A view's read of the
    /// statement's instant is noted in `noted`, if given (`Clocked`).
    pub(super) fn insert_target(&self, st: &State, stmt: &InsertStatement, noted: Option<&Cell<bool>>) -> Result<Target> {
        let held = self.auth();
        let auth = held.as_str();
        let name = qualify(&stmt.table, &self.default_schema());
        let target = binder::write_target(st, &name, ("§8.7 SR2", auth), noted).or_at(stmt.table.at)?;
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

    /// The plan of the rows of `query` inserted into `target`, its degree
    /// and its columns' types checked [X3.135-1986 §8.7 SR5, SR6].
    fn insert_query(&self, st: &State, target: &Target, query: &QueryExpression, params: &Params) -> Result<LogicalPlan> {
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
        Ok(bound.plan)
    }
}

/// [X3.135-1986 §8.7 GR3, GR4; ISO/IEC 9075:1992 §13.8 GR3, GR4] the
/// candidate rows of a `<query expression>`, bound as `logical`, one for
/// each row of its result, which is evaluated in full before any row is
/// inserted.
fn query_rows(cx: StatementContext<'_>, target: &Target, logical: LogicalPlan) -> Result<HeldRows<Row>> {
    let StatementContext { st, tx } = cx;
    let (optimized, _, physical) = plan(st, logical);
    for t in optimized.tables() {
        st.read_lock(tx, &t)?;
    }
    // Held up to MEMORY.WORK_BYTES, the rest spilled (RULES.md D38).
    let mut rows = HeldRows::new(st.list_space());
    for values in execute(physical, st, tx.id) {
        rows.push(target.candidate(values?)?)?;
    }
    Ok(rows)
}

/// The object rows of a searched UPDATE or DELETE, found before any is
/// changed [X3.135-1986 §8.5 GR1, §8.12 GR1], held up to MEMORY.WORK_BYTES, the
/// rest spilled (RULES.md D38).
pub(super) fn object_rows(st: &Arc<State>, tx: &Txn, w: Written) -> Result<(Written, HeldRows<(RowId, Row)>)> {
    let Written { bound, set, rows, values } = w;
    // T, to be written, and the tables its subqueries read.
    st.update_lock(tx, &bound.table)?;
    let (logical, kept) = match rows {
        ObjectRows::Bound => (Some(bound.rows.clone()), None),
        ObjectRows::Filled(logical) => (Some(logical), None),
        ObjectRows::Kept(plan, reads) => (None, Some((plan, reads))),
    };
    let reads = match (&logical, &kept) {
        (_, Some((_, reads))) => reads.to_vec(),
        (Some(logical), None) => logical.tables(),
        (None, None) => Vec::new(),
    };
    // The tables its condition's and its SET values' subqueries read, the
    // object table among them perhaps [ISO/IEC 9075:1992 §13.7, §13.10,
    // Leveling Rules]. A SET value's subqueries read the same tables
    // filled or not.
    for t in reads.into_iter().chain(bound.set_tables()) {
        st.read_lock(tx, &t)?;
    }
    let physical = match (kept, logical) {
        (Some((physical, _)), _) => physical,
        (None, logical) => plan(st, logical.unwrap_or_else(|| bound.rows.clone())).2,
    };
    let entries = row_entries(physical, st, tx.id)?;
    Ok((Written { bound, set, rows: ObjectRows::Bound, values }, entries))
}

/// [X3.135-1986 §8.12] the searched UPDATE bound as `w`: the rows it
/// updated [ISO/IEC 9075:1992 §18.1 GR1e], none for no data [X3.135-1986
/// §7.3 GR3a iv].
pub(super) fn update_searched(StatementContext { st, tx }: StatementContext<'_>, w: Written, mut effects: Effects<'_>) -> Result<usize> {
    let (w, objects) = object_rows(st, tx, w)?;
    if objects.is_empty() {
        return Ok(0);
    }
    let columns = set_columns(&w);
    let runner = Runner::new(st, tx.id);
    // [ISO/IEC 9075:1992 §13.10 GR6] every value computed before any row
    // is updated: a subquery of one may read the table.
    let mut changes = HeldRows::new(st.list_space());
    for object in objects {
        let (id, old) = object?;
        let new = candidate(&w, &old, ("ISO/IEC 9075:1992 §13.10 GR8", &runner))?;
        changes.push((id, old, new))?;
    }
    // A row another transaction changed is written in the version it
    // committed, if it is still one to change, its values computed from it.
    let mut again: HashMap<RowId, Option<Row>> = HashMap::new();
    changes.visit(&mut |(id, _, new)| {
        let mut recomputed = None;
        let mut recheck = |newest: &Row| {
            if !w.holds_of(newest, &runner)? {
                return Ok(Again::Skip);
            }
            let row = candidate(&w, newest, ("ISO/IEC 9075:1992 §13.10 GR8", &runner))?;
            recomputed = Some(row.clone());
            Ok(Again::Update(row))
        };
        match st.rewrite_row(tx, (&w.bound.table, *id), Some(new), &mut recheck)? {
            Some(old) => {
                effects.updated(&w.bound.table, *id, old, &columns)?;
                if let Some(row) = recomputed {
                    again.insert(*id, Some(row));
                }
            }
            None => {
                again.insert(*id, None);
            }
        }
        Ok(())
    })?;
    if !w.bound.checks.is_empty() {
        let checked = |row: &Row| check_option(&w.bound.checks, [row], ("§8.12 GR4", &runner));
        changes.visit(&mut |(id, _, new)| match again.get(id) {
            Some(None) => Ok(()),
            Some(Some(row)) => checked(row),
            None => checked(new),
        })?;
    }
    st.end_statement(tx, effects)?;
    Ok(changes.len().saturating_sub(again.values().filter(|r| r.is_none()).count()))
}

/// [X3.135-1986 §8.5] the searched DELETE bound as `w`: the rows it
/// deleted [ISO/IEC 9075:1992 §18.1 GR1e], none for no data [X3.135-1986
/// §7.3 GR3a iv].
pub(super) fn delete_searched(StatementContext { st, tx }: StatementContext<'_>, w: Written, mut effects: Effects<'_>) -> Result<usize> {
    let (w, objects) = object_rows(st, tx, w)?;
    if objects.is_empty() {
        return Ok(0);
    }
    let runner = Runner::new(st, tx.id);
    let mut count = 0_usize;
    for object in objects {
        let (id, _) = object?;
        // A row another transaction changed is deleted in the version it
        // committed, if it is still one to delete.
        let mut recheck = |newest: &Row| Ok(if w.holds_of(newest, &runner)? { Again::Delete } else { Again::Skip });
        if let Some(old) = st.rewrite_row(tx, (&w.bound.table, id), None, &mut recheck)? {
            effects.deleted(&w.bound.table, id, old)?;
            count = count.saturating_add(1);
        }
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
