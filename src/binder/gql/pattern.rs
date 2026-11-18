//! \[EXTENSION\] A GQL MATCH's graph pattern written again as a
//! GRAPH_TABLE's (RULES.md D-GQL), for the shared binder and path search
//! to match: its tokens copied as written, but for what reads the binding
//! table, which becomes its columns, read laterally.
//!
//! The pattern's own variables are read by their own names. In a graph
//! over tables its labels and properties are the graph's, written as they
//! are. An open graph's element tables have the one label NODE or EDGE,
//! so each element pattern's labels and property map become a condition
//! of its LABELS and PROPS, its WHERE kept with them.

use super::rewrite::{NATIVE, Scope, json_property, literal_type, rewrite, token};
use super::sources::{candidates, element_var, label_sql};
use super::{Graph, Kind, Lowering, Var};
use crate::ast::{ElementPattern, GqlSpan, GraphPattern, LabelExpression, PathExpression, PathPrimary};
use crate::catalog::{ElementKind, identifier};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// A variable of a pattern, and the labels of the element tables it may
/// bind to, of a graph over tables.
#[derive(Debug, Clone)]
pub(super) struct Bound {
    /// The variable.
    pub(super) var: Var,
    /// The labels.
    pub(super) labels: Vec<String>,
}

/// An element variable's occurrence in a pattern.
struct Occurrence<'p> {
    /// Its name.
    name: String,
    /// Vertex or edge.
    kind: ElementKind,
    /// Whether under a quantifier: a group variable's.
    group: bool,
    /// Its label expression.
    label: Option<&'p LabelExpression>,
}

/// The occurrences of element variables in `e`, under a quantifier if
/// `group`.
fn occurrences<'p>(e: &'p PathExpression, group: bool, out: &mut Vec<Occurrence<'p>>) {
    for f in e.terms().iter().flat_map(|t| &t.factors) {
        let group = group || f.quantifier.is_some();
        match &f.primary {
            PathPrimary::Node(n) => named(n, (ElementKind::Vertex, group), out),
            PathPrimary::Edge(edge) => named(&edge.filler, (ElementKind::Edge, group), out),
            PathPrimary::Parenthesized(p) => occurrences(&p.expression, group, out),
        }
    }
}

/// The occurrence of `p`'s variable, if it names one.
fn named<'p>(p: &'p ElementPattern, (kind, group): (ElementKind, bool), out: &mut Vec<Occurrence<'p>>) {
    if let Some(v) = p.variable.as_ref().filter(|v| !v.anonymous) {
        out.push(Occurrence { name: v.name.clone(), kind, group, label: p.label.as_ref() });
    }
}

