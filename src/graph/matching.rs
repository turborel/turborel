//! \[DESIGN\] A graph pattern's matches [GS §6.4, §7.3] [P1 §6.5]: the
//! path bindings of its path patterns, each found by its own search,
//! joined on the singleton variables they share by name, then kept or
//! dropped by the match mode.
//!
//! The match mode is applied after each path pattern's selector, as the
//! final WHERE is (implementation-defined reading): DIFFERENT EDGES and
//! DIFFERENT VERTICES drop a match binding one element at two places,
//! unless one singleton variable binds both, the join that makes them
//! one. A group variable binding one edge twice is a repeat.

use std::collections::HashSet;

use super::binding::PathBinding;
use super::program::{MatchMode, PathPattern, Pattern, VarId, VarKind};
use super::view::ElementId;

/// The bindings of one path pattern.
pub struct PathMatches<'p> {
    /// The path pattern.
    pub pattern: &'p PathPattern,
    /// Its bindings.
    pub bindings: Vec<PathBinding>,
}

/// The singleton bindings of `binding` of `pattern`, by variable name.
fn singletons<'b>(pattern: &'b PathPattern, binding: &PathBinding) -> Vec<(&'b str, ElementId)> {
    pattern.variables.iter().zip(0_u32..).filter_map(|(v, id)| binding.singleton(id).map(|e| (v.name.as_str(), e))).collect()
}

/// The group variables of `pattern`: those within a quantifier that may
/// repeat its body [GS §6.6]. Their places are not joins.
fn group_variables(pattern: &PathPattern) -> HashSet<VarId> {
    let mut out = HashSet::new();
    let mut stack: Vec<(&Pattern, bool)> = vec![(&pattern.pattern, false)];
    while let Some((at, grouped)) = stack.pop() {
        match at {
            Pattern::Vertex(test) | Pattern::Edge(test, _) => out.extend(test.var.filter(|_| grouped)),
            Pattern::Concat(parts) | Pattern::Union(parts) | Pattern::Alternation(parts) => {
                stack.extend(parts.iter().map(|p| (p, grouped)));
            }
            Pattern::Quantified(q) => stack.push((&q.body, grouped || q.max != Some(1))),
            Pattern::Group(g) => stack.push((&g.body, grouped)),
        }
    }
    out
}

/// Whether `bindings` agree on every variable they share.
fn joins(singletons: &[Vec<(&str, ElementId)>]) -> bool {
    let all: Vec<&(&str, ElementId)> = singletons.iter().flatten().collect();
    all.iter().all(|(name, element)| all.iter().all(|(other, e)| other != name || e == element))
}

/// \[GS §7.3\] Whether `mode` admits the match of `bindings`.
fn admits(mode: MatchMode, bindings: &[(&PathPattern, &PathBinding)]) -> bool {
    let kind = match mode {
        MatchMode::RepeatableElements => return true,
        MatchMode::DifferentEdges => VarKind::Edge,
        MatchMode::DifferentVertices => VarKind::Vertex,
    };
    // Each place of an element of the kind, with the names bound there.
    let mut places: Vec<(ElementId, Vec<&str>)> = Vec::new();
    for (pattern, binding) in bindings {
        let groups = group_variables(pattern);
        let at = binding.elements.iter().zip(&binding.vars).enumerate();
        for (_, (element, vars)) in at.filter(|(i, _)| i.is_multiple_of(2) != (kind == VarKind::Edge)) {
            let singles = vars.iter().filter(|v| !groups.contains(v));
            let names =
                singles.filter_map(|&v| usize::try_from(v).ok().and_then(|v| pattern.variables.get(v))).map(|v| v.name.as_str()).collect();
            places.push((*element, names));
        }
    }
    places.iter().enumerate().all(|(i, (element, names))| {
        places
            .iter()
            .skip(i.saturating_add(1))
            .filter(|(other, _)| other == element)
            .all(|(_, others)| names.iter().any(|n| others.contains(n)))
    })
}

/// \[GS §6.4\] The matches of a graph pattern of `paths` under `mode`:
/// for each, the index of its binding of each path pattern.
pub fn matches(paths: &[PathMatches<'_>], mode: MatchMode) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let mut choice = vec![0_usize; paths.len()];
    if paths.iter().any(|p| p.bindings.is_empty()) {
        return out;
    }
    loop {
        let chosen: Vec<(&PathPattern, &PathBinding)> =
            paths.iter().zip(&choice).filter_map(|(p, &c)| p.bindings.get(c).map(|b| (p.pattern, b))).collect();
        let single: Vec<Vec<(&str, ElementId)>> = chosen.iter().map(|(p, b)| singletons(p, b)).collect();
        if joins(&single) && admits(mode, &chosen) {
            out.push(choice.clone());
        }
        // The next choice, the last path pattern's binding turning fastest.
        let mut at = paths.len();
        loop {
            let Some(i) = at.checked_sub(1) else { return out };
            at = i;
            let (Some(c), Some(p)) = (choice.get_mut(i), paths.get(i)) else { return out };
            *c = c.saturating_add(1);
            if *c < p.bindings.len() {
                break;
            }
            *c = 0;
        }
    }
}
