//! \[EXTENSION\] A GQL MATCH's graph pattern as a GRAPH_TABLE (RULES.md
//! D110): `bind_match`, the one function through which a GQL query
//! reads a graph's matches.
//!
//! Its pattern is written again as a GRAPH_TABLE's (`gql/pattern.rs`),
//! so the shared binder binds it and path search matches it, with its
//! restrictors over whole paths, its selectors, CHEAPEST among them, its
//! match mode and its WHERE, and the binder checks the graph's
//! privileges as it does any GRAPH_TABLE's. Its COLUMNS give each
//! variable's columns of the binding table: of an element its identity,
//! label set and properties; of a path its elements as JSON and each path
//! function's value; of a group variable its identities, number and each
//! property's values, as arrays. It reads the binding table before it
//! laterally, so what the pattern reads of it is that row's.

use super::pattern::{Bound, pattern_vars};
use super::rewrite::Scope;
use super::{Graph, Kind, Lowering, PATH_PARTS, Var, quote, string};
use crate::ast::{GqlSpan, GraphPattern};
use crate::catalog::{identifier, qualified_identifier};
use crate::error::Result;

/// A pattern's matches: a GRAPH_TABLE, and the variables of its columns.
#[derive(Debug, Clone)]
pub(crate) struct Matched {
    /// A `<table primary>` without its correlation name: "$M", then each
    /// variable's columns.
    pub(crate) table: String,
    /// Its variables.
    pub(crate) vars: Vec<Var>,
}

impl Lowering<'_> {
    /// \[EXTENSION\] The matches of `pattern`, written at tokens `span`, in
    /// `graph`, what it reads of the binding table read through `outer`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 of a label or property the graph has not (GRF-0012);
    /// as the GRAPH_TABLE's binding.
    pub(crate) fn bind_match(&self, graph: &Graph, (pattern, span): (&GraphPattern, GqlSpan), outer: &Scope<'_>) -> Result<Matched> {
        let bound = pattern_vars(graph, pattern)?;
        let vars: Vec<Var> = bound.iter().map(|b| b.var.clone()).collect();
        let text = self.pattern_text(graph, (pattern, span), outer, &vars)?;
        let mut cols = vec!["1 AS \"$M\"".to_owned()];
        for b in &bound {
            cols.extend(columns(graph, b));
        }
        let table = format!("GRAPH_TABLE ({} MATCH {text} COLUMNS ({}))", qualified_identifier(&graph.desc.name), cols.join(", "));
        Ok(Matched { table, vars })
    }
}

/// The COLUMNS of `b`'s variable, each named as the binding table names
/// it.
fn columns(graph: &Graph, b: &Bound) -> Vec<String> {
    let v = &b.var;
    let x = identifier(&v.name);
    let named = |value: String, column: String| format!("{value} AS {}", quote(&column));
    match &v.kind {
        Kind::Element { properties, .. } => {
            let mut out =
                vec![named(format!("ELEMENT_ID({x})"), v.name.clone()), named(label_set(graph, &x, &b.labels), v.labels_column())];
            match properties {
                Some(props) => out.extend(props.iter().map(|(p, _)| named(format!("{x}.{}", identifier(p)), v.property_column(p)))),
                None => out.push(named(format!("{x}.\"PROPS\""), v.props_column())),
            }
            out
        }
        Kind::Group { properties } => {
            let mut out = vec![named(format!("ARRAY_AGG({x})"), v.name.clone()), named(format!("COUNT({x}.*)"), v.count_column())];
            match properties {
                Some(props) => {
                    out.extend(props.iter().map(|(p, _)| named(format!("ARRAY_AGG({x}.{})", identifier(p)), v.property_column(p))));
                }
                None => out.push(named(format!("ARRAY_AGG({x}.\"PROPS\")"), v.props_column())),
            }
            out
        }
        Kind::Path => {
            let mut out = vec![named(format!("PATH_TO_JSON({x})"), v.name.clone())];
            out.extend(PATH_PARTS.iter().map(|(part, f)| named(format!("{f}({x})"), v.part_column(part))));
            out
        }
        Kind::Value => Vec::new(),
    }
}

/// The label set of element variable `x`, `|A|B|`: of an open graph its
/// LABELS; else of each label `labels` names, whether `x` has it.
fn label_set(graph: &Graph, x: &str, labels: &[String]) -> String {
    if graph.open {
        return format!("{x}.\"LABELS\"");
    }
    let mut parts = vec![string(super::LABEL_BAR)];
    for l in labels {
        let has = string(&format!("{l}{}", super::LABEL_BAR));
        parts.push(format!("CASE WHEN {x} IS LABELED {} THEN {has} ELSE '' END", identifier(l)));
    }
    format!("CAST({} AS {})", parts.join(" || "), super::LABELS_TYPE)
}
