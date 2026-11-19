//! \[EXTENSION\] [GS §16 R16.6] Common sub-pattern elimination (RULES.md
//! D114): of comma-separated path patterns, one written as an earlier one
//! is, of the same variables, element patterns, prefilters and starts, is
//! searched once, its bindings the earlier's. Anonymous element patterns
//! are alike by their place, as their numbers in each path pattern are.
//! The matches join each with the other on all their variables, so the
//! rows are the same.

use std::fmt::Write;

use super::{GraphMatch, PathShape, PatternShape, TestShape};

/// The first character of an anonymous variable's name
/// (`parser/graph/normalize.rs`).
const ANONYMOUS: char = '$';

/// Of each path pattern of `g`, the earlier one it is written as, if one.
pub(super) fn earlier_paths(g: &GraphMatch) -> Vec<Option<usize>> {
    let keys: Vec<Option<String>> = g.paths.iter().map(|p| p.cost.is_none().then(|| key(g, p))).collect();
    keys.iter()
        .enumerate()
        .map(|(n, k)| {
            let k = k.as_ref()?;
            keys.iter().take(n).position(|o| o.as_ref() == Some(k))
        })
        .collect()
}

/// What path pattern `p` of `g` searches, as text.
fn key(g: &GraphMatch, p: &PathShape) -> String {
    let variables: Vec<(String, bool)> = p.variables.iter().map(|(_, edge, shape)| (named(g, *shape), *edge)).collect();
    let mut out = format!("{variables:?} {:?} {:?} {:?} ", p.selector, p.restrictor, p.starts);
    pattern(g, &p.pattern, &mut out);
    out
}

/// `shape` as text, each prefilter by its condition and reads.
fn pattern(g: &GraphMatch, shape: &PatternShape, out: &mut String) {
    let test = |t: &TestShape, text: &mut String| {
        let filter = t.filter.and_then(|k| g.prefilters.get(k)).map(|f| format!("{:?} {:?} {:?}", f.cond, f.own, f.reads));
        let _ = write!(text, "{:?} {} {:?} {filter:?}", t.var, named(g, t.shape), t.labels);
    };
    match shape {
        PatternShape::Vertex(t) => {
            out.push_str("(V ");
            test(t, out);
        }
        PatternShape::Edge(t, o) => {
            let _ = write!(out, "(E {o} ");
            test(t, out);
        }
        PatternShape::Concat(parts) => list(g, ("C", parts), out),
        PatternShape::Union(parts) => list(g, ("U", parts), out),
        PatternShape::Alternation(parts) => list(g, ("A", parts), out),
        PatternShape::Quantified(body, min, max) => {
            let _ = write!(out, "(Q {min} {max:?} ");
            pattern(g, body, out);
        }
        PatternShape::Group(restrictor, body, filter) => {
            let filter = filter.and_then(|k| g.prefilters.get(k)).map(|f| format!("{:?} {:?}", f.cond, f.reads));
            let _ = write!(out, "(G {restrictor:?} {filter:?} ");
            pattern(g, body, out);
        }
    }
    out.push(')');
}

/// The patterns `parts` of a list of kind `tag` as text.
fn list(g: &GraphMatch, (tag, parts): (&str, &[PatternShape]), out: &mut String) {
    let _ = write!(out, "({tag} {} ", parts.len());
    for p in parts {
        pattern(g, p, out);
    }
}

/// Element variable `shape` of `g` as the key has it: its number, or `$`
/// if it is anonymous, as normalization names it.
fn named(g: &GraphMatch, shape: usize) -> String {
    let anonymous = g.vars.get(shape).is_none_or(|v| v.name.starts_with(ANONYMOUS));
    if anonymous { ANONYMOUS.to_string() } else { shape.to_string() }
}
