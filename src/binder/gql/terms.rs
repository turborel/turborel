//! \[EXTENSION\] A path term of a GQL MATCH lowered to SQL (RULES.md
//! D-GQL): the join of its elements' rows (`gql/sources.rs`), each edge's
//! "$FROM" the vertex before it and its "$TO" the vertex after, and of its
//! parenthesized and quantified parts' tables (`gql/paths.rs`), each
//! "$START" the vertex before it and "$END" the vertex after.
//!
//! A term's table has the columns `FRAME`: its first and last vertices,
//! its elements, vertices and edges as lists, and its length; then the
//! identity of each edge and each vertex it fixes ("$E1", ..., "$V1",
//! ...), which a quantified part's iterations are checked against; then
//! its variables'. Its element patterns' WHERE and property conditions,
//! and its restrictor of the elements it fixes, are its WHERE.

use super::rewrite::{Scope, column, property, rewrite};
use super::sources::{candidates, edge_rows, element_var, vertex_rows};
use super::{Graph, LABEL_BAR, LIST_TYPE, Lowering, Var, quote, string};
use crate::ast::{EdgePattern, ElementPattern, PathFactor, PathPrimary, Quantifier, Restrictor};
use crate::catalog::ElementKind;
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// The columns every term's table begins with.
pub(super) const FRAME: [&str; 6] = ["$START", "$END", "$SEQ", "$NODES", "$EDGES", "$LEN"];

/// A segment of a path term.
#[derive(Clone, Copy)]
pub(super) enum Seg<'p> {
    /// A vertex pattern, or an anonymous one where none is written.
    Node(Option<&'p ElementPattern>),
    /// An edge pattern.
    Edge(&'p EdgePattern),
    /// A parenthesized or quantified part, and its quantifier.
    Sub(&'p PathPrimary, Option<Quantifier>),
}

/// A lowered term: its SQL, its variables, and how many edges and
/// vertices it fixes.
#[derive(Debug, Clone)]
pub(super) struct Term {
    /// A `<query expression>`.
    pub(super) sql: String,
    /// Its variables, in the order of their columns.
    pub(super) vars: Vec<Var>,
    /// The number of its "$E" columns.
    pub(super) edges: usize,
    /// The number of its "$V" columns.
    pub(super) nodes: usize,
}

impl Term {
    /// Its columns' names, in order.
    pub(super) fn columns(&self) -> Vec<String> {
        let mut all: Vec<String> = FRAME.iter().map(|c| (*c).to_owned()).collect();
        all.extend((1..=self.edges).map(|k| format!("$E{k}")));
        all.extend((1..=self.nodes).map(|k| format!("$V{k}")));
        all.extend(self.vars.iter().flat_map(Var::columns).map(|(c, _)| c));
        all
    }
}

/// How a term is restricted: its restrictor, and whether a selector
/// chooses among its matches.
#[derive(Debug, Clone, Copy)]
pub(super) struct Restrict {
    /// TRAIL, ACYCLIC, SIMPLE or WALK, if any.
    pub(super) restrictor: Option<Restrictor>,
    /// Whether a selector applies.
    pub(super) selected: bool,
}

/// The segments of `factors`, an anonymous vertex where none is written
/// before an edge or a part, or after the last.
pub(super) fn segments(factors: &[PathFactor]) -> Vec<Seg<'_>> {
    let mut out: Vec<Seg<'_>> = Vec::new();
    for f in factors {
        let seg = match &f.primary {
            PathPrimary::Node(n) => f.quantifier.map_or(Seg::Node(Some(n)), |q| Seg::Sub(&f.primary, Some(q))),
            PathPrimary::Edge(e) => f.quantifier.map_or(Seg::Edge(e), |q| Seg::Sub(&f.primary, Some(q))),
            PathPrimary::Parenthesized(_) => Seg::Sub(&f.primary, f.quantifier),
        };
        let node = matches!(seg, Seg::Node(_));
        if !node && !matches!(out.last(), Some(Seg::Node(_))) {
            out.push(Seg::Node(None));
        }
        out.push(seg);
    }
    if !matches!(out.last(), Some(Seg::Node(_))) {
        out.push(Seg::Node(None));
    }
    out
}

/// What a part of a term is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartKind {
    /// A vertex.
    Vertex,
    /// An edge.
    Edge,
    /// A parenthesized or quantified part.
    Sub,
}

