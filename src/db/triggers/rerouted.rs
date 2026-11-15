//! [ISO/IEC 9075-2:2023 §15.10, §15.13, §15.16] (Feature T213) An INSERT,
//! searched UPDATE or searched DELETE of a view with an INSTEAD OF trigger
//! for its event: no row of the view's tables is written, its trigger is
//! executed in place of that, for the state change of the rows the
//! statement would change.
//!
//! The view's rows are read as its owner reads them, its query bound where
//! it was defined. The statement's WHERE and SET values are then bound
//! over those rows, read as a transition table of the view's name and
//! columns, as the statement's user binds them: so a table they read
//! besides needs that user's privileges. The user holds DELETE, or UPDATE
//! or INSERT on each column named, on the view [§14.9 AR1, §14.11 AR1,
//! §14.14 AR1], and SELECT on it if a WHERE clause is written. No CHECK
//! OPTION applies [§11.49 SR15c, SR15d]. ROW_COUNT counts the transitions.

use std::sync::Arc;

use super::super::{Session, State, StatementContext, plan, rule};
use super::changes::Event;
use crate::ast::{
    ColumnSpecification, InsertSource, InsertStatement, NamedTable, QuerySpecification, SearchCondition, SelectItem, SelectList,
    SelectSublist, SetClause, SqlStatement, TableExpression, TableName, TableReference, UpdateSource, ValueExpression,
};
use crate::binder::{Context, ObjectColumn, bind_insert_query, bind_select, qualify};
use crate::catalog::{ActionTime, Actions, ColumnDesc, Horizon, QualifiedName, TriggerDesc, TriggerEvent, ViewDesc, applicable, owner_of};
use crate::error::{Code, Result, SqlError, Subject};
use crate::exec::{HeldRows, TransitionRows, execute};
use crate::logical::LogicalPlan;
use crate::params::{Params, Transitions};
use crate::storage::{Row, RowId};
use crate::value::{Value, assign};

/// The rule store assignment to a view's column cites.
const STORED: &str = "ISO/IEC 9075-2:2023 §15.13 GR1";

impl Session {
    /// [ISO/IEC 9075-2:2023 §15.10, §15.13, §15.16] `stmt`, in `cx` with
    /// `params`, if it inserts into, updates or deletes from a view with an
    /// INSTEAD OF trigger for its event: that trigger executed in place of
    /// it, and how many rows it would have changed; none of any other.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 for a privilege the user lacks; -103 for a SET clause
    /// of no column of the view, or of one twice; the reads', the binding's
    /// and the trigger's.
    pub(in crate::db) fn instead_of(&self, cx: StatementContext<'_>, stmt: &SqlStatement, params: &Params) -> Result<Option<usize>> {
        if !cx.st.catalog.any_triggers() {
            return Ok(None);
        }
        let (table, event) = if let SqlStatement::Insert(i) = stmt {
            (&i.table, Event::Insert)
        } else if let SqlStatement::UpdateSearched { table, .. } = stmt {
            (table, Event::Update)
        } else if let SqlStatement::DeleteSearched { table, .. } = stmt {
            (table, Event::Delete)
        } else {
            return refused_instead(cx.st, stmt, &self.default_schema()).map(|()| None);
        };
        let name = qualify(table, &self.default_schema());
        let (Some(view), Some(trigger)) = (cx.st.catalog.view(&name).cloned(), instead_trigger(cx.st, &name, event)) else {
            return Ok(None);
        };
        let transitions = if let SqlStatement::Insert(i) = stmt {
            self.inserted_rows(cx, (&view, i), params)?
        } else if let SqlStatement::UpdateSearched { set, where_clause, .. } = stmt {
            self.changed_rows(cx, (&view, set), (where_clause.as_ref(), params))?
        } else if let SqlStatement::DeleteSearched { where_clause, .. } = stmt {
            self.changed_rows(cx, (&view, &[]), (where_clause.as_ref(), params))?
        } else {
            return Ok(None);
        };
        let count = transitions.len();
        self.execute_trigger(cx, &trigger, transitions)?;
        Ok(Some(count))
    }

