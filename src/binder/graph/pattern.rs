//! \[EXTENSION\] A GRAPH_TABLE's path patterns, bound [GS §6, §7]: each
//! as path search takes it (`logical/graph.rs` `PathShape`), its element
//! patterns' WHERE and property specifications its prefilters [GS §6.8].
//!
//! A prefilter reads its own element and the variables bound before it,
//! which path search keeps for it [P1 §4.4]. One of an element outside
//! every quantifier that reads a variable bound after it is the path's
//! instead, tested as its match ends, which is the same test of the same
//! bindings; within a quantifier it is refused, as its iteration would
//! read a binding not yet made.

use std::collections::BTreeSet;

use super::super::{Scope, rule};
use super::grf;
use super::scope::unsupported;
use crate::ast::{
    ElementPattern, GraphPattern, GraphVariable, LabelExpression, Orientation as Written, ParenthesizedPath, PathExpression, PathFactor,
    PathPrimary, PathTerm, Restrictor as WrittenRestrictor, Selector as WrittenSelector,
};
use crate::error::Result;
use crate::graph::program::{LabelExpr, Restrictor, Selector, VarId};
use crate::graph::view::Orientation;
use crate::logical::{Cond, PathShape, PatternShape, Prefilter, SUBPATH_MARK, TestShape};

/// The rule a set function in an element pattern's WHERE breaks.
const ELEMENT_ROWS: &str = "[GS §6.8] the WHERE of an element pattern is of one element, so it holds no set function";

/// What binds a prefilter: of its condition, the element variable whose
/// properties it sees unqualified, if any, and whether it is of a path
/// (set functions allowed); its condition, the variables it reads and
/// the slots of its set functions.
pub(super) type BindFilter<'f> = dyn Fn(&Filtered<'_>) -> Result<BoundFilter> + 'f;

/// A prefilter bound: its condition, the element variables it reads, and
/// the slots it computes, each with its element variable.
pub(super) type BoundFilter = (Cond, BTreeSet<usize>, Vec<(usize, usize)>);

/// A prefilter to bind.
pub(super) enum Filtered<'p> {
    /// An element pattern's WHERE and property specifications, of the
    /// element variable `own`.
    Element(&'p ElementPattern, usize),
    /// A parenthesized path pattern's WHERE, and the pattern.
    Path(&'p crate::ast::SearchCondition, &'p ParenthesizedPath),
}

/// The scope of an `Element` prefilter.
pub(super) const ELEMENT_SCOPE: Scope<'static> = Scope::Rows(ELEMENT_ROWS);

/// A path pattern being bound.
struct Walk<'w> {
    /// The variables of the graph pattern, by name.
    names: &'w [String],
    /// The graph's labels.
    labels: &'w [String],
    /// Its variables: name, whether an edge's, element variable.
    vars: Vec<(String, bool, usize)>,
    /// Each element pattern's variable, in the order written.
    order: Vec<usize>,
    /// The element patterns walked.
    walked: usize,
    /// The quantifiers around the element pattern walked.
    depth: usize,
    /// The prefilters bound.
    prefilters: &'w mut Vec<Prefilter>,
    /// Its subpath variables, each with the variables marking where its
    /// subpath begins and ends.
    subpaths: Vec<(String, VarId, VarId)>,
    /// Prefilters of the path itself, lifted from its elements.
    lifted: Vec<usize>,
    bind: &'w BindFilter<'w>,
}

/// A subpath variable: its name, its path pattern, and the variables
/// marking where its subpath begins and ends there.
pub(super) type Subpath = (String, usize, VarId, VarId);

