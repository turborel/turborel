//! \[EXTENSION\] [GS §10.1] The graph a MATCH reads, and the label of each
//! of its variables: of the tables of the FROM clause it names, SYS_GRAPH
//! of their schema if all are its element tables [GS §4.2], else the one
//! graph whose element tables they all are. A variable of table T is
//! labeled by a label that only T's element tables carry, so it binds
//! T's rows alone.

use std::collections::BTreeMap;

use super::Bound;
use crate::ast::{LabelExpression, PathExpression, PathFactor, PathPattern, PathPrimary, PathTerm};
use crate::binder::Context;
use crate::catalog::{ElementKind, GraphDesc, QualifiedName, SYS_GRAPH};
use crate::error::{Code, Result, SqlError};
use crate::value::DataType;

/// A MATCH's variables: what each stands for, and whether it is an edge.
pub(super) type Vars = BTreeMap<String, (Bound, bool)>;

/// The kind of element table a variable of an edge, if `edge`, binds.
fn kind(edge: bool) -> ElementKind {
    if edge { ElementKind::Edge } else { ElementKind::Vertex }
}

/// The table variable `bound` stands for, if any.
fn table(bound: &Bound) -> Option<&QualifiedName> {
    match bound {
        Bound::Table(t) | Bound::Path(t) => Some(t),
        Bound::End => None,
    }
}

/// Whether the tables of `vars` are all element tables of `g`, each of
/// its variable's kind.
fn covers(g: &GraphDesc, vars: &Vars) -> bool {
    vars.values().all(|(b, edge)| table(b).is_none_or(|t| g.of_table(t).iter().any(|e| e.kind == kind(*edge))))
}

/// The graph of `vars`, and the label of each variable of a table.
///
/// # Errors
///
/// SQLCODE -103 if the tables are of no graph, or of several and not of
/// one SYS_GRAPH; or a table's element tables have no label of their own.
pub(super) fn resolved(cx: Context<'_>, vars: &Vars, at: usize) -> Result<(QualifiedName, BTreeMap<String, String>)> {
    let graphs: Vec<&GraphDesc> = cx.db.catalog.graphs().into_iter().filter(|g| covers(g, vars)).collect();
    let native =
        graphs.iter().find(|g| g.name.ident == SYS_GRAPH && vars.values().filter_map(|(b, _)| table(b)).all(|t| t.auth == g.name.auth));
    let graph = match (native, graphs.as_slice()) {
        (Some(g), _) | (None, [g]) => *g,
        (None, []) => return Err(SqlError::at(Code::SyntaxRule, at, "[GS §10.1] the tables MATCH names are of no one property graph")),
        (None, _) => {
            let msg = "[GS §10.1] the tables MATCH names are of several property graphs; use GRAPH_TABLE to name one";
            return Err(SqlError::at(Code::SyntaxRule, at, msg));
        }
    };
    let mut labels = BTreeMap::new();
    for (name, (bound, edge)) in vars {
        let Some(t) = table(bound) else { continue };
        labels.insert(name.clone(), own_label(graph, t, kind(*edge), at)?);
    }
    Ok((graph.name.clone(), labels))
}

/// A label of `g` that its element tables of `t`, of kind `k`, carry and
/// no other element table of that kind does.
fn own_label(g: &GraphDesc, t: &QualifiedName, k: ElementKind, at: usize) -> Result<String> {
    let mine: Vec<_> = g.of_table(t).into_iter().filter(|e| e.kind == k).collect();
    let own = mine.iter().flat_map(|e| &e.labels).find(|l| g.labeled(&l.name).iter().all(|e| e.table == *t || e.kind != k));
    let msg = || format!("[GS §10.1] no label of graph {} is of table {t} alone", g.name);
    own.map(|l| l.name.clone()).ok_or_else(|| SqlError::at(Code::SyntaxRule, at, msg()))
}

/// Labels each element pattern of `paths` whose variable `labels` names,
/// as it says.
pub(super) fn label(paths: &mut [PathPattern], labels: &BTreeMap<String, String>) {
    for p in paths {
        label_terms(&mut p.expression, labels);
    }
}

/// Labels the element patterns of each term of `e`, as `label` does.
fn label_terms(e: &mut PathExpression, labels: &BTreeMap<String, String>) {
    let terms: Vec<&mut PathTerm> = match e {
        PathExpression::Term(t) => vec![t],
        PathExpression::Union(ts) | PathExpression::Alternation(ts) => ts.iter_mut().collect(),
    };
    for t in terms {
        label_factors(&mut t.factors, labels);
    }
}

/// Labels the element patterns of `factors`, as `label` does.
fn label_factors(factors: &mut [PathFactor], labels: &BTreeMap<String, String>) {
    for f in factors {
        let element = match &mut f.primary {
            PathPrimary::Node(e) => e,
            PathPrimary::Edge(e) => &mut e.filler,
            PathPrimary::Parenthesized(p) => {
                label_terms(&mut p.expression, labels);
                continue;
            }
        };
        let name = element.variable.as_ref().map(|v| v.name.as_str());
        if let Some(l) = name.and_then(|n| labels.get(n))
            && element.label.is_none()
        {
            element.label = Some(LabelExpression::Label(l.clone(), element.at));
        }
    }
}

/// The key of graph `graph`'s element tables of table `table`, of an edge
/// if `edge`: each column's name and type, in order.
///
/// # Errors
///
/// SQLCODE -103 if the graph or table is not defined.
pub(super) fn key(
    cx: Context<'_>,
    (graph, table, edge): (&QualifiedName, &QualifiedName, bool),
    at: usize,
) -> Result<Vec<(String, DataType)>> {
    let missing = || SqlError::at(Code::SyntaxRule, at, format!("[GS §10.1] table {table} is no element table of graph {graph}"));
    let g = cx.db.catalog.graph(graph).ok_or_else(missing)?;
    let element = g.of_table(table).into_iter().find(|e| e.kind == kind(edge)).ok_or_else(missing)?;
    let desc = cx.db.catalog.table(table).ok_or_else(missing)?;
    let mut out = Vec::new();
    for k in &element.key {
        let column = desc.column(k).and_then(|i| desc.columns.get(i)).ok_or_else(missing)?;
        out.push((k.clone(), column.data_type.clone()));
    }
    Ok(out)
}
