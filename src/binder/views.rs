//! [X3.135-1986 §6.5] Viewed tables, where a query or a data-changing
//! statement names one. Whether a view is materialized is
//! implementor-defined [GR1]: here it is not. Its `<query expression>`
//! [ISO/IEC 9075:1992 §11.19] is bound where it is named, with the view's `<authorization
//! identifier>` [X3.135-1986 §5.4 SR2, §6.6 SR3a i], into the plan of the
//! query naming it, so the optimizer sees through it.
//!
//! An updatable view [SR3] is read as the base table its rows are derived
//! from [GR2]: its columns are columns of that table's rows, and the
//! conditions of the views between are a filter on them. So a cursor over
//! it, an index on the base table, and INSERT, UPDATE and DELETE through it
//! work as on the table itself. Any other view is its bound plan. Split
//! from binder.rs (RULES.md R-SIZE-2).

use super::query::Nesting;
use super::set_operations::bind_query_expression;
use super::statements::updatable_query;
use super::{Bound, Context, OutColumn, Purpose, Uses, qualify, rule};
use crate::ast::{QueryExpression, QuerySpecification, SelectItem, SelectList, SelectSublist, TableReference, ValueExpression};
use crate::catalog::{CheckLevels, ColumnDesc, QualifiedName, TableDesc, ViewDesc, owner_of};
use crate::db::State;
use crate::error::{Code, Result, SqlError, Subject};
use crate::logical::{Cond, Expr, LogicalPlan, and_all};
use crate::params::Params;
use crate::parser::Parser;
use crate::settings::{self, Own};
use crate::system::{self, DEFINITION_SCHEMA, INFORMATION_SCHEMA, SystemTable, SystemView, desc_of};
use crate::value::datetime::Now;

use std::sync::Arc;

/// An updatable view, read as its base table.
#[derive(Clone)]
pub(super) struct ViewBase {
    /// The base table.
    pub(super) base: TableDesc,
    /// The base table column each column of the view is.
    pub(super) map: Vec<usize>,
    /// The view's own `<where clause>`, over the base table's rows.
    pub(super) own: Option<Cond>,
    /// Its own and those of the views between, over the base table's rows.
    pub(super) cond: Option<Cond>,
    /// Those of the views between alone: of the table its query reads.
    pub(super) below: Option<Cond>,
}

/// Where the rows of a FROM item come from.
#[derive(Clone)]
pub(super) enum Source {
    /// A base table.
    Table,
    /// An updatable view, read as its base table.
    Base(Box<ViewBase>),
    /// Another view, or a derived table: its bound plan.
    Derived(LogicalPlan),
    /// The common columns of a NATURAL or USING join (`binder/from.rs`).
    Common,
    /// A system table of INFORMATION_SCHEMA, its rows made as it is read
    /// (`system.rs`).
    System(Arc<dyn SystemTable>),
}

/// A table or view by name: its description (a view's columns, with no
/// UNIQUE constraint or engine), and where its rows come from.
pub(super) struct Named {
    pub(super) desc: TableDesc,
    pub(super) source: Source,
}

/// An internal error: a bound view that is not the shape binding gives.
fn broken(what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {what}"))
}

/// Who reads a table or view: what for, with the settings of its session,
/// as which current user, which a view's CURRENT_USER is [ISO/IEC
/// 9075:1992 §4.26, §6.2 GR9], and whether it may read the Definition
/// Schema (`Context::definitions`).
#[derive(Clone, Copy)]
pub(super) struct Reader<'a> {
    pub(super) purpose: Purpose,
    pub(super) own: &'a Own,
    pub(super) user: &'a str,
    pub(super) definitions: bool,
    pub(super) now: Now,
}

