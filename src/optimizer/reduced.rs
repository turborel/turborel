//! \[DESIGN\] Joins reduced to semi joins under set functions blind to
//! duplicates (RULES.md D542; Yan and Larson, VLDB 1995, §2.2; Lanzinger,
//! Pichler and Selzer, PVLDB 2025).
//!
//! MIN and MAX, and any set function of DISTINCT, give the same value over
//! a multiset as over its set of distinct values, so the rows a join
//! repeats need not be made. A grouping of no grouping columns whose
//! every set function is such reads the columns of some relations of the
//! chain of inner joins under it, the kept ones. Every other relation that
//! is an ear of the chain's join tree (`reduced/tree.rs`) joins its parent
//! as a semi join instead, its own ears joined into it first: the
//! bottom-up half of Yannakakis' algorithm, over the relations no output
//! reads. The rows of the kept relations that the chain joins are those
//! the reduced chain joins, each fewer times or once, and the set
//! functions' values are the same.
//!
//! It applies only where no conjunct of the chain can raise or holds a
//! subquery, as a semi join evaluates its condition on fewer pairs (D15).
//! A set function's argument and filter read kept relations alone, so
//! they are evaluated on the same values as before.

mod tree;

use super::{Rule, RuleContext};
use crate::catalog::Catalog;
use crate::functions::{Registry, Standard};
use crate::logical::{
    AggCall, Cond, Expr, LogicalPlan, Truth, and_all, cond_columns, expr_columns, map_cond_columns, plan_column_type, split_and,
};
use crate::value::Value;
use tree::{Edge, Graph};

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const SEMI_REDUCTION: &str = "semi_reduction";

/// The most relations of a chain the rule reduces, one bit of a set each.
const MOST_RELATIONS: usize = 64;

/// The rule that reduces joins to semi joins (see the module's
/// documentation).
pub struct SemiReduction;

impl Rule for SemiReduction {
    fn name(&self) -> &'static str {
        SEMI_REDUCTION
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Aggregate { groups, aggs, input, .. } = plan else { return None };
        if !groups.is_empty() || aggs.is_empty() || !aggs.iter().all(|a| blind(a, &ctx.db.functions)) {
            return None;
        }
        let mut read = Vec::new();
        let (exprs, chain) = if let LogicalPlan::Project { exprs, input } = &**input {
            exprs.iter().for_each(|e| expr_columns(e, &mut read));
            (Some(exprs), &**input)
        } else {
            aggs.iter().flat_map(AggCall::exprs).for_each(|e| expr_columns(e, &mut read));
            (None, &**input)
        };
        let (reduced, map) = reduce(chain, &read, &*ctx.db.catalog)?;
        let at = |c: usize| map.get(c).copied().flatten();
        let exprs = match exprs {
            Some(exprs) => exprs.iter().map(|e| crate::logical::map_expr_columns(e.clone(), &|c| at(c).unwrap_or(c))).collect(),
            None => (0..chain.width()).map(|c| at(c).map_or(Expr::Const(Value::Null), Expr::Column)).collect(),
        };
        let mut out = plan.clone();
        if let LogicalPlan::Aggregate { input, .. } = &mut out {
            *input = Box::new(LogicalPlan::Project { exprs, input: Box::new(reduced) });
        }
        Some(out)
    }
}

/// Whether the set function `a` gives the same value of a multiset as of
/// its set: MIN, MAX, or any of DISTINCT.
fn blind(a: &AggCall, functions: &Registry) -> bool {
    let extreme = matches!(functions.standard(&a.func), Some(Standard::Max | Standard::Min));
    a.distribution.is_none() && (a.distinct || extreme)
}

/// The relations of a chain: each plan, where its columns begin, and the
/// conjuncts of the chain over its columns.
#[derive(Default)]
struct Chain {
    leaves: Vec<(usize, LogicalPlan)>,
    conds: Vec<Cond>,
}