/// The path patterns of `pattern`, their prefilters and their subpath
/// variables, of the element variables `names` and graph labels `labels`.
///
/// # Errors
///
/// SQLCODE -103 if a Syntax Rule is broken; -900 of what this release
/// does not bind.
pub(super) fn paths(
    pattern: &GraphPattern,
    (names, labels): (&[String], &[String]),
    bind: &BindFilter<'_>,
) -> Result<(Vec<PathShape>, Vec<Prefilter>, Vec<Subpath>)> {
    let mut prefilters = Vec::new();
    let mut shapes = Vec::new();
    let mut subpaths = Vec::new();
    for (n, path) in pattern.paths.iter().enumerate() {
        let prefix = path.prefix.as_ref().or(pattern.keep.as_ref());
        let mut walk = Walk {
            names,
            labels,
            vars: Vec::new(),
            order: Vec::new(),
            walked: 0,
            depth: 0,
            prefilters: &mut prefilters,
            subpaths: Vec::new(),
            lifted: Vec::new(),
            bind,
        };
        order(&path.expression, names, &mut walk.order);
        let mut shape = walk.expression(&path.expression)?;
        for lifted in walk.lifted.clone() {
            let reads = walk.prefilters.get(lifted).map(|p| p.reads.clone()).unwrap_or_default();
            let renumbered = reads.into_iter().map(|(v, _)| Ok((v, walk.var_of(v)?))).collect::<Result<Vec<_>>>()?;
            if let Some(p) = walk.prefilters.get_mut(lifted) {
                p.reads = renumbered;
            }
            shape = PatternShape::Group(Restrictor::Walk, Box::new(shape), Some(lifted));
        }
        let selector = prefix.and_then(|p| p.selector.as_ref()).map_or(Selector::All, selector);
        let restrictor = prefix.and_then(|p| p.restrictor).map_or(Restrictor::Walk, restrictor);
        subpaths.extend(walk.subpaths.drain(..).map(|(name, start, end)| (name, n, start, end)));
        shapes.push(PathShape { variables: walk.vars, selector, restrictor, pattern: shape, cost: None, starts: Vec::new() });
    }
    Ok((shapes, prefilters, subpaths))
}

/// Each element pattern's variable in `e`, in the order written.
fn order(expression: &PathExpression, names: &[String], out: &mut Vec<usize>) {
    for factor in terms(expression).into_iter().flat_map(|t| &t.factors) {
        let pattern = match &factor.primary {
            PathPrimary::Node(n) => n,
            PathPrimary::Edge(e) => &e.filler,
            PathPrimary::Parenthesized(p) => {
                order(&p.expression, names, out);
                continue;
            }
        };
        let var = pattern.variable.as_ref().and_then(|v| names.iter().position(|n| n == &v.name));
        out.push(var.unwrap_or(usize::MAX));
    }
}

/// The terms of `e`.
fn terms(e: &PathExpression) -> Vec<&PathTerm> {
    match e {
        PathExpression::Term(t) => vec![t],
        PathExpression::Union(ts) | PathExpression::Alternation(ts) => ts.iter().collect(),
    }
}