impl<'a> Reader<'a> {
    /// The reader of a query bound with `cx`.
    pub(super) fn of(cx: &Context<'a>) -> Reader<'a> {
        Reader { purpose: cx.purpose.of_view(), own: cx.own, user: cx.user, definitions: cx.definitions, now: cx.now }
    }
}

/// The table or view `name` [X3.135-1986 §5.4 SR5], system table, or view
/// of the Information Schema, as `reader` reads it.
pub(super) fn named(db: &State, name: &QualifiedName, reader: Reader<'_>) -> Result<Named> {
    if let Some(table) = db.system.get(name) {
        if name.auth == DEFINITION_SCHEMA && !reader.definitions {
            return Err(rule(format!("[ISO/IEC 9075:1992 §21.1] the Definition Schema cannot be accessed in an SQL-statement: {name}")));
        }
        return Ok(Named { desc: desc_of(&**table), source: Source::System(Arc::clone(table)) });
    }
    if let Some(view) = system::view(name) {
        let bound = bind_system_view(db, view, reader)?;
        let desc = TableDesc { columns: described(&bound.columns), ..unconstrained(name) };
        return Ok(Named { desc, source: Source::Derived(bound.plan) });
    }
    if let Some(desc) = db.catalog.table(name) {
        return Ok(Named { desc: desc.clone(), source: Source::Table });
    }
    let undefined = || rule(format!("[X3.135-1986 §5.4 SR5] table {name} is not defined")).about(Subject::table(name.parts()));
    let view = db.catalog.view(name).ok_or_else(undefined)?;
    let desc = TableDesc { columns: view.columns.clone(), ..unconstrained(name) };
    Ok(Named { desc, source: expand(db, view, reader)? })
}

/// The description of a view `name`, its columns yet to be given: with no
/// constraint or engine.
pub(super) fn unconstrained(name: &QualifiedName) -> TableDesc {
    TableDesc {
        name: name.clone(),
        columns: Vec::new(),
        constraints: Vec::new(),
        engine: String::new(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    }
}

/// The columns of a view whose query's result has `columns`.
fn described(columns: &[OutColumn]) -> Vec<ColumnDesc> {
    let column = |c: &OutColumn| ColumnDesc {
        name: c.name.clone().unwrap_or_default(),
        data_type: c.data_type,
        not_null: c.not_null,
        default: None,
        domain: None,
    };
    columns.iter().map(column).collect()
}

/// [ISO/IEC 9075:1992 §21.2] The query of the Information Schema's `view`,
/// bound as INFORMATION_SCHEMA's, which owns it, reading the Definition
/// Schema, for `reader`, whose CURRENT_USER it shows what of.
fn bind_system_view(db: &State, view: &SystemView, Reader { purpose, own, user, now, .. }: Reader<'_>) -> Result<Bound> {
    let query = Parser::new(view.query)?.view_query()?;
    let params = Params::new();
    let cx = Context { purpose, own, user, now, definitions: true, ..Context::new(db, INFORMATION_SCHEMA, &params) };
    bind_query_expression(cx, &query, &Nesting::Query)
}

/// [ISO/IEC 9075:1992 §21.3.10] The columns of the Information Schema's
/// `view`.
///
/// # Errors
///
/// Its query does not bind: a defect.
pub(crate) fn system_view_columns(db: &State, view: &SystemView) -> Result<Vec<ColumnDesc>> {
    let reader =
        Reader { purpose: Purpose::Query, own: &settings::NONE, user: INFORMATION_SCHEMA, definitions: true, now: db.now(&settings::NONE) };
    Ok(described(&bind_system_view(db, view, reader)?.columns))
}

/// [ISO/IEC 9075:1992 §21.3.12] The columns of tables and views `view`'s
/// query references, explicitly or by `*`, once each, as its owner binds it.
///
/// # Errors
///
/// Its query does not bind.
pub(crate) fn view_uses(db: &State, view: &ViewDesc) -> Result<Vec<(QualifiedName, String)>> {
    let query = Parser::new(&view.query)?.view_query()?;
    let (params, uses) = (Params::new(), Uses::default());
    let cx = Context {
        horizon: &view.horizon,
        uses: Some(&uses),
        schema: &view.name.auth,
        ..Context::new(db, owner_of(&*db.catalog, &view.name.auth), &params)
    };
    bind_query_expression(cx, &query, &Nesting::Query)?;
    Ok(once_each(uses.into_inner()))
}

/// `uses`, each once, in the order first referenced.
pub(crate) fn once_each(uses: Vec<(QualifiedName, String)>) -> Vec<(QualifiedName, String)> {
    let mut seen = Vec::with_capacity(uses.len());
    for u in uses {
        if !seen.contains(&u) {
            seen.push(u);
        }
    }
    seen
}

/// The name column `i` of the named table or view `name` has there, if
/// it is a base table, view or system table.
pub(super) fn column_name(db: &State, name: &QualifiedName, i: usize) -> Option<String> {
    let columns = match (db.catalog.table(name), db.catalog.view(name), db.system.get(name)) {
        (Some(table), _, _) => table.columns.clone(),
        (None, Some(view), _) => view.columns.clone(),
        (None, None, Some(table)) => table.columns(),
        (None, None, None) => system::view(name).and_then(|v| system_view_columns(db, v).ok())?,
    };
    columns.into_iter().nth(i).map(|c| c.name)
}

/// The degree of `view`'s query bound now, as its owner's: for a change of
/// a table it reads, which a view that refers to what changed no longer
/// binds to, or not to as many columns (`db/alter.rs`).
///
/// # Errors
///
/// The query does not bind.
pub fn view_degree(db: &State, view: &ViewDesc) -> Result<usize> {
    let query = Parser::new(&view.query)?.view_query()?;
    let params = Params::new();
    let cx =
        Context { horizon: &view.horizon, schema: &view.name.auth, ..Context::new(db, owner_of(&*db.catalog, &view.name.auth), &params) };
    Ok(bind_query_expression(cx, &query, &Nesting::Query)?.columns.len())
}

/// [X3.135-1986 §6.5 GR1] The rows of `view`: its query, bound as its
/// owner's for `purpose`, and for an updatable view read as its base table.
fn expand(db: &State, view: &ViewDesc, Reader { purpose, own, user, now, .. }: Reader<'_>) -> Result<Source> {
    let query = Parser::new(&view.query)?.view_query()?;
    let params = Params::new();
    let cx = Context {
        purpose,
        horizon: &view.horizon,
        own,
        user,
        now,
        schema: &view.name.auth,
        ..Context::new(db, owner_of(&*db.catalog, &view.name.auth), &params)
    };
    let bound = bind_query_expression(cx, &query, &Nesting::Query)?;
    if view.updates.is_none() {
        return Ok(Source::Derived(bound.plan));
    }
    let has_where = query.spec().is_some_and(|spec| spec.table_expression.where_clause.is_some());
    let base = collapse(db, bound.plan, has_where)?;
    Ok(Source::Base(Box::new(base)))
}

/// The bound plan of an updatable view, which is its select list of
/// columns [X3.135-1986 §5.25 SR11b] over its own WHERE condition, if
/// `has_where`, over those of the views between, over a scan of the base
/// table: as a `ViewBase`.
fn collapse(db: &State, plan: LogicalPlan, has_where: bool) -> Result<ViewBase> {
    let LogicalPlan::Project { exprs, input } = plan else { return Err(broken("an updatable view is not a projection")) };
    let map = exprs
        .iter()
        .map(|e| if let Expr::Column(c) = e { Ok(*c) } else { Err(broken("an updatable view has a column that is not one")) })
        .collect::<Result<Vec<_>>>()?;
    let mut conds = Vec::new();
    let mut node = *input;
    let table = loop {
        match node {
            LogicalPlan::Filter { cond, input: below } => {
                conds.push(cond);
                node = *below;
            }
            LogicalPlan::Scan { table, .. } => break table,
            other @ (LogicalPlan::Product { .. }
            | LogicalPlan::OuterJoin { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::DefaultRow { .. }
            | LogicalPlan::Project { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Sort { .. }) => return Err(broken(&format!("an updatable view reads {}", other.explain()))),
        }
    };
    let base = db.catalog.table(&table).cloned().ok_or_else(|| broken(&format!("an updatable view reads {table}, not a table")))?;
    let own = conds.first().filter(|_| has_where).cloned();
    let below = and_all(conds.iter().skip(usize::from(has_where)).cloned().collect());
    Ok(ViewBase { base, map, own, cond: and_all(conds), below })
}

/// [X3.135-1986 §6.5 GR2] The table a data-changing statement that names a
/// table or view changes.
pub struct WriteTarget {
    /// The table or view named, as privileges and column names refer to it.
    pub named: TableDesc,
    /// The base table whose rows change.
    pub base: TableDesc,
    /// The base table column each column of `named` is.
    pub map: Vec<usize>,
    /// [ISO/IEC 9075:1992 §11.19 GR11] The views WITH CHECK OPTION it
    /// spans, which every row it inserts or updates is checked against.
    pub checks: Vec<ViewCheck>,
    /// The conditions of the views between, which the rows it changes meet.
    pub(super) cond: Option<Cond>,
    /// Where the rows of `named` come from.
    pub(super) source: Source,
}

/// The table a statement that changes the table or view `name` changes:
/// the table, or the base table of an updatable view.
///
/// # Errors
///
/// `name` is not defined [X3.135-1986 §5.4 SR5], or is a read-only table,
/// which `read_only` (e.g. "§8.7 SR2") forbids.
pub fn write_target(db: &State, name: &QualifiedName, (read_only, user): (&str, &str)) -> Result<WriteTarget> {
    if [INFORMATION_SCHEMA, DEFINITION_SCHEMA].contains(&name.auth.as_str()) {
        return Err(SqlError::new(
            Code::Privilege,
            format!("[ISO/IEC 9075:1992 §21.1] only SELECT is granted on {name}, so it cannot be updated"),
        )
        .about(Subject::table(name.parts())));
    }
    let Named { desc, source, .. } =
        named(db, name, Reader { purpose: Purpose::Query, own: &settings::NONE, user, definitions: false, now: db.now(&settings::NONE) })?;
    match &source {
        Source::Table => {
            let map = (0..desc.columns.len()).collect();
            Ok(WriteTarget { named: desc.clone(), base: desc, map, checks: Vec::new(), cond: None, source })
        }
        Source::Base(vb) => {
            let checks = spanned(db, name, (vb, user))?;
            let (base, map, cond) = (vb.base.clone(), vb.map.clone(), vb.cond.clone());
            Ok(WriteTarget { named: desc, base, map, checks, cond, source })
        }
        Source::Derived(_) | Source::Common => Err(rule(format!("[X3.135-1986 {read_only}, §4.4] {name} is a read-only table"))),
        Source::System(_) => Err(SqlError::new(
            Code::Privilege,
            format!("[ISO/IEC 9075:1992 §21.1] only SELECT is granted on {name}, so it cannot be updated"),
        )
        .about(Subject::table(name.parts()))),
    }
}

/// [ISO/IEC 9075:1992 §11.19 GR11] A view WITH CHECK OPTION that an
/// update operation spans, and what of it a row of the base table is
/// checked against, at the end of the statement \[GR12\].
#[derive(Debug, Clone)]
pub struct ViewCheck {
    /// The view.
    pub view: QualifiedName,
    /// Its `<levels clause>`.
    pub levels: CheckLevels,
    /// Its own `<where clause>`, over the base table's rows.
    pub own: Option<Cond>,
    /// Those of the views under it: whether a row appears in the table its
    /// query reads.
    pub below: Option<Cond>,
}

/// [ISO/IEC 9075:1992 §11.19 GR9, GR11] The views WITH CHECK OPTION the
/// updatable view `name`, read as `vb`, spans: itself, and each view under
/// it, down to the base table, read as `user` reads it.
fn spanned(db: &State, name: &QualifiedName, (vb, user): (&ViewBase, &str)) -> Result<Vec<ViewCheck>> {
    let reader = Reader { purpose: Purpose::Query, own: &settings::NONE, user, definitions: false, now: db.now(&settings::NONE) };
    let mut checks = Vec::new();
    let (mut at, mut read) = (name.clone(), vb.clone());
    while let Some(view) = db.catalog.view(&at) {
        if let Some(levels) = view.check_option {
            checks.push(ViewCheck { view: at.clone(), levels, own: read.own.clone(), below: read.below.clone() });
        }
        let Some((under, _)) = &view.updates else { break };
        let Named { source: Source::Base(next), .. } = named(db, under, reader)? else { break };
        (at, read) = (under.clone(), *next);
    }
    Ok(checks)
}

/// [X3.135-1986 §6.5; ISO/IEC 9075:1992 §11.19] The `<query expression>`
/// of a view, bound, and what the catalog keeps of it.
pub struct BoundView {
    /// The bound query.
    pub bound: Bound,
    /// Whether it makes a grouped view [X3.135-1986 §6.5 SR8; ISO/IEC
    /// 9075:1992 §11.19 SR7].
    pub grouped: bool,
    /// For an updatable view [X3.135-1986 §6.5 SR3]: the table its `<from
    /// clause>` names, and the column of that table each of its columns is
    /// [X3.135-1986 §6.5 GR2].
    pub updates: Option<(QualifiedName, Vec<usize>)>,
    /// The tables it names, in subqueries too.
    pub reads: Vec<QualifiedName>,
}

/// [X3.135-1986 §6.5; ISO/IEC 9075:1992 §11.19] Binds the `<query
/// expression>` of a view of the schema `cx.auth`.
///
/// # Errors
///
/// It breaks a rule of its query.
pub fn bind_view(cx: Context<'_>, query: &QueryExpression) -> Result<BoundView> {
    let bound = bind_query_expression(cx, query, &Nesting::Query)?;
    let grouped = query.spec().is_some_and(|spec| !spec.table_expression.group_by.is_empty() || spec.table_expression.having.is_some());
    let mut reads = Vec::new();
    named_tables(cx, query, &mut reads);
    // [ISO/IEC 9075:1992 §11.19 SR6, §7.10 SR4] updatable only if it is
    // an updatable <query specification>.
    let updates = match query.spec() {
        Some(spec) => updates(cx, spec, &bound.plan.subquery_tables())?,
        None => None,
    };
    Ok(BoundView { bound, grouped, updates, reads })
}

/// [X3.135-1986 §6.5 SR3, §5.25 SR11; ISO/IEC 9075:1992 §7.9 SR12] For an
/// updatable query specification, whose one table is updatable, and whose
/// leaf generally underlying table its subqueries, which read `reads`, do
/// not read \[SR12d\]: that table, and the column of it each select list
/// item is.
fn updates(cx: Context<'_>, spec: &QuerySpecification, reads: &[QualifiedName]) -> Result<Option<(QualifiedName, Vec<usize>)>> {
    let Some(r) = spec.table_expression.from.first().filter(|_| updatable_query(spec)).and_then(TableReference::named) else {
        return Ok(None);
    };
    let name = qualify(&r.table, cx.schema);
    let Named { desc, source, .. } = named(cx.db, &name, Reader { purpose: Purpose::Query, ..Reader::of(&cx) })?;
    let leaf = match &source {
        Source::Table => &name,
        Source::Base(vb) => &vb.base.name,
        Source::Derived(_) | Source::Common | Source::System(_) => return Ok(None),
    };
    if reads.contains(leaf) {
        return Ok(None);
    }
    let all = || (0..desc.columns.len()).collect::<Vec<_>>();
    let map = match &spec.select_list {
        SelectList::Star => Some(all()),
        SelectList::Values(vs) => vs
            .iter()
            .map(|v| match v {
                SelectSublist::Derived(SelectItem { value: ValueExpression::Column(cs), .. }) => desc.column(&cs.column).map(|c| vec![c]),
                SelectSublist::Derived(_) => None,
                SelectSublist::All(_) => Some(all()),
            })
            .collect::<Option<Vec<_>>>()
            .map(|m| m.concat()),
    };
    Ok(map.map(|m| (name, m)))
}

/// Adds to `out` the tables the `<from clause>`s of `query` and of its
/// subqueries name.
fn named_tables(cx: Context<'_>, query: &QueryExpression, out: &mut Vec<QualifiedName>) {
    let mut names = Vec::new();
    query.table_names(&mut names);
    out.extend(names.into_iter().map(|t| qualify(t, cx.schema)));
}
