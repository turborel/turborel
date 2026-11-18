//! \[EXTENSION\] [GS §16 R16.2, R16.4] The shape of a rigid pattern as a
//! join takes it (RULES.md D113, D114): its element patterns from a node
//! to a node, each an instance of its variable, one operand of the join.
//!
//! A quantifier of at most `MOST_UNROLLED` iterations, of a body that is
//! rigid itself, is unrolled: each count of iterations is one rigid
//! pattern, each iteration's variables new instances, and the GRAPH_TABLE
//! the union of their joins. Node patterns made adjacent by it, or by a
//! quantifier's zero iterations, are one vertex, an anonymous one taking
//! the other's variable; two named ones are searched.

use crate::graph::program::{MatchMode, Restrictor, Selector};
use crate::graph::view::Orientation;
use crate::logical::{GraphMatch, PatternShape, TestShape};

/// \[GS §16 R16.4\] The most iterations of a quantifier unrolled.
const MOST_UNROLLED: u32 = 3;

/// The first character of an anonymous variable's name, which
/// normalization gives it (`parser/graph/normalize.rs`).
const ANONYMOUS: char = '$';

/// The most rigid patterns a GRAPH_TABLE unrolls to.
const MOST_UNROLLINGS: usize = 16;

/// An element pattern of a rigid path, in order, and the iteration of a
/// quantifier it is of, zero of none.
#[derive(Clone, Copy)]
pub(super) enum Step<'a> {
    /// A node pattern.
    Vertex(&'a TestShape, usize),
    /// An edge pattern, and how it points.
    Edge(&'a TestShape, Orientation, usize),
}

/// An edge pattern between two node patterns: the instances of the node
/// before, the edge and the node after, and how it points.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Hop {
    pub(super) before: usize,
    pub(super) edge: usize,
    pub(super) after: usize,
    pub(super) orientation: Orientation,
}

/// What a rigid GRAPH_TABLE joins: the variable of each instance, each
/// element pattern's test of its instance, and its edge patterns.
#[derive(Clone)]
pub(super) struct Rigid<'a> {
    /// The element variable of each instance, by its number.
    pub(super) vars: Vec<usize>,
    /// Each (variable, iteration) and its instance.
    keys: Vec<((usize, usize), usize)>,
    /// Each element pattern's test, and its instance.
    pub(super) tests: Vec<(usize, &'a TestShape)>,
    /// Each edge pattern between its nodes.
    pub(super) hops: Vec<Hop>,
    /// The instances of each path pattern, in order, for its restrictor.
    pub(super) paths: Vec<(Restrictor, Vec<usize>)>,
    /// Whether a quantifier was unrolled.
    pub(super) unrolled: bool,
}

/// `g`'s path patterns as joins take them, each count of iterations of
/// its quantifiers one, if each is rigid.
pub(super) fn rigids(g: &GraphMatch) -> Option<Vec<Rigid<'_>>> {
    let empty = Rigid { vars: Vec::new(), keys: Vec::new(), tests: Vec::new(), hops: Vec::new(), paths: Vec::new(), unrolled: false };
    let mut out = vec![empty];
    let mut copies = 0_usize;
    for path in &g.paths {
        if path.selector != Selector::All || path.cost.is_some() {
            return None;
        }
        let alternatives = flatten(&path.pattern, 0, &mut copies)?;
        let mut next = Vec::new();
        for r in &out {
            for steps in &alternatives {
                let mut r = r.clone();
                let instances = r.path_hops(g, steps)?;
                r.unrolled |= steps.iter().any(|s| s.copy() > 0);
                r.paths.push((path.restrictor, instances));
                next.push(r);
            }
        }
        (next.len() <= MOST_UNROLLINGS).then_some(())?;
        out = next;
    }
    out.into_iter().map(|r| r.distinct(g).then_some(r)).collect()
}

impl Step<'_> {
    /// The iteration it is of.
    fn copy(&self) -> usize {
        match self {
            Step::Vertex(_, c) | Step::Edge(_, _, c) => *c,
        }
    }
}

/// Whether an instance of `vars` is written twice.
pub(super) fn twice(vars: &[usize]) -> bool {
    vars.iter().enumerate().any(|(i, v)| vars.iter().skip(i.saturating_add(1)).any(|o| o == v))
}

/// The ways `shape` is a sequence of element patterns, each of iteration
/// `copy`, a quantifier's iterations numbered from `copies` on.
fn flatten<'a>(shape: &'a PatternShape, copy: usize, copies: &mut usize) -> Option<Vec<Vec<Step<'a>>>> {
    match shape {
        PatternShape::Vertex(t) => Some(vec![vec![Step::Vertex(t, copy)]]),
        PatternShape::Edge(t, o) => Some(vec![vec![Step::Edge(t, *o, copy)]]),
        PatternShape::Concat(parts) => {
            let mut out = vec![Vec::new()];
            for p in parts {
                out = joined(&out, &flatten(p, copy, copies)?)?;
            }
            Some(out)
        }
        PatternShape::Group(Restrictor::Walk, body, None) => flatten(body, copy, copies),
        PatternShape::Quantified(body, min, max) => unrolled(body, (*min, (*max)?), copy, copies),
        PatternShape::Union(_) | PatternShape::Alternation(_) | PatternShape::Group(..) => None,
    }
}

