//! \[EXTENSION\] [GS §10.1] A query specification with MATCH in its WHERE
//! clause, bound as the query of a GRAPH_TABLE it is: the MATCH's GPML,
//! in the graph of the FROM clause's tables (`graph_match/graph.rs`),
//! each variable of a table labeled by it.
//!
//! The MATCHes that are conjuncts of the WHERE clause become one
//! GRAPH_TABLE of the FROM clause, in place of its FOR PATH tables, whose
//! columns are each WITHIN GROUP (GRAPH PATH) aggregate and the key of
//! each variable of a table; a conjunct of the WHERE clause equates that
//! key with the table's. So the tables are joined with the matches, not
//! crossed. A MATCH under OR or NOT is the IN predicate of the tables'
//! keys and a GRAPH_TABLE's. LAST_NODE ( x ) = LAST_NODE ( y ) is SAME of
//! the two end nodes, in the postfilter.

mod graph;
mod prefilter;

use std::collections::BTreeMap;

use super::super::Context;
use crate::ast::{
    ColumnSpecification, GraphColumn, GraphPattern, GraphPredicate, GraphReference, GraphRows, GraphTable, GraphVariable, Literal,
    MatchClause, PathTable, Predicate, QueryExpression, QuerySpecification, SameKind, SearchCondition, SelectItem, SelectList,
    SelectSublist, Subquery, TableName, TableReference, ValueExpression, ValueSpecification,
};
use crate::catalog::{EDGE_ID, NODE_ID, QualifiedName};
use crate::error::{Code, Result, SqlError};
use crate::parser::{Declared, MATCH_TABLE, checked_graph_table, element_key, exposure};
use crate::value::{CompOp, Decimal};

/// \[EXTENSION\] [GS §10.1] The error of a MATCH elsewhere than in a WHERE
/// clause.
pub(crate) fn misplaced(m: &MatchClause) -> SqlError {
    SqlError::at(Code::SyntaxRule, m.at, "[GS §10.1] MATCH is a predicate of a WHERE clause alone")
}

/// What a variable of a MATCH stands for.
#[derive(Debug, Clone)]
enum Bound {
    /// A table of the FROM clause, exposed by the name, read with each
    /// match by its key.
    Table(QualifiedName),
    /// A FOR PATH table: a group of the match.
    Path(QualifiedName),
    /// The last node of a SHORTEST_PATH pattern.
    End,
}

/// Each key column of a MATCH's tables, with the GRAPH_TABLE's column of
/// it.
type Pins = Vec<(ValueExpression, ValueExpression)>;

/// A MATCH's variables, as the query sees them.
struct Scope<'a> {
    /// The binder's context.
    cx: Context<'a>,
    /// The FROM clause, but its FOR PATH tables.
    from: &'a [TableReference],
    /// The FOR PATH tables.
    paths: &'a [PathTable],
}

/// `spec` with each MATCH of its WHERE clause a GRAPH_TABLE, if it has
/// one.
///
/// # Errors
///
/// SQLCODE -103 if a MATCH names no table of the FROM clause, or its
/// tables are of no graph or of several; as `checked_graph_table`.
pub(in crate::binder) fn lowered(cx: Context<'_>, spec: &QuerySpecification) -> Result<Option<QuerySpecification>> {
    let Some(w) = &spec.table_expression.where_clause else { return Ok(None) };
    if !has_match(w) {
        return Ok(None);
    }
    let mut spec = spec.clone();
    let (matches, rest) = split(spec.table_expression.where_clause.take());
    let paths: Vec<PathTable> = matches.iter().flat_map(|m| m.path_tables.iter().cloned()).collect();
    let mut from = std::mem::take(&mut spec.table_expression.from);
    from.retain(|t| !t.named().is_some_and(|n| paths.iter().any(|p| p.name == exposed(n))));
    let mut conjuncts = Vec::new();
    let mut table = None;
    {
        let scope = Scope { cx, from: &from, paths: &paths };
        if let Some(joined) = merged(matches) {
            let (t, pins) = scope.graph_table(joined, &rest)?;
            conjuncts.extend(pins.into_iter().map(|(outer, inner)| equal(outer, inner)));
            table = Some(t);
        }
        for c in rest {
            conjuncts.push(scope.nested(c)?);
        }
    }
    if let Some(t) = table {
        if let SelectList::Star = spec.select_list {
            spec.select_list = SelectList::Values(star(&from)?);
        }
        from.push(TableReference::Graph(Box::new(t)));
    }
    spec.table_expression.from = from;
    spec.table_expression.where_clause = conjuncts.into_iter().reduce(|a, b| SearchCondition::And(Box::new(a), Box::new(b)));
    Ok(Some(spec))
}

