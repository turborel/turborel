//! \[EXTENSION\] [P1 §6.2] The normal form of a graph pattern, which the
//! binder and the path search compiler start from:
//!
//! * every edge pattern is in its full form, its orientation's table row
//!   (`ast::ORIENTATIONS`) unchanged;
//! * a quantified edge pattern is the bracketed path pattern of it and an
//!   anonymous node on each side, the quantifier on the brackets;
//! * in each concatenation, every edge pattern has a node pattern before
//!   it and after it, an anonymous one inserted where none is written, so
//!   each term begins and ends with a node pattern unless a parenthesized
//!   path pattern begins or ends it;
//! * `*` and `+` are their bounds, { 0, } and { 1, }; `?` stays, as its
//!   singletons are conditional [P1 §4.6];
//! * every element pattern has a variable, an anonymous one named `$n`,
//!   numbered in the order written from 1, past any name the pattern
//!   declares.
//!
//! Its walks recur no deeper than the statement nests, which the parser
//! bounds (RULES.md R-CF-6).

use std::collections::BTreeSet;

use crate::ast::{
    EdgePattern, ElementPattern, GraphPattern, GraphVariable, ParenthesizedPath, PathExpression, PathFactor, PathPrimary, PathTerm,
    Quantifier, QuantifierSugar,
};

/// The first character of the name of an anonymous variable: no letter,
/// so no `<regular identifier>` begins with it.
const ANONYMOUS: &str = "$";

/// \[EXTENSION\] [P1 §6.2] `pattern` in normal form, as this module
/// describes it.
#[must_use]
pub fn normalize(pattern: &GraphPattern) -> GraphPattern {
    let mut taken = BTreeSet::new();
    let mut out = pattern.clone();
    for path in &mut out.paths {
        taken.extend(path.variable.iter().map(|v| v.name.clone()));
        path.expression = shaped(&path.expression);
        declared_in(&path.expression, &mut taken);
    }
    let mut names = Names { taken, next: 0 };
    for path in &mut out.paths {
        names.expression(&mut path.expression);
    }
    out
}

/// `expression` with each of its terms in normal form, its variables
/// as written.
fn shaped(expression: &PathExpression) -> PathExpression {
    match expression {
        PathExpression::Term(t) => PathExpression::Term(term(t)),
        PathExpression::Union(terms) => PathExpression::Union(terms.iter().map(term).collect()),
        PathExpression::Alternation(terms) => PathExpression::Alternation(terms.iter().map(term).collect()),
    }
}

/// The concatenation `t` in normal form: its factors', and a node
/// pattern on each side of every edge pattern.
fn term(t: &PathTerm) -> PathTerm {
    let mut factors: Vec<PathFactor> = Vec::new();
    for f in &t.factors {
        let f = factor(f);
        let after_node = factors.last().is_some_and(is_node);
        let after_edge = factors.last().is_some_and(is_edge);
        if (is_edge(&f) && !after_node) || (after_edge && !is_node(&f)) {
            factors.push(anonymous_node(at_of(&f)));
        }
        factors.push(f);
    }
    if let Some(last) = factors.last().filter(|f| is_edge(f)) {
        let at = at_of(last);
        factors.push(anonymous_node(at));
    }
    PathTerm { factors }
}

/// The factor `f` in normal form: an edge pattern in full, a quantified
/// one in brackets with its end nodes, a parenthesized path pattern's
/// contents normal, a quantifier in braces but `?`.
fn factor(f: &PathFactor) -> PathFactor {
    let quantifier = f.quantifier.map(braced);
    let primary = match &f.primary {
        PathPrimary::Node(e) => PathPrimary::Node(e.clone()),
        PathPrimary::Edge(e) => {
            let edge = EdgePattern { abbreviated: false, ..e.clone() };
            if quantifier.is_none() {
                return PathFactor { primary: PathPrimary::Edge(edge), quantifier };
            }
            let at = edge.filler.at;
            let edge = PathFactor { primary: PathPrimary::Edge(edge), quantifier: None };
            let factors = vec![anonymous_node(at), edge, anonymous_node(at)];
            let expression = PathExpression::Term(PathTerm { factors });
            let bracket = ParenthesizedPath { variable: None, prefix: None, expression, where_clause: None, bracketed: true, at };
            PathPrimary::Parenthesized(Box::new(bracket))
        }
        PathPrimary::Parenthesized(p) => {
            PathPrimary::Parenthesized(Box::new(ParenthesizedPath { expression: shaped(&p.expression), ..(**p).clone() }))
        }
    };
    PathFactor { primary, quantifier }
}

/// `q` in braces: `*` and `+` as their bounds, `?` kept.
fn braced(q: Quantifier) -> Quantifier {
    let sugar = q.sugar.filter(|s| *s == QuantifierSugar::Question);
    Quantifier { sugar, ..q }
}

/// An anonymous node pattern, at `at`.
fn anonymous_node(at: usize) -> PathFactor {
    let node = ElementPattern { variable: None, label: None, where_clause: None, properties: Vec::new(), at };
    PathFactor { primary: PathPrimary::Node(node), quantifier: None }
}

/// Whether `f` is a node pattern.
fn is_node(f: &PathFactor) -> bool {
    matches!(f.primary, PathPrimary::Node(_))
}

/// Whether `f` is an edge pattern.
fn is_edge(f: &PathFactor) -> bool {
    matches!(f.primary, PathPrimary::Edge(_))
}

/// Where `f` begins.
fn at_of(f: &PathFactor) -> usize {
    match &f.primary {
        PathPrimary::Node(e) => e.at,
        PathPrimary::Edge(e) => e.filler.at,
        PathPrimary::Parenthesized(p) => p.at,
    }
}

/// Adds to `taken` the names `expression` declares.
fn declared_in(expression: &PathExpression, taken: &mut BTreeSet<String>) {
    for f in expression.terms().iter().flat_map(|t| &t.factors) {
        let variable = match &f.primary {
            PathPrimary::Node(e) => e.variable.as_ref(),
            PathPrimary::Edge(e) => e.filler.variable.as_ref(),
            PathPrimary::Parenthesized(p) => {
                declared_in(&p.expression, taken);
                p.variable.as_ref()
            }
        };
        taken.extend(variable.map(|v| v.name.clone()));
    }
}

/// The names given to anonymous variables.
struct Names {
    /// The names declared, and those given so far.
    taken: BTreeSet<String>,
    /// The number of the last name given.
    next: u64,
}

impl Names {
    /// Names each anonymous element pattern of `expression`, in the order
    /// written.
    fn expression(&mut self, expression: &mut PathExpression) {
        let terms = match expression {
            PathExpression::Term(t) => std::slice::from_mut(t),
            PathExpression::Union(terms) | PathExpression::Alternation(terms) => terms.as_mut_slice(),
        };
        for f in terms.iter_mut().flat_map(|t| &mut t.factors) {
            match &mut f.primary {
                PathPrimary::Node(e) => self.element(e),
                PathPrimary::Edge(e) => self.element(&mut e.filler),
                PathPrimary::Parenthesized(p) => self.expression(&mut p.expression),
            }
        }
    }

    /// Names `e` if it is anonymous.
    fn element(&mut self, e: &mut ElementPattern) {
        if e.variable.is_none() {
            e.variable = Some(GraphVariable { name: self.fresh(), at: e.at, anonymous: true });
        }
    }

    /// A name not taken, then taken.
    fn fresh(&mut self) -> String {
        loop {
            self.next = self.next.saturating_add(1);
            let name = format!("{ANONYMOUS}{}", self.next);
            if self.taken.insert(name.clone()) {
                return name;
            }
        }
    }
}