/// The element patterns of `e` that have a filler: every vertex pattern,
/// each edge pattern not abbreviated.
fn elements<'p>(e: &'p PathExpression, out: &mut Vec<&'p ElementPattern>) {
    for f in e.terms().iter().flat_map(|t| &t.factors) {
        match &f.primary {
            PathPrimary::Node(n) => out.push(n),
            PathPrimary::Edge(edge) => {
                if !edge.abbreviated {
                    out.push(&edge.filler);
                }
            }
            PathPrimary::Parenthesized(p) => elements(&p.expression, out),
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

/// The variables `pattern` declares, of `graph`: each element
/// variable, of the element tables every occurrence's labels allow, a
/// group variable's if one is under a quantifier; then each path
/// variable.
///
/// # Errors
///
/// SQLCODE -103 (GRF-0012) of a label the graph has not.
pub(super) fn pattern_vars(graph: &Graph, pattern: &GraphPattern) -> Result<Vec<Bound>> {
    let mut all = Vec::new();
    for path in &pattern.paths {
        occurrences(&path.expression, false, &mut all);
    }
    let mut out: Vec<Bound> = Vec::new();
    for o in &all {
        if out.iter().any(|b| b.var.name == o.name) {
            continue;
        }
        let same: Vec<&Occurrence<'_>> = all.iter().filter(|x| x.name == o.name).collect();
        let mut tables = candidates(graph, o.kind, o.label)?;
        for x in &same {
            let more = candidates(graph, x.kind, x.label)?;
            tables.retain(|t| more.iter().any(|m| m.alias == t.alias));
        }
        let mut var = element_var(graph, &o.name, o.kind, &tables);
        if same.iter().any(|x| x.group)
            && let Kind::Element { properties, .. } = var.kind
        {
            var.kind = Kind::Group { properties };
        }
        let mut labels: Vec<String> = Vec::new();
        for l in tables.iter().flat_map(|t| &t.labels) {
            if !labels.contains(&l.name) {
                labels.push(l.name.clone());
            }
        }
        out.push(Bound { var, labels });
    }
    for v in pattern.paths.iter().filter_map(|p| p.variable.as_ref()).filter(|v| !v.anonymous) {
        out.push(Bound { var: Var { name: v.name.clone(), kind: Kind::Path }, labels: Vec::new() });
    }
    Ok(out)
}

impl Lowering<'_> {
    /// The text of `pattern`, written at tokens `span`, as a GRAPH_TABLE's:
    /// what reads a variable of the binding table `outer` sees, but not
    /// one of the pattern's own `vars`, read from its columns.
    ///
    /// # Errors
    ///
    /// As `rewrite`.
    pub(super) fn pattern_text(
        &self,
        graph: &Graph,
        (pattern, span): (&GraphPattern, GqlSpan),
        outer: &Scope<'_>,
        vars: &[Var],
    ) -> Result<String> {
        let own = |n: &String| vars.iter().any(|v| v.name == *n);
        let mut scope: Scope<'_> = outer.iter().filter(|(n, _)| !own(n)).map(|(n, held)| (n.clone(), held.clone())).collect();
        for v in vars.iter().filter(|v| v.kind != Kind::Path) {
            scope.insert(v.name.clone(), (v, NATIVE.to_owned()));
        }
        let fillers = if graph.open { self.fillers(pattern, &scope)? } else { Vec::new() };
        let mut out = String::new();
        let mut from = self.text.at(span.start);
        let mut i = span.start;
        while i < span.end {
            let (written, next) = match fillers.iter().find(|(s, _, _)| *s == i) {
                Some((_, e, text)) => (text.clone(), *e),
                None => token(self.text, i, &scope)?,
            };
            let next = next.max(i.saturating_add(1));
            if next != i.saturating_add(1) || written != self.text.written(i) {
                out.extend(self.text.chars.get(from..self.text.at(i)).unwrap_or_default());
                out.push_str(&written);
                out.push(' ');
                from = self.text.end(next.saturating_sub(1));
            }
            i = next;
        }
        out.extend(self.text.chars.get(from..self.text.end(span.end.saturating_sub(1))).unwrap_or_default());
        Ok(out)
    }

    /// Of an open graph, each element pattern's filler written again: the
    /// token after its opening "(" or "[", its closing one, and its text.
    fn fillers(&self, pattern: &GraphPattern, scope: &Scope<'_>) -> Result<Vec<(usize, usize, String)>> {
        let mut all = Vec::new();
        for path in &pattern.paths {
            elements(&path.expression, &mut all);
        }
        let mut out = Vec::new();
        for p in all {
            let Some((open, close)) = self.bracketed(p.at) else { continue };
            out.push((open.saturating_add(1), close, self.open_filler(p, scope)?));
        }
        Ok(out)
    }

    /// The filler of element pattern `p` of an open graph: its variable,
    /// one of its own if anonymous, and as its WHERE its labels' test of
    /// LABELS, its property map's of PROPS, and its own WHERE.
    fn open_filler(&self, p: &ElementPattern, scope: &Scope<'_>) -> Result<String> {
        let name = p.variable.as_ref().filter(|v| !v.anonymous).map_or_else(|| self.fresh("GQL_A"), |v| v.name.clone());
        let written = identifier(&name);
        let mut conds = Vec::new();
        if let Some(e) = &p.label {
            conds.push(label_sql(e, &format!("{written}.\"LABELS\"")));
        }
        for spec in &p.properties {
            let Some(k) = self.text.token_at(spec.at) else { continue };
            let start = k.saturating_add(2);
            let value = rewrite(self.text, (start, self.value_end(start)), scope)?;
            let map = format!("{written}.\"PROPS\"");
            conds.push(format!("{} = ({value})", json_property(&map, &spec.name, literal_type(self.text, start))));
        }
        if p.where_clause.is_some()
            && let Some(range) = self.where_range(p.at)
        {
            conds.push(format!("({})", rewrite(self.text, range, scope)?));
        }
        if conds.is_empty() {
            return Ok(written);
        }
        Ok(format!("{written} WHERE {}", conds.join(" AND ")))
    }

    /// The first "(" or "[" from the token that begins at character `at`,
    /// and the token that closes it.
    fn bracketed(&self, at: usize) -> Option<(usize, usize)> {
        let mut i = self.text.token_at(at)?;
        while !matches!(self.text.tok(i), Tok::Delimiter(delim::LEFT_PAREN | delim::LEFT_BRACKET)) {
            if *self.text.tok(i) == Tok::End {
                return None;
            }
            i = i.saturating_add(1);
        }
        let open = i;
        let mut depth = 0_usize;
        loop {
            let t = self.text.tok(i);
            if opens(t) {
                depth = depth.saturating_add(1);
            } else if closes(t) {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((open, i));
                }
            } else if *t == Tok::End {
                return None;
            }
            i = i.saturating_add(1);
        }
    }

    /// The tokens of the WHERE condition of the element pattern or
    /// parenthesized path written at `at`: after its WHERE, at its own
    /// depth, up to its closing delimiter.
    pub(super) fn where_range(&self, at: usize) -> Option<(usize, usize)> {
        let (open, close) = self.bracketed(at)?;
        let mut depth = 0_usize;
        for i in open..close {
            let t = self.text.tok(i);
            if opens(t) {
                depth = depth.saturating_add(1);
            } else if closes(t) {
                depth = depth.saturating_sub(1);
            } else if depth == 1 && *t == Tok::KeyWord(kw::WHERE) {
                return Some((i.saturating_add(1), close));
            }
        }
        None
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
