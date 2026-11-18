//! \[EXTENSION\] The element variables of a GRAPH_TABLE's graph pattern
//! [GS §6.6], in the order they are first declared, each with the
//! element tables it may bind: those of its kind whose labels satisfy the
//! label expression of any element pattern it is declared in [GS §6.2],
//! each of which still tests its own. A variable of
//! the rows clause, ONE ROW PER VERTEX or STEP, is one more [GS §8 R8.3].
//!
//! Each is a FROM item of its own in the scope its expressions are bound
//! in: its properties, each of the union of the types its element tables
//! give it [GS §5 R5.6], then its hidden columns (`logical/graph.rs`).

use super::super::FromItem;
use super::super::graph_properties::property_type;
use super::elements::TableInfo;
use super::functions::label_holds;
use super::grf;
use super::scope::ScopeVar;
use crate::ast::{GraphPattern, LabelExpression, PathExpression, PathPrimary, PathTerm};
use crate::catalog::{ColumnDesc, QualifiedName, TableDesc};
use crate::error::Result;
use crate::logical::{EDGE_COLUMNS, VERTEX_COLUMNS, identity_type};
use crate::value::DataType;

/// The names of the hidden columns of a vertex variable.
const VERTEX_HIDDEN: [&str; VERTEX_COLUMNS] = ["$id", "$table"];
/// The names of the hidden columns of an edge variable.
const EDGE_HIDDEN: [&str; EDGE_COLUMNS] = ["$id", "$table", "$source", "$destination", "$directed"];

/// An element variable declared.
pub(super) struct Declared {
    /// Its name.
    pub(super) name: String,
    /// Whether it binds edges.
    pub(super) edge: bool,
    /// The label expression of each element pattern it is declared in,
    /// if one is written.
    pub(super) labels: Vec<Option<LabelExpression>>,
    /// Where it is first declared.
    pub(super) at: usize,
}

/// The element variables of `pattern`, then `extra`, each a name and
/// whether it binds edges.
///
/// # Errors
///
/// SQLCODE -103 if one is declared both a vertex's and an edge's.
pub(super) fn declared(pattern: &GraphPattern, extra: &[(String, bool, usize)]) -> Result<Vec<Declared>> {
    let mut out: Vec<Declared> = Vec::new();
    for path in &pattern.paths {
        expression(&path.expression, &mut out)?;
    }
    for (name, edge, at) in extra {
        if out.iter().any(|d| &d.name == name) {
            return Err(grf("GRF-0024", format!("{name} of the rows clause is declared in the graph pattern")).or_at(*at));
        }
        out.push(Declared { name: name.clone(), edge: *edge, labels: Vec::new(), at: *at });
    }
    Ok(out)
}

/// The variables of `e`, added to `out`.
fn expression(expr: &PathExpression, out: &mut Vec<Declared>) -> Result<()> {
    let terms: Vec<&PathTerm> = match expr {
        PathExpression::Term(t) => vec![t],
        PathExpression::Union(ts) | PathExpression::Alternation(ts) => ts.iter().collect(),
    };
    for factor in terms.into_iter().flat_map(|t| &t.factors) {
        let (pattern, edge) = match &factor.primary {
            PathPrimary::Node(n) => (n, false),
            PathPrimary::Edge(e) => (&e.filler, true),
            PathPrimary::Parenthesized(p) => {
                expression(&p.expression, out)?;
                continue;
            }
        };
        let Some(var) = &pattern.variable else { continue };
        let found = out.iter().position(|d| d.name == var.name);
        let at = found.unwrap_or(out.len());
        if found.is_none() {
            out.push(Declared { name: var.name.clone(), edge, labels: Vec::new(), at: var.at });
        }
        let d = out.get_mut(at).ok_or_else(|| grf("GRF-0024", "a variable is lost"))?;
        if d.edge != edge {
            return Err(grf("GRF-0024", format!("{} is declared both a vertex variable and an edge variable", var.name)).or_at(var.at));
        }
        d.labels.push(pattern.label.clone());
    }
    Ok(())
}