/// \[GS §16 R16.4\] The ways `body` quantified `min` to `max` times is a
/// sequence of element patterns, each count of iterations one, if it is
/// within no quantifier, `copy` zero, and `max` at most `MOST_UNROLLED`.
fn unrolled<'a>(body: &'a PatternShape, (min, max): (u32, u32), copy: usize, copies: &mut usize) -> Option<Vec<Vec<Step<'a>>>> {
    if copy > 0 || max > MOST_UNROLLED {
        return None;
    }
    let mut out = Vec::new();
    for k in min..=max {
        let mut iterations = vec![Vec::new()];
        for _ in 0..k {
            *copies = copies.checked_add(1)?;
            iterations = joined(&iterations, &flatten(body, *copies, copies)?)?;
        }
        out.extend(iterations);
    }
    (out.len() <= MOST_UNROLLINGS).then_some(out)
}

/// Each of `firsts` followed by each of `thens`, if not too many.
fn joined<'a>(firsts: &[Vec<Step<'a>>], thens: &[Vec<Step<'a>>]) -> Option<Vec<Vec<Step<'a>>>> {
    let out: Vec<Vec<Step<'a>>> = firsts.iter().flat_map(|f| thens.iter().map(move |t| [f.as_slice(), t.as_slice()].concat())).collect();
    (out.len() <= MOST_UNROLLINGS).then_some(out)
}

impl<'a> Rigid<'a> {
    /// The instance of `var` in iteration `copy`, made if new.
    fn instance(&mut self, var: usize, copy: usize) -> usize {
        if let Some(&(_, i)) = self.keys.iter().find(|(k, _)| *k == (var, copy)) {
            return i;
        }
        let i = self.vars.len();
        self.vars.push(var);
        self.keys.push(((var, copy), i));
        i
    }

    /// The instance the match row's columns of `var` are read of: its
    /// first.
    pub(super) fn of_var(&self, var: usize) -> Option<usize> {
        self.keys.iter().find(|(k, _)| k.0 == var).map(|&(_, i)| i)
    }

    /// Each instance of `var`, once.
    pub(super) fn instances_of(&self, var: usize) -> Vec<usize> {
        let mut out: Vec<usize> = self.keys.iter().filter(|(k, _)| k.0 == var).map(|&(_, i)| i).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Adds the element patterns `steps` of a path: its instances in
    /// order, if its node and edge patterns alternate from a node to a
    /// node once adjacent node patterns are one.
    fn path_hops(&mut self, g: &GraphMatch, steps: &[Step<'a>]) -> Option<Vec<usize>> {
        let mut seq: Vec<(bool, usize, Orientation)> = Vec::new();
        for step in steps {
            match *step {
                Step::Vertex(t, copy) => {
                    if let Some(&(false, prev, _)) = seq.last() {
                        self.merge(g, prev, (t, copy))?;
                        continue;
                    }
                    let i = self.instance(t.shape, copy);
                    self.tests.push((i, t));
                    seq.push((false, i, Orientation::default()));
                }
                Step::Edge(t, orientation, copy) => {
                    let i = self.instance(t.shape, copy);
                    self.tests.push((i, t));
                    seq.push((true, i, orientation));
                }
            }
        }
        let alternate = seq.iter().enumerate().all(|(n, (edge, _, _))| *edge == (n % 2 == 1));
        if !alternate || seq.len().is_multiple_of(2) {
            return None;
        }
        for (n, &(_, edge, orientation)) in seq.iter().enumerate().filter(|(_, s)| s.0) {
            let before = seq.get(n.checked_sub(1)?)?.1;
            let after = seq.get(n.checked_add(1)?)?.1;
            // [GS §16 R16.6] An edge pattern written again is joined once.
            let hop = Hop { before, edge, after, orientation };
            if !self.hops.contains(&hop) {
                self.hops.push(hop);
            }
        }
        Some(seq.into_iter().map(|(_, i, _)| i).collect())
    }

    /// Makes node pattern `t` of iteration `copy`, after node pattern
    /// `prev`, one vertex with it: the anonymous one of them takes the
    /// other's variable; none if both are named, unless they are one.
    fn merge(&mut self, g: &GraphMatch, prev: usize, (t, copy): (&'a TestShape, usize)) -> Option<()> {
        let key = (t.shape, copy);
        if let Some(&(_, i)) = self.keys.iter().find(|(k, _)| *k == key) {
            return (i == prev).then_some(());
        }
        let anonymous = |var: usize| g.vars.get(var).is_some_and(|v| v.name.starts_with(ANONYMOUS));
        if !anonymous(t.shape) {
            if !anonymous(*self.vars.get(prev)?) {
                return None;
            }
            *self.vars.get_mut(prev)? = t.shape;
        }
        self.keys.push((key, prev));
        self.tests.push((prev, t));
        Some(())
    }

    /// \[GS §7.3\] Whether its match mode is kept by its join: no
    /// instance its mode keeps apart is written twice.
    fn distinct(&self, g: &GraphMatch) -> bool {
        let written = |edge: bool| -> Vec<usize> {
            let all = self.paths.iter().flat_map(|(_, p)| p.iter().copied());
            all.filter(|&i| self.vars.get(i).and_then(|&v| g.vars.get(v)).is_some_and(|s| s.edge == edge)).collect()
        };
        match g.mode {
            MatchMode::RepeatableElements => true,
            MatchMode::DifferentEdges => !twice(&written(true)),
            MatchMode::DifferentVertices => !twice(&written(false)),
        }
    }
}
