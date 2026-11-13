//! [ISO/IEC 9075-2:2023 §7.17] The `<with clause>`, bound: each `<with
//! list element>` names the table its query gives [GR1b], which a `<table
//! reference>` in its scope that names it identifies [GR1c].
//!
//! Each element's query is bound once, where the clause is written, so its
//! names are those in scope there [SR3i], and each reference to it is a
//! derived table of that plan. A reference in a subquery deeper than the
//! clause has its outer references made again through each query between
//! (`lifted`), as a derived table written there would make them. A
//! non-recursive element sees the elements before it [SR3i i]; a
//! potentially recursive list is bound by its strata (`binder/recursive.rs`).

use std::cell::RefCell;

use super::from::{Bounds, FromItem, column_of, renamed};
use super::query::Nesting;
use super::set_operations::bind_query_expression;
use super::subquery::{Outer, capture};
use super::underlying::drawn_from;
use super::views::Source;
use super::{Bound, Context, OutColumn, rule};
use crate::ast::{NamedTable, WithElement, WithQuery};
use crate::catalog::{ColumnDesc, QualifiedName, TableDesc, Underlying};
use crate::error::{At, Code, Result, SqlError};
use crate::logical::{Expr, LogicalPlan, map_plan_exprs};

/// [ISO/IEC 9075-2:2023 §7.17 SR3i] The query names one `<with clause>`
/// has in scope, as each is bound, and those of the clauses around it.
pub(crate) struct Queries<'a> {
    /// Each bound so far, in the order written.
    named: RefCell<Vec<Query>>,
    /// The query the clause is a subquery of, if it is in one: whose
    /// outer references its elements' plans make.
    site: Option<&'a Outer<'a>>,
    /// The query names of the clauses it is in.
    parent: Option<&'a Queries<'a>>,
}

/// A query name, and the table it names.
#[derive(Clone)]
pub(super) struct Query {
    /// The `<query name>`.
    pub(super) name: String,
    /// Its columns, named by its `<with column list>` if it has one.
    pub(super) columns: Vec<ColumnDesc>,
    /// Its rows.
    pub(super) plan: LogicalPlan,
    /// What each of its columns is drawn from, if that is being found
    /// (`binder/underlying.rs`).
    pub(super) underlying: Option<Underlying>,
}

impl<'a> Queries<'a> {
    /// The query names of a clause written where `cx` binds, a subquery of
    /// `site` if it is in one, none bound yet.
    pub(super) fn new(cx: &Context<'a>, site: Option<&'a Outer<'a>>) -> Queries<'a> {
        Queries { named: RefCell::new(Vec::new()), site, parent: cx.queries }
    }

    /// The query names of a scope within `parent`'s, of the same clause,
    /// none added yet: of a recursive stratum's work tables.
    pub(super) fn within(parent: &'a Queries<'a>) -> Queries<'a> {
        Queries { named: RefCell::new(Vec::new()), site: parent.site, parent: Some(parent) }
    }

    /// The query the clause is a subquery of, if it is in one.
    pub(super) fn site(&self) -> Option<&'a Outer<'a>> {
        self.site
    }

    /// Adds `query`, now in scope.
    pub(super) fn add(&self, query: Query) {
        self.named.borrow_mut().push(query);
    }

    /// The leaf underlying tables of the query `name` names, if a clause
    /// has one of that name in scope and they are being found.
    pub(super) fn leaves_of(&self, name: &str) -> Option<Vec<QualifiedName>> {
        self.find(name).map(|(_, q)| q.underlying.map(|u| u.leaves).unwrap_or_default())
    }

    /// The query `name` names, of the innermost clause with one of that
    /// name in scope, and where that clause is.
    fn find(&self, name: &str) -> Option<(Option<&'a Outer<'a>>, Query)> {
        let here = self.named.borrow().iter().rev().find(|q| q.name == name).cloned();
        match here {
            Some(query) => Some((self.site, query)),
            None => self.parent?.find(name),
        }
    }
}