    /// [ISO/IEC 9075-2:2023 §15.13, §15.16] The transitions of an UPDATE
    /// of `view` by `set`, or of a DELETE if `set` is empty, of the rows
    /// `where_clause` chooses, bound with `params`.
    fn changed_rows(
        &self,
        cx: StatementContext<'_>,
        (view, set): (&ViewDesc, &[SetClause]),
        (where_clause, params): (Option<&SearchCondition>, &Params),
    ) -> Result<HeldRows<(RowId, Row, Row)>> {
        let user = self.auth().clone();
        let columns = set_columns(view, set)?;
        let wanted = if set.is_empty() {
            Actions { delete: true, select: where_clause.is_some(), ..Actions::default() }
        } else {
            Actions { update: columns.iter().copied().collect(), select: where_clause.is_some(), ..Actions::default() }
        };
        held(cx.st, &user, view, &wanted)?;
        let rows = view_rows(cx, view)?;
        let mut items: Vec<SelectSublist> = view.columns.iter().map(|c| item(column(&c.name))).collect();
        items.extend(set.iter().filter_map(|s| if let UpdateSource::Value(v) = &s.value { Some(item(v.clone())) } else { None }));
        let spec = query(&view.name.ident, SelectList::Values(items), where_clause.cloned());
        let over = params.clone().with_transitions(Arc::new(read_as(view, rows)));
        let bound = self.bound(cx.st, &over, |b| bind_select(b, &spec))?;
        let width = view.columns.len();
        let mut out = HeldRows::new(cx.st.list_space());
        for read in run(cx, bound.plan)? {
            let read = read?;
            let old: Row = read.get(..width).map(<[Value]>::to_vec).unwrap_or_default();
            let new =
                if set.is_empty() { Row::new() } else { replaced(view, &old, (set, &columns), read.get(width..).unwrap_or_default())? };
            out.push((0, old, new))?;
        }
        Ok(out)
    }

    /// [ISO/IEC 9075-2:2023 §15.10] The transitions of the INSERT `i` into
    /// `view`: each row its query gives, in the view's columns, the null
    /// value in each it does not name.
    fn inserted_rows(
        &self,
        cx: StatementContext<'_>,
        (view, i): (&ViewDesc, &InsertStatement),
        params: &Params,
    ) -> Result<HeldRows<(RowId, Row, Row)>> {
        let user = self.auth().clone();
        let named: Vec<usize> = match &i.columns {
            None => (0..view.columns.len()).collect(),
            Some(list) => list.iter().map(|c| column_of(view, &c.name)).collect::<Result<_>>()?,
        };
        held(cx.st, &user, view, &Actions { insert: named.iter().copied().collect(), ..Actions::default() })?;
        let width = view.columns.len();
        let mut out = HeldRows::new(cx.st.list_space());
        let InsertSource::Query(q) = &i.source else {
            out.push((0, Row::new(), vec![Value::Null; width]))?;
            return Ok(out);
        };
        let objects: Vec<ObjectColumn> = named
            .iter()
            .filter_map(|&k| view.columns.get(k))
            .map(|c| ObjectColumn { desc: c.clone(), default: Value::Null, draws: None })
            .collect();
        let bound = self.bound(cx.st, params, |b| bind_insert_query(b, q, &objects))?;
        for read in run(cx, bound.plan)? {
            let mut new = vec![Value::Null; width];
            for (&k, v) in named.iter().zip(read?) {
                let (Some(slot), Some(c)) = (new.get_mut(k), view.columns.get(k)) else { continue };
                *slot = assign(v, &c.data_type, STORED)?;
            }
            out.push((0, Row::new(), new))?;
        }
        Ok(out)
    }
}