/// A part of a term as joined: its correlation name, the vertices it
/// starts and ends at, its element's identity, and what it is.
struct Part {
    /// Its correlation name.
    alias: String,
    /// The SQL of its first vertex's identity.
    start: String,
    /// The SQL of its last vertex's identity.
    end: String,
    /// The SQL of its element's identity: of a part, its first vertex's.
    id: String,
    /// What it is.
    kind: PartKind,
}

/// A segment's SQL, how it joins, and its variables, each with whether
/// it is named.
type Segment = (String, Part, Vec<(Var, bool)>);

impl Lowering<'_> {
    /// The term of `segs`, restricted by `r`.
    ///
    /// # Errors
    ///
    /// As `sources::candidates`, `rewrite` and `paths::repeat`.
    pub(super) fn term(&self, graph: &Graph, segs: &[Seg<'_>], r: Restrict) -> Result<Term> {
        let mut parts: Vec<Part> = Vec::new();
        let mut held: Vec<(Var, String, bool)> = Vec::new();
        let mut from = String::new();
        let mut conds: Vec<String> = Vec::new();
        for seg in segs {
            let alias = self.alias();
            let (sql, part, vars) = self.segment(graph, *seg, &alias, r)?;
            if let Some(prev) = parts.last() {
                from = format!("{from} JOIN ({sql}) AS {alias} ON {} = {}", prev.end, part.start);
            } else {
                from = format!("({sql}) AS {alias}");
            }
            for (var, exported) in vars {
                match held.iter().find(|(v, _, _)| v.name == var.name) {
                    Some((_, first, _)) => conds.push(format!("{} = {}", column(first, &var.name), column(&alias, &var.name))),
                    None => held.push((var, alias.clone(), exported)),
                }
            }
            parts.push(part);
        }
        let scope: Scope<'_> = held.iter().map(|(v, q, _)| (v.name.clone(), (v, q.clone()))).collect();
        for seg in segs {
            conds.extend(self.element_conditions(*seg, &scope)?);
        }
        conds.extend(restricted(&parts, r.restrictor));
        let vars: Vec<(Var, String)> = held.into_iter().filter(|(_, _, e)| *e).map(|(v, q, _)| (v, q)).collect();
        Ok(framed(&parts, &vars, &from, &conds))
    }

    /// The SQL of segment `seg`, correlated as `alias`, how it joins, and
    /// its variables, each with whether it is named.
    fn segment(&self, graph: &Graph, seg: Seg<'_>, alias: &str, r: Restrict) -> Result<Segment> {
        match seg {
            Seg::Node(p) => {
                let (name, named) = self.variable(p);
                let label = p.and_then(|p| p.label.as_ref());
                let tables = candidates(graph, ElementKind::Vertex, label)?;
                let var = element_var(graph, &name, ElementKind::Vertex, &tables);
                let id = column(alias, &name);
                let part = Part { alias: alias.to_owned(), start: id.clone(), end: id.clone(), id, kind: PartKind::Vertex };
                Ok((vertex_rows(graph, &var, &tables, label), part, vec![(var, named)]))
            }
            Seg::Edge(e) => {
                let (name, named) = self.variable(Some(&e.filler));
                let label = e.filler.label.as_ref();
                let tables = candidates(graph, ElementKind::Edge, label)?;
                let var = element_var(graph, &name, ElementKind::Edge, &tables);
                let sql = edge_rows(graph, &var, (&tables, label), e.orientation.directions())?;
                let (start, end, id) = (column(alias, "$FROM"), column(alias, "$TO"), column(alias, &name));
                Ok((sql, Part { alias: alias.to_owned(), start, end, id, kind: PartKind::Edge }, vec![(var, named)]))
            }
            Seg::Sub(primary, quantifier) => {
                let t = self.part(graph, primary, quantifier, r)?;
                let (start, end) = (column(alias, "$START"), column(alias, "$END"));
                let vars = t.vars.into_iter().map(|v| (v, true)).collect();
                let part = Part { alias: alias.to_owned(), id: start.clone(), start, end, kind: PartKind::Sub };
                Ok((t.sql, part, vars))
            }
        }
    }

    /// The name of the variable of element pattern `p`, and whether it
    /// is named: an anonymous one's is the lowering's own.
    fn variable(&self, p: Option<&ElementPattern>) -> (String, bool) {
        match p.and_then(|p| p.variable.as_ref()) {
            Some(v) => (v.name.clone(), !v.anonymous),
            None => (self.anonymous(), false),
        }
    }
}