/// The name a FROM clause's table `t` is exposed by.
fn exposed(t: &crate::ast::NamedTable) -> String {
    t.correlation.clone().unwrap_or_else(|| t.table.ident.clone())
}

/// Whether `c` has a MATCH, at any depth of its conditions.
fn has_match(c: &SearchCondition) -> bool {
    match c {
        SearchCondition::Or(a, b) | SearchCondition::And(a, b) => has_match(a) || has_match(b),
        SearchCondition::Not(a) | SearchCondition::Is(a, ..) => has_match(a),
        SearchCondition::Predicate(p, _) => matches!(**p, Predicate::GraphMatch(_)),
        SearchCondition::Predicand(v, _) => condition_of(v).is_some_and(has_match),
    }
}

/// The search condition `v` is, parenthesized, if it is one.
fn condition_of(v: &ValueExpression) -> Option<&SearchCondition> {
    if let ValueExpression::Nested(inner) = v {
        return condition_of(inner);
    }
    if let ValueExpression::Boolean(c) = v { Some(c) } else { None }
}

/// The MATCHes that are conjuncts of `c`, and its other conjuncts.
fn split(c: Option<SearchCondition>) -> (Vec<MatchClause>, Vec<SearchCondition>) {
    let (mut matches, mut rest) = (Vec::new(), Vec::new());
    let mut todo: Vec<SearchCondition> = c.into_iter().collect();
    while let Some(next) = todo.pop() {
        match next {
            SearchCondition::And(a, b) => {
                todo.push(*b);
                todo.push(*a);
            }
            SearchCondition::Predicate(p, at) => {
                let p = *p;
                if let Predicate::GraphMatch(m) = p {
                    matches.push(*m);
                } else {
                    rest.push(SearchCondition::Predicate(Box::new(p), at));
                }
            }
            other @ (SearchCondition::Or(..) | SearchCondition::Not(_) | SearchCondition::Is(..) | SearchCondition::Predicand(..)) => {
                rest.push(other);
            }
        }
    }
    (matches, rest)
}

/// `matches` as one MATCH, its path patterns and end comparisons all of
/// theirs, if there is one.
fn merged(matches: Vec<MatchClause>) -> Option<MatchClause> {
    matches.into_iter().reduce(|mut a, b| {
        a.paths.extend(b.paths);
        a.ends.extend(b.ends);
        a.path_tables.extend(b.path_tables);
        a.measures.extend(b.measures);
        a
    })
}

/// The column `column` of the table exposed as `table`, at `at`.
fn column(table: &str, column: &str, at: usize) -> ValueExpression {
    let qualifier = Some(TableName { auth: None, ident: table.to_owned(), at });
    ValueExpression::Column(ColumnSpecification { qualifier, column: column.to_owned(), catalog: None, at })
}

/// `a` = `b`.
fn equal(a: ValueExpression, b: ValueExpression) -> SearchCondition {
    SearchCondition::Predicate(Box::new(Predicate::Comparison(vec![a], CompOp::Eq, vec![b])), 0)
}