/// [ISO/IEC 9075-2:2023 §15.10, §15.13, §15.16] The INSTEAD OF trigger of
/// view `name` for `event`, if it has one [§11.49 SR5a: one at most].
fn instead_trigger(st: &State, name: &QualifiedName, event: Event) -> Option<TriggerDesc> {
    let of = |t: &&TriggerDesc| match (&t.event, event) {
        (TriggerEvent::Insert, Event::Insert) | (TriggerEvent::Delete, Event::Delete) | (TriggerEvent::Update(_), Event::Update) => true,
        (TriggerEvent::Insert, Event::Delete | Event::Update)
        | (TriggerEvent::Delete, Event::Insert | Event::Update)
        | (TriggerEvent::Update(_), Event::Insert | Event::Delete) => false,
    };
    st.catalog.triggers_on(name).into_iter().filter(|t| t.time == ActionTime::InsteadOf).find(of).cloned()
}

/// \[DESIGN\] That `stmt`, if a MERGE or a positioned UPDATE or DELETE,
/// changes no view with an INSTEAD OF trigger, its names in `schema`:
/// those are not rerouted to the trigger yet, and are refused rather than
/// written through the view.
fn refused_instead(st: &State, stmt: &SqlStatement, schema: &str) -> Result<()> {
    let table = match stmt {
        SqlStatement::Merge(m) => &m.target,
        SqlStatement::UpdatePositioned { table, .. } | SqlStatement::DeletePositioned { table, .. } => table,
        SqlStatement::Insert(_)
        | SqlStatement::UpdateSearched { .. }
        | SqlStatement::DeleteSearched { .. }
        | SqlStatement::Close(_)
        | SqlStatement::SetCollation(_)
        | SqlStatement::Commit
        | SqlStatement::Truncate { .. }
        | SqlStatement::AlterTable { .. }
        | SqlStatement::Drop { .. }
        | SqlStatement::Schema { .. }
        | SqlStatement::Fetch { .. }
        | SqlStatement::GetDiagnostics(_)
        | SqlStatement::Dynamic(_)
        | SqlStatement::Execute(_)
        | SqlStatement::DeclareTable(_)
        | SqlStatement::Open(_)
        | SqlStatement::Rollback
        | SqlStatement::Select { .. }
        | SqlStatement::SetTransaction(_)
        | SqlStatement::SetCatalog(_)
        | SqlStatement::SetSchema(_)
        | SqlStatement::SetTimeZone(_)
        | SqlStatement::SetSessionAuthorization(_)
        | SqlStatement::Connection(_)
        | SqlStatement::SetSetting { .. }
        | SqlStatement::SetDatabase { .. }
        | SqlStatement::Index(_)
        | SqlStatement::User(_)
        | SqlStatement::Served(_)
        | SqlStatement::SetConstraints { .. } => return Ok(()),
    };
    let name = qualify(table, schema);
    if st.catalog.triggers_on(&name).iter().any(|t| t.instead()) {
        let msg = format!("[DESIGN] {name} has an INSTEAD OF trigger, which a MERGE or a positioned UPDATE or DELETE does not execute yet");
        return Err(SqlError::new(Code::Unsupported, msg).about(Subject::table(name.parts())));
    }
    Ok(())
}

/// That `user` holds `wanted` on `view`.
fn held(st: &State, user: &str, view: &ViewDesc, wanted: &Actions) -> Result<()> {
    let (holds, _) = applicable(&*st.catalog, user, &view.name);
    let names: Vec<String> = view.columns.iter().map(|c| c.name.clone()).collect();
    let Some(lacked) = holds.missing(wanted, &names) else { return Ok(()) };
    let msg = format!("[ISO/IEC 9075-2:2023 §14.9 AR1, §14.11 AR1, §14.14 AR1] {user} lacks {lacked} on {}", view.name);
    Err(SqlError::new(Code::Privilege, msg).about(Subject::table(view.name.parts())))
}

/// The position of column `name` of `view`.
fn column_of(view: &ViewDesc, name: &str) -> Result<usize> {
    view.columns
        .iter()
        .position(|c| c.name == name)
        .ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §14.11 SR10] view {} has no column {name}", view.name)))
}