/// How the query of an element of a clause written as `nesting` says is
/// bound: a subquery of the same query, if the clause is in one.
pub(super) fn element_nesting<'n>(site: Option<&'n Outer<'n>>) -> Nesting<'n> {
    match site {
        Some(outer) => Nesting::Subquery { outer, result: None },
        None => Nesting::Query,
    }
}

/// The query a clause written as `nesting` says is a subquery of.
fn site_of<'n>(nesting: &Nesting<'n>) -> Option<&'n Outer<'n>> {
    match nesting {
        Nesting::Subquery { outer, .. } => Some(*outer),
        Nesting::Query | Nesting::Source { .. } => None,
    }
}

/// [ISO/IEC 9075-2:2023 §7.17] A `<query expression>` with a `<with
/// clause>`, bound as `nesting` says: its elements, then its body, in
/// whose scope they are. Of an INSERT, it is a `<from subquery>`, not a
/// `<from constructor>` [§14.11], so its rows take no DEFAULT.
pub(super) fn bind_with(cx: Context<'_>, w: &WithQuery, nesting: &Nesting<'_>) -> Result<Bound> {
    with_scope(cx, w, (site_of(nesting), false), |inner| match nesting {
        Nesting::Source { .. } => bind_query_expression(inner, &w.body, &Nesting::Query),
        Nesting::Query | Nesting::Subquery { .. } => bind_query_expression(inner, &w.body, nesting),
    })
}

/// `then` of the context of `cx` with the query names of `w` in scope,
/// its elements bound first, the clause a subquery of `site`'s if given,
/// and what each element's columns are drawn from found too if `drawn`
/// (`binder/underlying.rs`).
pub(super) fn with_scope<'a, T>(
    cx: Context<'a>,
    w: &WithQuery,
    (site, drawn): (Option<&'a Outer<'a>>, bool),
    then: impl FnOnce(Context<'_>) -> Result<T>,
) -> Result<T> {
    let queries = Queries::new(&cx, site);
    for (n, e) in w.elements.iter().enumerate() {
        // [SR3c] no two elements of one list have one name.
        if w.elements.iter().take(n).any(|earlier| earlier.name == e.name) {
            let msg = format!("[ISO/IEC 9075-2:2023 §7.17 SR3c] query {} is named twice in one <with list>", e.name);
            return Err(rule(msg).or_at(e.at));
        }
    }
    if w.recursive {
        super::recursive::bind_list(cx, w, (&queries, drawn))?;
    } else {
        for e in &w.elements {
            bind_element(cx, e, (&queries, drawn))?;
        }
    }
    then(Context { queries: Some(&queries), ..cx })
}

/// [ISO/IEC 9075-2:2023 §7.17 GR1b, GR2c i] The non-recursive element
/// `e`, bound with the query names of `queries` in scope, and added to
/// them; what its columns are drawn from found too if `drawn`.
pub(super) fn bind_element(cx: Context<'_>, e: &WithElement, (queries, drawn): (&Queries<'_>, bool)) -> Result<()> {
    let inner = Context { queries: Some(queries), draws: false, ..cx };
    let bound = bind_query_expression(inner, &e.query, &element_nesting(queries.site)).or_at(e.at)?;
    let underlying = if drawn { Some(drawn_from(inner, &e.query, queries.site)?) } else { None };
    let columns = element_columns(&bound.columns, e)?;
    queries.add(Query { name: e.name.clone(), columns, plan: bound.plan, underlying });
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §7.17 SR3d, GR1b] The columns of the table element
/// `e` names, its query's of `columns` named by its `<with column list>`:
/// one name for each, none twice \[SR3d ii\]. Without one, no two of its
/// query's may have one name \[SR3d i\]; a column its query does not name
/// is named by its position, as a derived table's is.
pub(super) fn element_columns(columns: &[OutColumn], e: &WithElement) -> Result<Vec<ColumnDesc>> {
    let mut descs: Vec<ColumnDesc> = columns.iter().enumerate().map(|(k, c)| column_of(k, c)).collect();
    let Some(names) = &e.columns else {
        for (n, c) in columns.iter().enumerate() {
            let Some(name) = &c.name else { continue };
            if columns.iter().take(n).any(|earlier| earlier.name.as_ref() == Some(name)) {
                let msg = format!(
                    "[ISO/IEC 9075-2:2023 §7.17 SR3d i] query {} has two columns named {name}, so it has a <with column list>",
                    e.name
                );
                return Err(rule(msg).or_at(e.at));
            }
        }
        return Ok(descs);
    };
    if names.len() != descs.len() {
        let msg = format!(
            "[ISO/IEC 9075-2:2023 §7.17 SR3d ii 2] the <with column list> of query {} names {} columns, its query has {}",
            e.name,
            names.len(),
            descs.len()
        );
        return Err(rule(msg).or_at(e.at));
    }
    renamed(&mut descs, names).map_err(|_| {
        let twice = names.iter().enumerate().find(|(n, name)| names.iter().take(*n).any(|x| x == *name)).map_or("", |(_, n)| n.as_str());
        let msg =
            format!("[ISO/IEC 9075-2:2023 §7.17 SR3d ii 1] column {twice} is named twice in the <with column list> of query {}", e.name);
        rule(msg).or_at(e.at)
    })?;
    Ok(descs)
}