/// The term's table: `FRAME`, the fixed elements, and `vars` held in
/// the parts named, over the join `from` restricted by `conds`.
fn framed(parts: &[Part], vars: &[(Var, String)], from: &str, conds: &[String]) -> Term {
    let (mut seq, mut nodes, mut edges, mut len) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut fixed_edges, mut fixed_nodes) = (Vec::new(), Vec::new());
    let mut after_part = false;
    for p in parts {
        let listed = format!("{} || {}", p.start, string(LABEL_BAR));
        match p.kind {
            PartKind::Vertex => {
                if !after_part {
                    seq.push(listed.clone());
                    nodes.push(listed);
                }
                fixed_nodes.push(p.id.clone());
            }
            PartKind::Edge => {
                seq.push(format!("{} || {}", p.id, string(LABEL_BAR)));
                edges.push(format!("{} || {}", p.id, string(LABEL_BAR)));
                fixed_edges.push(p.id.clone());
                len.push("1".to_owned());
            }
            PartKind::Sub => {
                let rest = |c: &str| format!("SUBSTRING({} FROM CHAR_LENGTH({}) + 2)", column(&p.alias, c), p.start);
                seq.push(rest("$SEQ"));
                nodes.push(rest("$NODES"));
                edges.push(column(&p.alias, "$EDGES"));
                len.push(column(&p.alias, "$LEN"));
            }
        }
        after_part = p.kind == PartKind::Sub;
    }
    let list = |items: &[String]| {
        if items.is_empty() { format!("CAST('' AS {LIST_TYPE})") } else { format!("CAST({} AS {LIST_TYPE})", items.join(" || ")) }
    };
    let first = parts.first().map_or_else(String::new, |p| p.start.clone());
    let end = parts.last().map_or_else(String::new, |p| p.end.clone());
    let length = if len.is_empty() { "0".to_owned() } else { len.join(" + ") };
    let mut cols = vec![first, end, list(&seq), list(&nodes), list(&edges), format!("CAST({length} AS INTEGER)")];
    cols = cols.into_iter().zip(FRAME).map(|(c, n)| format!("{c} AS {}", quote(n))).collect();
    cols.extend(fixed_edges.iter().enumerate().map(|(k, e)| format!("{e} AS {}", quote(&format!("$E{}", k.saturating_add(1))))));
    cols.extend(fixed_nodes.iter().enumerate().map(|(k, v)| format!("{v} AS {}", quote(&format!("$V{}", k.saturating_add(1))))));
    for (v, q) in vars {
        cols.extend(v.columns().into_iter().map(|(c, _)| format!("{} AS {}", column(q, &c), quote(&c))));
    }
    let filter = if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) };
    let sql = format!("SELECT {} FROM {from}{filter}", cols.join(", "));
    Term { sql, vars: vars.iter().map(|(v, _)| v.clone()).collect(), edges: fixed_edges.len(), nodes: fixed_nodes.len() }
}