/// [ISO/IEC 9075-2:2023 §14.15 SR2, SR3] The columns of `view` `set`
/// assigns, each once.
fn set_columns(view: &ViewDesc, set: &[SetClause]) -> Result<Vec<usize>> {
    let mut columns = Vec::with_capacity(set.len());
    for s in set {
        if s.element.is_some() {
            return Err(SqlError::new(
                Code::Unsupported,
                "[DESIGN] an INSTEAD OF trigger's view takes no element of an array as an update target",
            ));
        }
        let k = column_of(view, &s.column)?;
        if columns.contains(&k) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §14.15 SR3] column {} is assigned twice", s.column)));
        }
        columns.push(k);
    }
    Ok(columns)
}

/// `old`, each of `columns` that `set` assigns given its value: the next
/// of `values` of a value expression, else the null value, a view's column
/// having no default [§11.5].
fn replaced(view: &ViewDesc, old: &Row, (set, columns): (&[SetClause], &[usize]), values: &[Value]) -> Result<Row> {
    let mut new = old.clone();
    let mut values = values.iter();
    for (s, &k) in set.iter().zip(columns) {
        let v = match &s.value {
            UpdateSource::Value(_) => values.next().cloned().unwrap_or(Value::Null),
            UpdateSource::Null | UpdateSource::Default => Value::Null,
        };
        let (Some(slot), Some(c)) = (new.get_mut(k), view.columns.get(k)) else { continue };
        *slot = assign(v, &c.data_type, STORED)?;
    }
    Ok(new)
}

/// The rows of `view`, as its owner reads them.
fn view_rows(cx: StatementContext<'_>, view: &ViewDesc) -> Result<HeldRows<Row>> {
    let owner = owner_of(&*cx.st.catalog, &view.name.auth).to_owned();
    let named = TableName { auth: Some(view.name.auth.clone()), ident: view.name.ident.clone(), at: 0 };
    let spec = QuerySpecification { distinct: false, select_list: SelectList::Star, table_expression: from(named, None) };
    let params = Params::new();
    let bound = bind_select(Context::new(cx.st, &owner, &params), &spec)?;
    let mut rows = HeldRows::new(cx.st.list_space());
    for read in run(cx, bound.plan)? {
        rows.push(read?)?;
    }
    Ok(rows)
}

/// `rows` of `view`, read as a transition table of its name and columns.
fn read_as(view: &ViewDesc, rows: HeldRows<Row>) -> Transitions {
    let table = TransitionRows::with(rows);
    let columns: Vec<ColumnDesc> = view.columns.clone();
    Transitions {
        subject: view.name.clone(),
        columns,
        old_row: None,
        new_row: None,
        old_table: Some((view.name.ident.clone(), table)),
        new_table: None,
        horizon: Horizon::default(),
        assigns: false,
    }
}

/// The rows of `logical`, run in `cx`, the tables it reads locked.
fn run(cx: StatementContext<'_>, logical: LogicalPlan) -> Result<crate::exec::RowIter> {
    let (optimized, _, physical) = plan(cx.st, logical);
    for t in optimized.tables() {
        cx.st.read_lock(cx.tx, &t)?;
    }
    Ok(execute(physical, cx.st, cx.tx.id))
}

/// SELECT `list` FROM `table` WHERE `condition`, of the table `table`
/// names unqualified.
fn query(table: &str, list: SelectList, condition: Option<SearchCondition>) -> QuerySpecification {
    let named = TableName { auth: None, ident: table.to_owned(), at: 0 };
    QuerySpecification { distinct: false, select_list: list, table_expression: from(named, condition) }
}

/// FROM `table` WHERE `condition`.
fn from(table: TableName, where_clause: Option<SearchCondition>) -> TableExpression {
    let reference = TableReference::Table(NamedTable { table, correlation: None, columns: None });
    TableExpression { from: vec![reference], where_clause, group_by: None, having: None, window: Vec::new() }
}

/// A select list item of `value`.
fn item(value: ValueExpression) -> SelectSublist {
    SelectSublist::Derived(SelectItem { value, alias: None, at: 0 })
}

/// A reference to column `name`.
fn column(name: &str) -> ValueExpression {
    ValueExpression::Column(ColumnSpecification { qualifier: None, column: name.to_owned(), catalog: None, at: 0 })
}
