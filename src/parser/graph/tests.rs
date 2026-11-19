//! Unit tests of GRAPH_TABLE and the graph pattern matching language
//! [P1 §4 to §6]: each production read, written back by `shape`; the
//! Syntax Rules and their GRF conditions; the normal form; and the
//! patterns of the graph standard's examples and conformance tests.

mod examples;
mod expressions;
mod normal;
mod productions;
mod rules;

use std::fmt::Write as _;

use super::*;
use crate::ast::{
    Cheapest, EdgePattern, ElementPattern, GraphPattern, LabelExpression, MatchMode, PathExpression, PathFactor, PathPattern, PathPrefix,
    PathPrimary, PathTerm, Quantifier, QuantifierSugar, Restrictor, Selector,
};

/// The graph pattern `text` reads as, its Syntax Rules not checked.
fn pattern(text: &str) -> Result<GraphPattern> {
    let mut p = Parser::new(text)?;
    p.graph = 1;
    let read = p.graph_pattern()?;
    if *p.peek() != Tok::End {
        return Err(p.syntax("expected the end of the pattern"));
    }
    Ok(read)
}

/// The GRAPH_TABLE `text` reads as, its Syntax Rules checked.
fn table(text: &str) -> Result<GraphTable> {
    let mut p = Parser::new(text)?;
    let TableReference::Graph(read) = p.graph_table()? else { return Err(p.syntax("expected a GRAPH_TABLE")) };
    if *p.peek() != Tok::End {
        return Err(p.syntax("expected the end of the table"));
    }
    Ok(*read)
}

/// The GRAPH_TABLE of `matched`, MATCH's text, in the graph G with one
/// column, its Syntax Rules checked.
fn matched(matched: &str) -> Result<GraphTable> {
    table(&format!("GRAPH_TABLE (G MATCH {matched} COLUMNS (1 AS ONE))"))
}

/// The GRF condition `matched_text` is refused with: its name, of an
/// error of SQLCODE -103.
fn grf(matched_text: &str) -> String {
    let e = matched(matched_text).map(|_| panic!("{matched_text}: accepted")).unwrap_err();
    assert_eq!(e.sqlcode(), -103_i32, "{matched_text}: {e:?}");
    let name = e.msg.rsplit_once("(GRF-").map_or_else(|| panic!("{matched_text}: {e:?}"), |(_, n)| n.trim_end_matches(')'));
    format!("GRF-{name}")
}

/// The SQLCODE `matched_text` is refused with.
fn refused(matched_text: &str) -> i32 {
    matched(matched_text).map(|_| panic!("{matched_text}: accepted")).unwrap_err().sqlcode()
}

/// Whether `matched_text` is accepted, its error shown if not.
fn accepted(matched_text: &str) {
    if let Err(e) = matched(matched_text) {
        panic!("{matched_text}: {e:?}");
    }
}

/// The shape of the pattern `text`, which reads.
fn shaped(text: &str) -> String {
    shape(&pattern(text).unwrap_or_else(|e| panic!("{text}: {e:?}")))
}

/// `p` written back: each element pattern's parts, labels' operators in
/// parentheses, each condition written `WHERE c` and each value `v`,
/// edge patterns and quantifiers as written.
pub(super) fn shape(p: &GraphPattern) -> String {
    let mut out = String::new();
    match p.mode {
        Some(MatchMode::RepeatableElements) => out.push_str("REPEATABLE ELEMENTS "),
        Some(MatchMode::DifferentEdges) => out.push_str("DIFFERENT EDGES "),
        Some(MatchMode::DifferentVertices) => out.push_str("DIFFERENT VERTICES "),
        None => {}
    }
    out.push_str(&p.paths.iter().map(path).collect::<Vec<_>>().join(", "));
    if let Some(k) = &p.keep {
        write!(out, " KEEP {}", prefix(k).trim_end()).unwrap();
    }
    if p.where_clause.is_some() {
        out.push_str(" WHERE c");
    }
    for s in &p.segments {
        write!(out, " FOR EACH SEGMENT ({}, {}) IN {} WHERE c", s.first.name, s.second.name, s.path.name).unwrap();
    }
    out
}

/// A path pattern written back.
fn path(p: &PathPattern) -> String {
    let variable = p.variable.as_ref().map(|v| format!("{} = ", v.name)).unwrap_or_default();
    let prefix = p.prefix.as_ref().map(prefix).unwrap_or_default();
    format!("{prefix}{variable}{}", expression(&p.expression))
}