impl Walk<'_> {
    /// `e` bound.
    fn expression(&mut self, e: &PathExpression) -> Result<PatternShape> {
        match e {
            PathExpression::Term(t) => self.term(t),
            PathExpression::Union(ts) => Ok(PatternShape::Union(ts.iter().map(|t| self.term(t)).collect::<Result<_>>()?)),
            PathExpression::Alternation(ts) => Ok(PatternShape::Alternation(ts.iter().map(|t| self.term(t)).collect::<Result<_>>()?)),
        }
    }

    /// `t` bound.
    fn term(&mut self, t: &PathTerm) -> Result<PatternShape> {
        let mut parts = t.factors.iter().map(|f| self.factor(f)).collect::<Result<Vec<_>>>()?;
        if parts.len() == 1
            && let Some(one) = parts.pop()
        {
            return Ok(one);
        }
        Ok(PatternShape::Concat(parts))
    }

    /// `f` bound.
    fn factor(&mut self, f: &PathFactor) -> Result<PatternShape> {
        let Some(q) = &f.quantifier else { return self.primary(f) };
        self.depth = self.depth.saturating_add(1);
        let body = self.primary(f);
        self.depth = self.depth.saturating_sub(1);
        Ok(PatternShape::Quantified(Box::new(body?), q.lower, q.upper))
    }

    /// The primary of `f` bound.
    fn primary(&mut self, f: &PathFactor) -> Result<PatternShape> {
        match &f.primary {
            PathPrimary::Node(n) => Ok(PatternShape::Vertex(self.element(n, false)?)),
            PathPrimary::Edge(e) => Ok(PatternShape::Edge(self.element(&e.filler, true)?, orientation(e.orientation))),
            PathPrimary::Parenthesized(p) => self.parenthesized(p),
        }
    }

    /// A parenthesized path pattern `p` bound: of a subpath variable,
    /// between the marks of where its subpath begins and ends.
    fn parenthesized(&mut self, p: &ParenthesizedPath) -> Result<PatternShape> {
        if p.prefix.as_ref().is_some_and(|x| x.selector.is_some()) {
            return Err(grf("GRF-0033", "a selector is placed inside a parenthesized path pattern").or_at(p.at));
        }
        let restrictor = p.prefix.as_ref().and_then(|x| x.restrictor).map_or(Restrictor::Walk, restrictor);
        let body = self.expression(&p.expression)?;
        let filter = match &p.where_clause {
            Some(c) => Some(self.path_filter(c, p)?),
            None => None,
        };
        let group = PatternShape::Group(restrictor, Box::new(body), filter);
        let Some(v) = &p.variable else { return Ok(group) };
        let (start, end) = self.subpath(v)?;
        let mark = |var: VarId| PatternShape::Vertex(TestShape { var: Some(var), shape: SUBPATH_MARK, labels: None, filter: None });
        Ok(PatternShape::Concat(vec![mark(start), group, mark(end)]))
    }

    /// \[GS §6.4\] The variables marking where subpath variable `v`'s
    /// subpath begins and ends: each bound to the vertex there, the
    /// same of each alternative declaring it.
    fn subpath(&mut self, v: &GraphVariable) -> Result<(VarId, VarId)> {
        if self.depth > 0 {
            let msg = format!(
                "[GS §6.6, §6.9] subpath variable {} is within a quantifier, so is bound to a list of subpaths, of which no path function is",
                v.name
            );
            return Err(rule(msg).or_at(v.at));
        }
        if let Some(&(_, start, end)) = self.subpaths.iter().find(|s| s.0 == v.name) {
            return Ok((start, end));
        }
        let start = self.mark(format!("$<{}", v.name))?;
        let end = self.mark(format!("$>{}", v.name))?;
        self.subpaths.push((v.name.clone(), start, end));
        Ok((start, end))
    }

    /// A new variable of the path, a mark of no element variable.
    fn mark(&mut self, name: String) -> Result<VarId> {
        self.vars.push((name, false, SUBPATH_MARK));
        VarId::try_from(self.vars.len().saturating_sub(1)).map_err(|_| grf("GRF-0024", "too many variables"))
    }

    /// The prefilter of a parenthesized path pattern's WHERE `c`.
    fn path_filter(&mut self, c: &crate::ast::SearchCondition, p: &ParenthesizedPath) -> Result<usize> {
        let (cond, reads, slots) = (self.bind)(&Filtered::Path(c, p))?;
        let reads = reads.into_iter().map(|v| Ok((v, self.var_of(v)?))).collect::<Result<Vec<_>>>()?;
        let slots = slots.into_iter().map(|(k, v)| Ok((k, self.var_of(v)?))).collect::<Result<Vec<_>>>()?;
        self.prefilters.push(Prefilter { cond, own: None, reads, slots });
        Ok(self.prefilters.len().saturating_sub(1))
    }

    /// The number in this path pattern of element variable `v`, which it
    /// must declare.
    fn var_of(&self, v: usize) -> Result<VarId> {
        let found = self.vars.iter().position(|(_, _, s)| *s == v).or_else(|| self.order.contains(&v).then_some(usize::MAX));
        let name = self.names.get(v).map_or("?", String::as_str);
        match found {
            Some(usize::MAX) | None => Err(unsupported(&format!("a prefilter reading {name}, of another path pattern or bound later"))),
            Some(k) => VarId::try_from(k).map_err(|_| grf("GRF-0024", "too many variables")),
        }
    }

    /// An element pattern `p` bound, an edge pattern's if `edge`.
    fn element(&mut self, p: &ElementPattern, edge: bool) -> Result<TestShape> {
        let ordinal = self.walked;
        self.walked = self.walked.saturating_add(1);
        let var = p.variable.as_ref().ok_or_else(|| grf("GRF-0024", "an element pattern has no variable after normalization"))?;
        let shape =
            self.names.iter().position(|n| n == &var.name).ok_or_else(|| grf("GRF-0024", format!("{} is not declared", var.name)))?;
        // An anonymous element is bound to no variable of the path, so
        // two such of alternatives do not tell their bindings apart.
        let found = self.vars.iter().position(|(_, _, s)| *s == shape);
        let id = if found.is_some() || var.name.starts_with('$') {
            found
        } else {
            self.vars.push((var.name.clone(), edge, shape));
            Some(self.vars.len().saturating_sub(1))
        };
        let labels = match &p.label {
            Some(l) => Some(self.label(l)?),
            None => None,
        };
        let filter = if p.where_clause.is_some() || !p.properties.is_empty() { self.element_filter(p, shape, ordinal)? } else { None };
        Ok(TestShape { var: id.and_then(|k| VarId::try_from(k).ok()), shape, labels, filter })
    }

    /// The prefilter of element pattern `p` of variable `shape`, the
    /// `ordinal`th written: its own, or the path's.
    fn element_filter(&mut self, p: &ElementPattern, shape: usize, ordinal: usize) -> Result<Option<usize>> {
        let (cond, reads, slots) = (self.bind)(&Filtered::Element(p, shape))?;
        let later = |v: &usize| self.order.iter().enumerate().any(|(k, s)| s == v && k >= ordinal);
        let lifted = reads.iter().any(|v| *v != shape && later(v));
        if lifted && self.depth > 0 {
            return Err(unsupported("a prefilter within a quantifier reading a variable bound after it"));
        }
        let own = if lifted { None } else { Some(shape) };
        let mut read = Vec::new();
        for v in reads.into_iter().filter(|v| lifted || *v != shape) {
            read.push((v, if lifted { VarId::MAX } else { self.var_of(v)? }));
        }
        let slots = slots.into_iter().map(|(k, v)| Ok((k, self.var_of(v)?))).collect::<Result<Vec<_>>>()?;
        self.prefilters.push(Prefilter { cond, own, reads: read, slots });
        let k = self.prefilters.len().saturating_sub(1);
        if lifted {
            self.lifted.push(k);
            return Ok(None);
        }
        Ok(Some(k))
    }

    /// \[GS §6.2\] Label expression `e`, its labels numbered.
    fn label(&self, e: &LabelExpression) -> Result<LabelExpr> {
        Ok(match e {
            LabelExpression::Label(name, at) => {
                let id = self
                    .labels
                    .iter()
                    .position(|l| l == name)
                    .ok_or_else(|| grf("GRF-0012", format!("the graph has no label {name}")).or_at(*at))?;
                LabelExpr::Label(u32::try_from(id).unwrap_or(u32::MAX))
            }
            LabelExpression::Wildcard(_) => LabelExpr::Wildcard,
            LabelExpression::Negation(inner) => LabelExpr::Not(Box::new(self.label(inner)?)),
            LabelExpression::Conjunction(a, b) => LabelExpr::And(Box::new(self.label(a)?), Box::new(self.label(b)?)),
            LabelExpression::Disjunction(a, b) => LabelExpr::Or(Box::new(self.label(a)?), Box::new(self.label(b)?)),
        })
    }
}

