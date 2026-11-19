//! \[EXTENSION\] EXISTS IN PATH p ( `<pattern>` ) [GS §11.2]: whether the
//! pattern matches the path p binds, walked in path order. Each pattern
//! is bound as a path pattern of its own, numbered after the graph
//! pattern's, its variables and prefilters the GRAPH_TABLE's; the
//! predicate is a slot of a path function of p (RULES.md D142).

use super::super::rule;
use crate::ast::{GraphPattern, GraphTable, ParenthesizedPath, PathExpression, PathFactor, PathPattern, PathPrimary, PathTerm};
use crate::error::Result;
use crate::parser::{in_path_patterns, normalize_within};

/// The patterns of EXISTS IN PATH in `g`, in the order written, each in
/// normal form.
///
/// # Errors
///
/// SQLCODE -103 if one declares a variable of `pattern`: its variables
/// are its own.
pub(super) fn patterns(g: &GraphTable, pattern: &GraphPattern) -> Result<Vec<ParenthesizedPath>> {
    let within: Vec<ParenthesizedPath> = in_path_patterns(g)?.iter().enumerate().map(|(k, p)| normalize_within(p, k)).collect();
    let outer = super::variables::declared(pattern, &[])?;
    let inner = super::variables::declared(&as_pattern(&within), &[])?;
    if let Some(d) = inner.iter().find(|d| outer.iter().any(|o| o.name == d.name)) {
        let msg = format!(
            "[GS §11.2] the pattern of EXISTS IN PATH declares {}, a variable of the graph pattern; its variables are its own",
            d.name
        );
        return Err(rule(msg).or_at(d.at));
    }
    Ok(within)
}

/// `within` as path patterns of a graph pattern.
pub(super) fn as_pattern(within: &[ParenthesizedPath]) -> GraphPattern {
    let paths = within
        .iter()
        .map(|p| PathPattern {
            variable: None,
            prefix: None,
            expression: PathExpression::Term(PathTerm {
                factors: vec![PathFactor { primary: PathPrimary::Parenthesized(Box::new(p.clone())), quantifier: None }],
            }),
            at: p.at,
        })
        .collect();
    GraphPattern { mode: None, paths, keep: None, where_clause: None, segments: Vec::new(), at: 0 }
}

/// `pattern` with the path patterns of `within` after its own, whose
/// variables the GRAPH_TABLE declares.
pub(super) fn combined(pattern: &GraphPattern, within: &[ParenthesizedPath]) -> GraphPattern {
    let mut all = pattern.clone();
    all.paths.extend(as_pattern(within).paths);
    all
}