/// The element tables variable `d` may bind, of `tables`.
pub(super) fn candidates(d: &Declared, tables: &[TableInfo]) -> Vec<usize> {
    // Of any of its element patterns: they are joined on it, or are
    // alternatives, and each tests its own label expression.
    let any = |t: &TableInfo| d.labels.is_empty() || d.labels.iter().any(|l| l.as_ref().is_none_or(|l| label_holds(l, &t.labels)));
    let held = |t: &TableInfo| t.edge == d.edge && any(t);
    tables.iter().enumerate().filter(|(_, t)| held(t)).map(|(n, _)| n).collect()
}

/// \[GS §6.2\] That each label `e` names is a label of the graph.
///
/// # Errors
///
/// SQLCODE -103 (GRF-0012) if it names another.
pub(super) fn labels_known(e: &LabelExpression, labels: &[String]) -> Result<()> {
    match e {
        LabelExpression::Label(name, at) => {
            if labels.contains(name) {
                return Ok(());
            }
            Err(grf("GRF-0012", format!("the graph has no label {name}")).or_at(*at))
        }
        LabelExpression::Wildcard(_) => Ok(()),
        LabelExpression::Negation(inner) => labels_known(inner, labels),
        LabelExpression::Conjunction(a, b) | LabelExpression::Disjunction(a, b) => {
            labels_known(a, labels)?;
            labels_known(b, labels)
        }
    }
}

/// A column of a variable's FROM item.
fn column(name: &str, data_type: DataType) -> ColumnDesc {
    ColumnDesc { name: name.to_owned(), data_type, not_null: false, default: None, domain: None, collation: None, generated: None }
}

/// The scope variable and FROM item of `d`, which may bind the element
/// tables `tables` of `infos`, its first column at `offset`.
///
/// # Errors
///
/// SQLCODE -103 if a property's types are not union-compatible.
pub(super) fn var_item(d: &Declared, (tables, infos): (&[usize], &[TableInfo]), offset: usize) -> Result<(ScopeVar, FromItem)> {
    let mut properties: Vec<(String, Vec<DataType>)> = Vec::new();
    for info in tables.iter().filter_map(|&t| infos.get(t)) {
        for (name, t) in &info.properties {
            match properties.iter_mut().find(|(n, _)| n == name) {
                Some((_, types)) => types.push(t.clone()),
                None => properties.push((name.clone(), vec![t.clone()])),
            }
        }
    }
    let mut columns = Vec::new();
    let mut typed = Vec::new();
    for (name, types) in &properties {
        let t = property_type(types)?;
        typed.push((name.clone(), t.clone()));
        columns.push(column(name, t));
    }
    let hidden: &[&str] = if d.edge { &EDGE_HIDDEN } else { &VERTEX_HIDDEN };
    let hidden_types = [identity_type(), super::super::composite::integer(), identity_type(), identity_type(), DataType::Boolean];
    columns.extend(hidden.iter().zip(hidden_types).map(|(n, t)| column(n, t)));
    let scoped = tables
        .iter()
        .filter_map(|&t| infos.get(t).map(|i| (t, i.labels.clone(), i.properties.iter().map(|(n, _)| n.clone()).collect())))
        .collect();
    let var = ScopeVar { name: d.name.clone(), offset, properties: properties.len(), columns: typed, edge: d.edge, tables: scoped };
    let name = QualifiedName { auth: String::new(), ident: d.name.clone() };
    let desc = TableDesc {
        periods: Vec::new(),
        typed: None,
        name: name.clone(),
        columns,
        constraints: Vec::new(),
        engine: String::new(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    let mut item = FromItem::alone(name, d.at, (desc, super::super::views::Source::Table));
    item.correlation = Some(d.name.clone());
    item.offset = offset;
    Ok((var, item))
}
