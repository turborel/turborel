//! \[EXTENSION\] A GQL MATCH's graph pattern lowered to SQL (RULES.md
//! D-GQL): `bind_match`, the one function through which a GQL query
//! reads a graph's matches.
//!
//! Each path pattern is its expression's table (`gql/paths.rs`,
//! `gql/terms.rs`), chosen among by its selector and holding its path
//! variable; the pattern's table is their join on the variables they
//! share, each element bound once, restricted by its match mode of the
//! elements each fixes. Its WHERE is the MATCH's, written over the
//! binding table and the pattern's variables (`gql/linear.rs`).
//!
//! Integration seam: once GRAPH_TABLE is bound (graph worker on
//! graph-pgq), `bind_match` is the one place that changes: its pattern
//! is then GRAPH_TABLE's, `GRAPH_TABLE (g MATCH <pattern> COLUMNS
//! (ELEMENT_ID(x) AS "X", x.p AS "X.P", ...))`, and its table the shared
//! binder's and path search's. Its result, `Matched`, stays.

use super::paths::select;
use super::rewrite::column;
use super::terms::{Restrict, Term};
use super::{Graph, Kind, Lowering, Var, misused, quote};
use crate::ast::{GqlSpan, GraphPattern, MatchMode, PathPattern, PathPrefix};
use crate::error::Result;
use crate::lexer::{Tok, kw};
use crate::parser::normalize;

/// A pattern's matches: their SQL, the variables they hold, and the
/// tokens of the pattern's WHERE, if written.
#[derive(Debug, Clone)]
pub(crate) struct Matched {
    /// A `<query expression>`: "$M", then each variable's columns.
    pub(crate) sql: String,
    /// Its named variables.
    pub(crate) vars: Vec<Var>,
    /// The tokens of its WHERE condition.
    pub(crate) condition: Option<(usize, usize)>,
}

impl Lowering<'_> {
    /// \[EXTENSION\] The matches of `pattern`, written at tokens `span`,
    /// in `graph`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 of a label or property the graph has not
    /// (GRF-0012), or of a variable bound as an element and a group or
    /// path (GRF-0022); -900 of CHEAPEST.
    pub(crate) fn bind_match(&self, graph: &Graph, pattern: &GraphPattern, span: GqlSpan) -> Result<Matched> {
        let pattern = normalize(pattern);
        let mut from = String::new();
        let mut held: Vec<(Var, String)> = Vec::new();
        let mut fixed: Vec<(String, Term)> = Vec::new();
        for path in &pattern.paths {
            let t = self.path(graph, path, pattern.keep.as_ref())?;
            let alias = self.alias();
            let mut on = vec!["1 = 1".to_owned()];
            for v in &t.vars {
                match held.iter().find(|(x, _)| x.name == v.name) {
                    Some((x, q)) => on.push(shared(x, q, v, &alias)?),
                    None => held.push((v.clone(), alias.clone())),
                }
            }
            if from.is_empty() {
                from = format!("({}) AS {alias}", t.sql);
            } else {
                from = format!("{from} JOIN ({}) AS {alias} ON {}", t.sql, on.join(" AND "));
            }
            fixed.push((alias, t));
        }
        let conds = mode(&fixed, pattern.mode);
        let mut cols = vec!["1 AS \"$M\"".to_owned()];
        for (v, q) in &held {
            cols.extend(v.columns().into_iter().map(|(c, _)| format!("{} AS {}", column(q, &c), quote(&c))));
        }
        let filter = if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) };
        let sql = format!("SELECT {} FROM {from}{filter}", cols.join(", "));
        let condition = pattern.where_clause.as_ref().and_then(|_| self.pattern_where(span));
        Ok(Matched { sql, vars: held.into_iter().map(|(v, _)| v).collect(), condition })
    }

    /// The table of path pattern `path`, its prefix `keep` if it has
    /// none: chosen among by its selector, with its path variable.
    fn path(&self, graph: &Graph, path: &PathPattern, keep: Option<&PathPrefix>) -> Result<Term> {
        let prefix = path.prefix.as_ref().or(keep);
        let selector = prefix.and_then(|p| p.selector.as_ref());
        let r = Restrict { restrictor: prefix.and_then(|p| p.restrictor), selected: selector.is_some() };
        let mut t = self.expression(graph, &path.expression, r)?;
        if let Some(s) = selector {
            t = select(self, t, s)?;
        }
        let Some(v) = path.variable.as_ref().filter(|v| !v.anonymous) else { return Ok(t) };
        let alias = self.alias();
        let var = Var { name: v.name.clone(), kind: Kind::Path };
        let sql = format!(
            "SELECT {alias}.*, {} AS {}, {} AS {} FROM ({}) AS {alias}",
            column(&alias, "$SEQ"),
            quote(&var.name),
            column(&alias, "$LEN"),
            quote(&var.length_column()),
            t.sql
        );
        t.vars.push(var);
        Ok(Term { sql, ..t })
    }

    /// The tokens of the WHERE of the graph pattern written at `span`:
    /// after its WHERE outside any nesting, to its end.
    fn pattern_where(&self, span: GqlSpan) -> Option<(usize, usize)> {
        let mut depth = 0_usize;
        for i in span.start..span.end {
            let t = self.text.tok(i);
            if super::terms::opens(t) {
                depth = depth.saturating_add(1);
            } else if super::terms::closes(t) {
                depth = depth.saturating_sub(1);
            } else if depth == 0 && *t == Tok::KeyWord(kw::WHERE) {
                return Some((i.saturating_add(1), span.end));
            }
        }
        None
    }
}

/// The condition that `v`, of the path pattern correlated as `q2`, is the
/// element `x` of `q` bound before.
///
/// # Errors
///
/// SQLCODE -103 (GRF-0022) unless both are elements.
fn shared(x: &Var, q: &str, v: &Var, q2: &str) -> Result<String> {
    let element = |k: &Kind| matches!(k, Kind::Element { .. });
    if !element(&x.kind) || !element(&v.kind) {
        return Err(misused(&v.name, "an element variable of one element"));
    }
    Ok(format!("{} = {}", column(q, &x.name), column(q2, &v.name)))
}

/// The conditions of match mode `m` on the elements the path patterns
/// `fixed` fix: DIFFERENT EDGES that no two edges are the same,
/// DIFFERENT VERTICES that no two vertices are.
fn mode(fixed: &[(String, Term)], m: Option<MatchMode>) -> Vec<String> {
    let (prefix, count): (&str, fn(&Term) -> usize) = match m {
        Some(MatchMode::DifferentEdges) => ("$E", |t| t.edges),
        Some(MatchMode::DifferentVertices) => ("$V", |t| t.nodes),
        Some(MatchMode::RepeatableElements) | None => return Vec::new(),
    };
    let all: Vec<String> = fixed.iter().flat_map(|(q, t)| (1..=count(t)).map(move |k| column(q, &format!("{prefix}{k}")))).collect();
    let mut out = Vec::new();
    for (i, a) in all.iter().enumerate() {
        out.extend(all.iter().skip(i.saturating_add(1)).map(|b| format!("{a} <> {b}")));
    }
    out
}