/// A prefix written back, a space after it.
fn prefix(p: &PathPrefix) -> String {
    let mut out = match &p.selector {
        Some(Selector::Any) => "ANY ".to_owned(),
        Some(Selector::AnyK(k)) => format!("ANY {k} "),
        Some(Selector::AnyShortest) => "ANY SHORTEST ".to_owned(),
        Some(Selector::AllShortest) => "ALL SHORTEST ".to_owned(),
        Some(Selector::ShortestK(k)) => format!("SHORTEST {k} "),
        Some(Selector::ShortestGroups(k)) => format!("SHORTEST {k} GROUPS "),
        Some(Selector::Cheapest { which, cost }) => {
            let words = match which {
                Cheapest::Any => "ANY CHEAPEST ".to_owned(),
                Cheapest::All => "ALL CHEAPEST ".to_owned(),
                Cheapest::K(k) => format!("CHEAPEST {k} "),
            };
            if cost.is_some() { format!("{words}COST (v) ") } else { words }
        }
        None => String::new(),
    };
    out.push_str(match p.restrictor {
        Some(Restrictor::Walk) => "WALK ",
        Some(Restrictor::Trail) => "TRAIL ",
        Some(Restrictor::Acyclic) => "ACYCLIC ",
        Some(Restrictor::Simple) => "SIMPLE ",
        None => "",
    });
    out
}

/// A path pattern expression written back.
fn expression(e: &PathExpression) -> String {
    let terms = |ts: &[PathTerm], sep: &str| ts.iter().map(term).collect::<Vec<_>>().join(sep);
    match e {
        PathExpression::Term(t) => term(t),
        PathExpression::Union(ts) => terms(ts, " | "),
        PathExpression::Alternation(ts) => terms(ts, " |+| "),
    }
}

/// A path term written back.
fn term(t: &PathTerm) -> String {
    t.factors.iter().map(factor).collect::<String>()
}

/// A path factor written back.
fn factor(f: &PathFactor) -> String {
    let primary = match &f.primary {
        PathPrimary::Node(e) => format!("({})", filler(e)),
        PathPrimary::Edge(e) => edge(e),
        PathPrimary::Parenthesized(p) => {
            let variable = p.variable.as_ref().map(|v| format!("{} = ", v.name)).unwrap_or_default();
            let prefix = p.prefix.as_ref().map(prefix).unwrap_or_default();
            let filter = if p.where_clause.is_some() { " WHERE c" } else { "" };
            let (open, close) = if p.bracketed { ("[", "]") } else { ("(", ")") };
            format!("{open}{variable}{prefix}{}{filter}{close}", expression(&p.expression))
        }
    };
    format!("{primary}{}", f.quantifier.as_ref().map(quantifier).unwrap_or_default())
}

/// An edge pattern written back, in the form written.
fn edge(e: &EdgePattern) -> String {
    let form = e.orientation.form();
    if e.abbreviated { form.abbreviation.to_owned() } else { format!("{}{}{}", form.opening, filler(&e.filler), form.closing) }
}

/// An element pattern's filler written back.
fn filler(e: &ElementPattern) -> String {
    let mut out = e.variable.as_ref().map(|v| v.name.clone()).unwrap_or_default();
    if let Some(l) = &e.label {
        write!(out, ":{}", label(l)).unwrap();
    }
    if e.where_clause.is_some() {
        out.push_str(" WHERE c");
    }
    if !e.properties.is_empty() {
        let props: Vec<String> = e.properties.iter().map(|p| format!("{}: v", p.name)).collect();
        write!(out, " {{{}}}", props.join(", ")).unwrap();
    }
    out.trim_start().to_owned()
}

/// A label expression written back, each operator's operands in
/// parentheses.
fn label(l: &LabelExpression) -> String {
    match l {
        LabelExpression::Label(n, _) => n.clone(),
        LabelExpression::Wildcard(_) => "%".into(),
        LabelExpression::Negation(x) => format!("!{}", label(x)),
        LabelExpression::Conjunction(a, b) => format!("({}&{})", label(a), label(b)),
        LabelExpression::Disjunction(a, b) => format!("({}|{})", label(a), label(b)),
    }
}

/// A quantifier written back, as written.
fn quantifier(q: &Quantifier) -> String {
    match q.sugar {
        Some(QuantifierSugar::Star) => "*".into(),
        Some(QuantifierSugar::Plus) => "+".into(),
        Some(QuantifierSugar::Question) => "?".into(),
        None => format!("{{{},{}}}", q.lower, q.upper.map(|u| u.to_string()).unwrap_or_default()),
    }
}