/// The orientation written as path search takes it.
fn orientation(o: Written) -> Orientation {
    match o {
        Written::Left => Orientation::LEFT,
        Written::Undirected => Orientation::UNDIRECTED,
        Written::Right => Orientation::RIGHT,
        Written::LeftOrUndirected => Orientation::LEFT_OR_UNDIRECTED,
        Written::UndirectedOrRight => Orientation::UNDIRECTED_OR_RIGHT,
        Written::LeftOrRight => Orientation::LEFT_OR_RIGHT,
        Written::Any => Orientation::ANY,
    }
}

/// The restrictor written as path search takes it.
fn restrictor(r: WrittenRestrictor) -> Restrictor {
    match r {
        WrittenRestrictor::Walk => Restrictor::Walk,
        WrittenRestrictor::Trail => Restrictor::Trail,
        WrittenRestrictor::Acyclic => Restrictor::Acyclic,
        WrittenRestrictor::Simple => Restrictor::Simple,
    }
}

/// The selector written as path search takes it.
fn selector(s: &WrittenSelector) -> Selector {
    use crate::ast::Cheapest;
    match s {
        WrittenSelector::Any => Selector::Any(1),
        WrittenSelector::AnyK(k) => Selector::Any(u64::from(*k)),
        WrittenSelector::AnyShortest => Selector::AnyShortest,
        WrittenSelector::AllShortest => Selector::AllShortest,
        WrittenSelector::ShortestK(k) => Selector::Shortest(u64::from(*k)),
        WrittenSelector::ShortestGroups(k) => Selector::ShortestGroups(u64::from(*k)),
        WrittenSelector::Cheapest { which: Cheapest::Any, .. } => Selector::AnyCheapest,
        WrittenSelector::Cheapest { which: Cheapest::All, .. } => Selector::AllCheapest,
        WrittenSelector::Cheapest { which: Cheapest::K(k), .. } => Selector::Cheapest(u64::from(*k)),
    }
}