/// [ISO/IEC 9075:1992 §7.9 SR4] SELECT * of `from`: each table's columns,
/// as `<qualifier>`.* names them, so the GRAPH_TABLE's are not among them.
///
/// # Errors
///
/// SQLCODE -900 of a table reference of no one name.
fn star(from: &[TableReference]) -> Result<Vec<SelectSublist>> {
    let mut items = Vec::new();
    let mut todo: Vec<&TableReference> = from.iter().rev().collect();
    while let Some(t) = todo.pop() {
        let name = match t {
            TableReference::Table(n) => {
                TableName { auth: n.correlation.is_none().then(|| n.table.auth.clone()).flatten(), ident: exposed(n), at: n.table.at }
            }
            TableReference::Derived(d) => TableName { auth: None, ident: d.correlation.clone(), at: d.query.at },
            TableReference::Joined(j) => {
                todo.extend([&j.right, &j.left]);
                continue;
            }
            TableReference::Unnest(_)
            | TableReference::Recognized(_)
            | TableReference::Polymorphic(_)
            | TableReference::Json(_)
            | TableReference::Graph(_)
            | TableReference::Hybrid(_) => {
                return Err(SqlError::new(
                    Code::Unsupported,
                    "[GS §10.1] SELECT * of a MATCH over this FROM clause is not supported; name the columns",
                ));
            }
        };
        items.push(SelectSublist::All(name));
    }
    Ok(items)
}

impl Scope<'_> {
    /// `c`, each MATCH in it the IN predicate of the keys of its tables
    /// and a GRAPH_TABLE's.
    fn nested(&self, c: SearchCondition) -> Result<SearchCondition> {
        Ok(match c {
            SearchCondition::Or(a, b) => SearchCondition::Or(Box::new(self.nested(*a)?), Box::new(self.nested(*b)?)),
            SearchCondition::And(a, b) => SearchCondition::And(Box::new(self.nested(*a)?), Box::new(self.nested(*b)?)),
            SearchCondition::Not(a) => SearchCondition::Not(Box::new(self.nested(*a)?)),
            SearchCondition::Is(a, t, not) => SearchCondition::Is(Box::new(self.nested(*a)?), t, not),
            SearchCondition::Predicate(p, at) => {
                let p = *p;
                if let Predicate::GraphMatch(m) = p { self.in_predicate(*m, at)? } else { SearchCondition::Predicate(Box::new(p), at) }
            }
            SearchCondition::Predicand(v, at) => match condition_of(&v).filter(|inner| has_match(inner)).cloned() {
                Some(inner) => self.nested(inner)?,
                None => SearchCondition::Predicand(v, at),
            },
        })
    }

    /// MATCH `m`, not a conjunct of the WHERE clause, as an IN predicate.
    fn in_predicate(&self, m: MatchClause, at: usize) -> Result<SearchCondition> {
        let (table, pins) = self.graph_table(m, &[])?;
        if table.columns.len() != pins.len() {
            return Err(SqlError::at(
                Code::SyntaxRule,
                at,
                "[GS §10.1] a MATCH under OR or NOT has no FOR PATH table nor GRAPH PATH aggregate",
            ));
        }
        let (value, selected): (Vec<_>, Vec<_>) = pins.into_iter().unzip();
        let items = selected.into_iter().map(|v| SelectSublist::Derived(SelectItem { value: v, alias: None, at })).collect();
        let te = crate::ast::TableExpression {
            from: vec![TableReference::Graph(Box::new(table))],
            where_clause: None,
            group_by: None,
            having: None,
            window: Vec::new(),
        };
        let spec = QuerySpecification { distinct: false, select_list: SelectList::Values(items), table_expression: te };
        let subquery = Box::new(Subquery { query: QueryExpression::Spec(Box::new(spec)), at });
        Ok(SearchCondition::Predicate(Box::new(Predicate::InSubquery { value, subquery, not: false }), at))
    }

    /// What variable `name` of a MATCH stands for.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is no table of the FROM clause.
    fn bound(&self, name: &str, at: usize) -> Result<Bound> {
        if name.starts_with(crate::parser::LAST_NODE_PREFIX) {
            return Ok(Bound::End);
        }
        let qualify = |t: &TableName| crate::binder::qualify(t, self.cx.schema);
        if let Some(p) = self.paths.iter().find(|p| p.name == name) {
            return Ok(Bound::Path(qualify(&p.table)));
        }
        let mut todo: Vec<&TableReference> = self.from.iter().collect();
        while let Some(t) = todo.pop() {
            if let TableReference::Joined(j) = t {
                todo.extend([&j.left, &j.right]);
            }
            if let Some(n) = t.named().filter(|n| exposed(n) == name) {
                return Ok(Bound::Table(qualify(&n.table)));
            }
        }
        Err(SqlError::at(Code::SyntaxRule, at, format!("[GS §10.1] MATCH names {name}, no table of the FROM clause")))
    }

    /// MATCH `m` as a GRAPH_TABLE, each of its tables' conditions of
    /// `conjuncts` tested as its elements are bound; and each key column
    /// of its tables with the GRAPH_TABLE's column of it.
    fn graph_table(&self, m: MatchClause, conjuncts: &[SearchCondition]) -> Result<(GraphTable, Pins)> {
        let at = m.at;
        let mut pattern =
            GraphPattern { mode: None, paths: m.paths, keep: None, where_clause: ends(&m.ends, at), segments: Vec::new(), at };
        let mut vars = BTreeMap::new();
        for (name, e) in exposure(&pattern)? {
            let edge = match e.kind {
                Declared::Vertex => false,
                Declared::Edge => true,
                Declared::Path => continue,
            };
            vars.insert(name.clone(), (self.bound(&name, e.at)?, edge));
        }
        let (graph, labels) = graph::resolved(self.cx, &vars, at)?;
        graph::label(&mut pattern.paths, &labels);
        let tables =
            vars.iter().filter_map(|(n, (b, _))| if let Bound::Table(t) = b { Some((n.clone(), t.clone())) } else { None }).collect();
        if let Some(g) = self.cx.db.catalog.graph(&graph) {
            prefilter::place(&mut pattern.paths, prefilter::prefilters(conjuncts, &tables, (g, &labels)));
        }
        let mut columns: Vec<GraphColumn> =
            m.measures.into_iter().map(|g| GraphColumn::Value { value: g.value, name: Some(g.name), at: g.at }).collect();
        let pins = self.keys(&vars, &graph, &mut columns, at)?;
        if columns.is_empty() {
            let one = ValueExpression::Value(ValueSpecification::Literal(Literal::Exact(Decimal::new(1, 0))));
            columns.push(GraphColumn::Value { value: one, name: Some("#1".to_owned()), at });
        }
        let graph = GraphReference::Named(TableName { auth: Some(graph.auth), ident: graph.ident, at });
        let correlation = Some(MATCH_TABLE.to_owned());
        let table = GraphTable {
            graph,
            pattern,
            rows: GraphRows::Match,
            columns,
            correlation,
            system_time: None,
            options: crate::ast::GraphLimits::default(),
            hints: crate::ast::GraphHints::default(),
            at,
        };
        checked_graph_table(&table)?;
        Ok((table, pins))
    }
}