impl Chain {
    /// Takes `plan`, its columns beginning at `start`, apart; none past
    /// `MOST_RELATIONS`.
    fn take(&mut self, plan: &LogicalPlan, start: usize) -> Option<()> {
        if self.leaves.len() > MOST_RELATIONS {
            return None;
        }
        match plan {
            LogicalPlan::Product { left, right } => {
                self.take(left, start)?;
                self.take(right, start.saturating_add(left.width()))
            }
            LogicalPlan::Filter { cond, input } => {
                split_and(map_cond_columns(cond.clone(), &|i| i.saturating_add(start)), &mut self.conds);
                self.take(input, start)
            }
            LogicalPlan::Scan { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Project { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::OuterJoin { .. }
            | LogicalPlan::SemiJoin { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::Sort { .. }
            | LogicalPlan::Limit { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::WorkTable { .. }
            | LogicalPlan::Transition { .. }
            | LogicalPlan::Unnest(_)
            | LogicalPlan::Window(_)
            | LogicalPlan::Recognize(_)
            | LogicalPlan::Polymorphic(_) => {
                self.leaves.push((start, plan.clone()));
                Some(())
            }
        }
    }

    /// The relation whose column the chain's column `c` is.
    fn rel_of(&self, c: usize) -> Option<usize> {
        self.leaves.iter().position(|(start, plan)| c >= *start && c < start.saturating_add(plan.width()))
    }

    /// The relations `c` reads, as a set.
    fn rels(&self, c: &Cond) -> u64 {
        let mut columns = Vec::new();
        cond_columns(c, &mut columns);
        columns.iter().filter_map(|&col| self.rel_of(col)).fold(0, |set, r| set | bit(r))
    }
}

/// The bit of relation `r`.
fn bit(r: usize) -> u64 {
    u32::try_from(r).ok().and_then(|r| 1_u64.checked_shl(r)).unwrap_or(0)
}

/// The chain `plan` reduced, the columns `read` of it kept, and where each
/// of its columns is in the reduced one, if it is; none if no relation is
/// reduced.
fn reduce(plan: &LogicalPlan, read: &[usize], catalog: &dyn Catalog) -> Option<(LogicalPlan, Vec<Option<usize>>)> {
    let mut chain = Chain::default();
    chain.take(plan, 0)?;
    if chain.leaves.len() < 2 || chain.leaves.len() > MOST_RELATIONS || chain.conds.iter().any(|c| c.can_raise() || c.has_subquery()) {
        return None;
    }
    let kept = read.iter().filter_map(|&c| chain.rel_of(c)).fold(0, |set, r| set | bit(r));
    let edges: Vec<Edge> = chain.conds.iter().map(|c| Edge::of(c, chain.rels(c))).collect();
    let rel_of = |c: usize| chain.rel_of(c);
    let type_of = |c: usize| {
        let r = chain.rel_of(c)?;
        let (start, leaf) = chain.leaves.get(r)?;
        plan_column_type(leaf, c.checked_sub(*start)?, catalog)
    };
    let graph = Graph { edges: &edges, rel_of: &rel_of, type_of: &type_of };
    let all = (0..chain.leaves.len()).fold(0, |set, r| set | bit(r));
    let ears = graph.ears(all, kept);
    if ears.is_empty() {
        return None;
    }
    Some(rebuilt(&chain, &edges, &ears))
}

/// The chain with each ear of `ears` a semi join into its parent, the
/// relations left joined as before, and where each column is now.
fn rebuilt(chain: &Chain, edges: &[Edge], ears: &[tree::Ear]) -> (LogicalPlan, Vec<Option<usize>>) {
    let mut plans: Vec<Option<LogicalPlan>> = chain.leaves.iter().map(|(_, p)| Some(p.clone())).collect();
    let mut used = vec![false; edges.len()];
    // Each relation's own conjuncts filter it.
    for (i, (c, e)) in chain.conds.iter().zip(edges).enumerate() {
        if e.rels == 0 {
            let r = chain.rels(c).trailing_zeros();
            let at = usize::try_from(r).unwrap_or(usize::MAX);
            if let (Some(slot), Some((start, _))) = (plans.get_mut(at), chain.leaves.get(at)) {
                *slot = slot.take().map(|p| filtered(p, local(c.clone(), *start, None, chain)));
                if let Some(u) = used.get_mut(i) {
                    *u = true;
                }
            }
        }
    }
    for ear in ears {
        semi_into(chain, ear, &mut plans);
        for &e in ear.joined.iter().chain(&ear.implied) {
            if let Some(u) = used.get_mut(e) {
                *u = true;
            }
        }
    }
    joined_rest(chain, plans, &used)
}

/// `plans`, the ear `ear` joined into its parent's as a semi join on its
/// conjuncts with it.
fn semi_into(chain: &Chain, ear: &tree::Ear, plans: &mut [Option<LogicalPlan>]) {
    let (Some((rs, _)), Some((ps, parent))) = (chain.leaves.get(ear.rel), chain.leaves.get(ear.parent)) else { return };
    let width = parent.width();
    let on: Vec<Cond> =
        ear.joined.iter().filter_map(|&e| chain.conds.get(e)).map(|c| local(c.clone(), *ps, Some((*rs, width, ear.rel)), chain)).collect();
    let cond = and_all(on).unwrap_or(Cond::Const(Truth::True));
    let right = plans.get_mut(ear.rel).and_then(Option::take);
    let Some(slot) = plans.get_mut(ear.parent) else { return };
    if let (Some(left), Some(right)) = (slot.take(), right) {
        *slot = Some(LogicalPlan::SemiJoin { anti: false, cond, left: Box::new(left), right: Box::new(right) });
    }
}

/// `c`, over the chain's columns, over those of the relation beginning at
/// `start`, or of the pair of it and, `width` columns on, the relation
/// `rel` of `pair`, beginning at its first.
fn local(c: Cond, start: usize, pair: Option<(usize, usize, usize)>, chain: &Chain) -> Cond {
    map_cond_columns(c, &|i| {
        if let Some((other, width, rel)) = pair
            && chain.rel_of(i) == Some(rel)
        {
            return width.saturating_add(i.saturating_sub(other));
        }
        i.saturating_sub(start)
    })
}

/// `plan` filtered by `cond`.
fn filtered(plan: LogicalPlan, cond: Cond) -> LogicalPlan {
    LogicalPlan::Filter { cond, input: Box::new(plan) }
}

/// The relations of `plans` left, joined in their order, filtered by the
/// conjuncts not `used`, and where each column of the chain is now.
fn joined_rest(chain: &Chain, plans: Vec<Option<LogicalPlan>>, used: &[bool]) -> (LogicalPlan, Vec<Option<usize>>) {
    let width = chain.leaves.last().map_or(0, |(start, p)| start.saturating_add(p.width()));
    let mut map = vec![None; width];
    let mut joined: Option<LogicalPlan> = None;
    let mut at = 0_usize;
    for ((start, leaf), plan) in chain.leaves.iter().zip(plans) {
        let Some(plan) = plan else { continue };
        for c in 0..leaf.width() {
            if let Some(slot) = map.get_mut(start.saturating_add(c)) {
                *slot = Some(at.saturating_add(c));
            }
        }
        at = at.saturating_add(leaf.width());
        joined = Some(match joined {
            Some(left) => LogicalPlan::Product { left: Box::new(left), right: Box::new(plan) },
            None => plan,
        });
    }
    let rest: Vec<Cond> = chain.conds.iter().zip(used).filter(|(_, u)| !**u).map(|(c, _)| c.clone()).collect();
    let moved = |i: usize| map.get(i).copied().flatten().unwrap_or(i);
    let cond = and_all(rest.into_iter().map(|c| map_cond_columns(c, &moved)).collect());
    let plan = joined.unwrap_or(LogicalPlan::Values { rows: Vec::new(), types: Vec::new() });
    let plan = match cond {
        Some(cond) => filtered(plan, cond),
        None => plan,
    };
    (plan, map)
}