/// The conditions `r` puts on the fixed elements of `parts`: TRAIL that
/// no two edges are the same, ACYCLIC that no two vertices are, SIMPLE
/// that no two are but the first and the last.
fn restricted(parts: &[Part], r: Option<Restrictor>) -> Vec<String> {
    let edges: Vec<&Part> = parts.iter().filter(|p| p.kind == PartKind::Edge).collect();
    let nodes: Vec<(usize, &Part)> = parts.iter().enumerate().filter(|(_, p)| p.kind == PartKind::Vertex).collect();
    let mut out = Vec::new();
    let distinct_nodes = |simple: bool, into: &mut Vec<String>| {
        for (x, (i, a)) in nodes.iter().enumerate() {
            for (j, b) in nodes.iter().skip(x.saturating_add(1)) {
                let adjacent = j.saturating_sub(*i) == 1;
                let ends = simple && *i == 0 && j.saturating_add(1) == parts.len();
                if !adjacent && !ends {
                    into.push(format!("{} <> {}", a.id, b.id));
                }
            }
        }
    };
    match r {
        Some(Restrictor::Trail) => {
            for (x, a) in edges.iter().enumerate() {
                out.extend(edges.iter().skip(x.saturating_add(1)).map(|b| format!("{} <> {}", a.id, b.id)));
            }
        }
        Some(Restrictor::Acyclic) => distinct_nodes(false, &mut out),
        Some(Restrictor::Simple) => distinct_nodes(true, &mut out),
        Some(Restrictor::Walk) | None => {}
    }
    out
}

impl Lowering<'_> {
    /// The conditions of segment `seg`'s element pattern: its WHERE and
    /// its property specification, written in `scope`.
    fn element_conditions(&self, seg: Seg<'_>, scope: &Scope<'_>) -> Result<Vec<String>> {
        let p = match seg {
            Seg::Node(Some(p)) => p,
            Seg::Edge(e) => &e.filler,
            Seg::Node(None) | Seg::Sub(_, _) => return Ok(Vec::new()),
        };
        let Some(var) = p.variable.as_ref().and_then(|v| scope.get(&v.name)) else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        if p.where_clause.is_some()
            && let Some(range) = self.where_range(p.at)
        {
            out.push(format!("({})", rewrite(self.text, range, scope)?));
        }
        for spec in &p.properties {
            let Some(name) = self.text.token_at(spec.at) else { continue };
            let start = name.saturating_add(2);
            let end = self.value_end(start);
            let value = rewrite(self.text, (start, end), scope)?;
            let left = property(self.text, (var.0, &var.1), &spec.name, Some(start))?;
            out.push(format!("{left} = ({value})"));
        }
        Ok(out)
    }

    /// The tokens of the WHERE condition of the element pattern or
    /// parenthesized path written at `at`: after its WHERE, at its own
    /// depth, up to its closing delimiter.
    pub(super) fn where_range(&self, at: usize) -> Option<(usize, usize)> {
        let mut i = self.text.token_at(at)?;
        while !opens(self.text.tok(i)) {
            if *self.text.tok(i) == Tok::End {
                return None;
            }
            i = i.saturating_add(1);
        }
        let mut depth = 0_usize;
        let mut condition = None;
        loop {
            let t = self.text.tok(i);
            if opens(t) {
                depth = depth.saturating_add(1);
            } else if closes(t) {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return condition.map(|c| (c, i));
                }
            } else if depth == 1 && *t == Tok::KeyWord(kw::WHERE) {
                condition = Some(i.saturating_add(1));
            } else if *t == Tok::End {
                return None;
            }
            i = i.saturating_add(1);
        }
    }

    /// The token after a property's value that begins at token `start`:
    /// the "," or "}" at its own depth.
    pub(crate) fn value_end(&self, start: usize) -> usize {
        let mut depth = 0_usize;
        let mut i = start;
        loop {
            let t = self.text.tok(i);
            let ends = depth == 0 && (*t == Tok::Delimiter(delim::COMMA) || *t == Tok::Delimiter(delim::RIGHT_BRACE));
            if ends || *t == Tok::End {
                return i;
            }
            if opens(t) {
                depth = depth.saturating_add(1);
            } else if closes(t) {
                depth = depth.saturating_sub(1);
            }
            i = i.saturating_add(1);
        }
    }
}

/// Whether `t` opens a nesting: "(", "[" or "{".
pub(super) fn opens(t: &Tok) -> bool {
    matches!(t, Tok::Delimiter(delim::LEFT_PAREN | delim::LEFT_BRACKET | delim::LEFT_BRACE))
}

/// Whether `t` closes a nesting: ")", "]" or "}".
pub(super) fn closes(t: &Tok) -> bool {
    matches!(t, Tok::Delimiter(delim::RIGHT_PAREN | delim::RIGHT_BRACKET | delim::RIGHT_BRACE))
}