impl Scope<'_> {
    /// The key of each table variable of `vars` in `graph`, each column a
    /// column of the match added to `columns`: each with that column.
    fn keys(&self, vars: &graph::Vars, graph: &QualifiedName, columns: &mut Vec<GraphColumn>, at: usize) -> Result<Pins> {
        let mut pins = Vec::new();
        for (name, (bound, edge)) in vars {
            let Bound::Table(table) = bound else { continue };
            let key = graph::key(self.cx, (graph, table, *edge), at)?;
            let native = key.len() == 1 && key.iter().all(|(k, _)| k == NODE_ID || k == EDGE_ID);
            for (i, (k, t)) in key.into_iter().enumerate() {
                let inner = format!("#{name}.{i}");
                columns.push(GraphColumn::Value { value: element_key(name, (i, native), &t)?, name: Some(inner.clone()), at });
                pins.push((column(name, &k, at), column(MATCH_TABLE, &inner, at)));
            }
        }
        Ok(pins)
    }
}

/// [GS §10.1] The postfilter of `ends`: SAME of each pair of end nodes.
fn ends(ends: &[(String, String)], at: usize) -> Option<SearchCondition> {
    let variable = |name: &String| GraphVariable { name: name.clone(), at, anonymous: false };
    ends.iter()
        .map(|(a, b)| {
            let same = GraphPredicate::Same { kind: SameKind::Same, elements: vec![variable(a), variable(b)] };
            SearchCondition::Predicate(Box::new(Predicate::Graph(Box::new(same))), at)
        })
        .reduce(|a, b| SearchCondition::And(Box::new(a), Box::new(b)))
}