/// [ISO/IEC 9075-2:2023 §7.17 GR1c] The table the query `t` names, if a
/// `<with clause>` has a query of its name in scope, as the item of a
/// `<from clause>` bound as `b` says, its first column at `offset`: a
/// derived table of the query's plan, its columns renamed by `t`'s
/// derived column list, if written.
pub(super) fn query_item(b: Bounds<'_>, t: &NamedTable, offset: usize) -> Result<Option<FromItem>> {
    if t.table.auth.is_some() {
        return Ok(None);
    }
    let Some((site, query)) = b.cx.queries.and_then(|q| q.find(&t.table.ident)) else { return Ok(None) };
    let plan = lifted(query.plan, site, b.outer)?;
    let mut columns = query.columns;
    if let Some(names) = &t.columns {
        renamed(&mut columns, names)?;
    }
    let name = QualifiedName { auth: String::new(), ident: t.table.ident.clone() };
    let desc = TableDesc {
        name: name.clone(),
        columns,
        constraints: Vec::new(),
        engine: String::new(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    Ok(Some(FromItem {
        table: name,
        at: t.table.at,
        correlation: Some(t.correlation.clone().unwrap_or_else(|| t.table.ident.clone())),
        desc,
        offset,
        source: Source::Derived(plan),
        nullable: false,
        common: Vec::new(),
        drawn: query.underlying.map(Box::new),
    }))
}

/// Whether `a` and `b` are one query to be a subquery of, or both none.
fn same_site(a: Option<&Outer<'_>>, b: Option<&Outer<'_>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => std::ptr::eq(x, y),
        (None, Some(_)) | (Some(_), None) => false,
    }
}

/// `plan`, bound as a subquery of `site`, made a derived table of a query
/// that is a subquery of `here`: each outer reference made again through
/// each query between, the outermost first, as a reference written there
/// to the same column would be (`binder/subquery.rs`).
fn lifted(mut plan: LogicalPlan, site: Option<&Outer<'_>>, here: Option<&Outer<'_>>) -> Result<LogicalPlan> {
    let mut between = Vec::new();
    let mut at = here;
    while !same_site(site, at) {
        let Some(outer) = at else {
            return Err(SqlError::new(Code::Api, "[DESIGN] a query name is referenced outside its <with clause>"));
        };
        between.push(outer);
        at = outer.binder().outer;
    }
    for outer in between.into_iter().rev() {
        plan = map_plan_exprs(plan, &mut |e| relifted(e, outer));
    }
    Ok(plan)
}

/// `e`, of the query `outer` gives the values of, with each outer
/// reference slot, of the query around that one, made a slot of the
/// subquery's own.
fn relifted(e: Expr, outer: &Outer<'_>) -> Expr {
    if let Expr::Outer(slot, name) = e {
        return capture(outer, Expr::Outer(slot, name.clone()), name);
    }
    e.map_children(&mut |child| relifted(child, outer))
}
