//! \[DESIGN\] Joins estimated by classes of equalities (RULES.md D302,
//! D370), the one formula the enumeration (`enumerate/model.rs`), the
//! greedy order and a filter over a product all use, so every way of
//! joining a set of relations gives it the same rows.
//!
//! An equality of two columns keeps one pair in as many as the more
//! distinct of them has values (Selinger et al., 1979, §4, F = 1/MAX(ICARD)).
//! Equalities chained through one value, as `a = b`, `b = c` and `a = c`
//! are, form a class, and a set's class keeps one row in as many as each
//! of its columns but the least distinct has values, so an equality the
//! others imply, which predicate transitivity adds, is not counted again.
//! Other conjuncts are taken as independent.

use super::lineage::{Over, one_table};
use super::{Card, Estimates, at_least_one};
use crate::logical::{Cond, Expr, LogicalPlan, cond_columns, map_cond_columns, split_and};
use crate::optimizer::is_guarded;
use crate::value::CompOp;

/// The classes of a set of equalities: of each, its members, a relation
/// and its column's distinct values, one for each relation.
#[derive(Debug, Clone, Default)]
pub(in crate::physical) struct Classes {
    members: Vec<Vec<(usize, f64)>>,
}

impl Classes {
    /// Of `members`, each class's.
    pub(in crate::physical) fn new(members: Vec<Vec<(usize, f64)>>) -> Classes {
        Classes { members }
    }

    /// The fraction of the product of the relations `has` holds that the
    /// equalities of the classes keep: of each class, one in as many as
    /// each of its members there but the least distinct has values.
    pub(in crate::physical) fn fraction(&self, has: impl Fn(usize) -> bool) -> f64 {
        self.members
            .iter()
            .map(|members| {
                let mut distinct: Vec<f64> = members.iter().filter(|(rel, _)| has(*rel)).map(|(_, d)| d.max(1.0_f64)).collect();
                distinct.sort_by(f64::total_cmp);
                distinct.iter().skip(1).map(|d| 1.0_f64 / d).product::<f64>()
            })
            .product()
    }
}

/// The root of `c` among `parents`, a union-find's.
pub(in crate::physical) fn find(parents: &[usize], mut c: usize) -> usize {
    for _ in 0..parents.len() {
        let Some(&p) = parents.get(c) else { break };
        if p == c {
            break;
        }
        c = p;
    }
    c
}

/// A chain of products and the filters over them, taken apart: its
/// relations, each with its first column, and its conjuncts over the
/// chain's columns.
struct Chain<'p> {
    leaves: Vec<(&'p LogicalPlan, usize)>,
    conds: Vec<Cond>,
}

impl Chain<'_> {
    /// The relation the chain's column `column` is of.
    fn rel_of(&self, column: usize) -> Option<usize> {
        self.leaves.iter().rposition(|&(_, start)| start <= column)
    }
}

/// Whether `plan` is a product, or a filter over a chain not over a guard
/// (`optimizer::is_guarded`, one relation).
pub(super) fn is_chain(plan: &LogicalPlan) -> bool {
    if let LogicalPlan::Filter { input, .. } = plan {
        return !is_guarded(plan) && is_chain(input);
    }
    matches!(plan, LogicalPlan::Product { .. })
}

/// Adds the relations of the chain `plan`, its columns from `start`, and
/// its conjuncts, to `chain`, below `depth` levels.
fn take_apart<'p>(plan: &'p LogicalPlan, start: usize, chain: &mut Chain<'p>, depth: usize) {
    if depth > super::MAX_ESTIMATE_DEPTH {
        chain.leaves.push((plan, start));
        return;
    }
    let next = depth.saturating_add(1);
    if let LogicalPlan::Product { left, right } = plan {
        take_apart(left, start, chain, next);
        take_apart(right, start.saturating_add(left.width()), chain, next);
    } else if let LogicalPlan::Filter { cond, input } = plan
        && is_chain(plan)
    {
        split_and(map_cond_columns(cond.clone(), &|c| c.saturating_add(start)), &mut chain.conds);
        take_apart(input, start, chain, next);
    } else {
        chain.leaves.push((plan, start));
    }
}

impl Estimates<'_> {
    /// The card of the chain `plan`, under the conjuncts of `cond` if
    /// given: its relations' product, kept by the classes of its
    /// equalities of two relations' columns, and by its other conjuncts
    /// each as a filter of the chain keeps it.
    pub(super) fn chain(&self, cond: Option<&Cond>, plan: &LogicalPlan, depth: usize) -> Card {
        let mut chain = Chain { leaves: Vec::new(), conds: Vec::new() };
        take_apart(plan, 0, &mut chain, depth);
        if let Some(c) = cond {
            split_and(c.clone(), &mut chain.conds);
        }
        let cards: Vec<Card> = chain.leaves.iter().map(|(leaf, _)| self.card_at(leaf, depth.saturating_add(1))).collect();
        let product = cards.iter().map(|c| c.rows).product::<f64>();
        let upper = cards.iter().map(|c| c.upper).product::<f64>();
        let mut parents: Vec<usize> = (0..span(&chain)).collect();
        let mut rest = 1.0_f64;
        let mut pairs = Vec::new();
        for c in &chain.conds {
            match equated(c, &chain) {
                Some((a, b)) => {
                    let (ra, rb) = (find(&parents, a), find(&parents, b));
                    if let Some(p) = parents.get_mut(ra) {
                        *p = rb;
                    }
                    pairs.push((a, b));
                }
                None => rest *= self.share(c, Over::One(plan)),
            }
        }
        let classes = self.classes_of(&chain, &parents, &pairs);
        let fraction = classes.fraction(|_| true) * rest;
        Card { rows: at_least_one(product * fraction, product), upper }
    }

    /// The classes of the equalities `equated` of `chain`, by `parents`:
    /// of each, a member of each relation, its column's distinct values.
    fn classes_of(&self, chain: &Chain<'_>, parents: &[usize], equated: &[(usize, usize)]) -> Classes {
        let mut roots: Vec<usize> = Vec::new();
        let mut members: Vec<Vec<(usize, f64)>> = Vec::new();
        for col in equated.iter().flat_map(|&(a, b)| [a, b]) {
            let (root, Some(rel)) = (find(parents, col), chain.rel_of(col)) else { continue };
            let Some(&(leaf, start)) = chain.leaves.get(rel) else { continue };
            let at = roots.iter().position(|&r| r == root).unwrap_or_else(|| {
                roots.push(root);
                members.push(Vec::new());
                roots.len().saturating_sub(1)
            });
            let Some(class) = members.get_mut(at) else { continue };
            if !class.iter().any(|(r, _)| *r == rel) {
                class.push((rel, self.distinct(leaf, col.saturating_sub(start))));
            }
        }
        Classes::new(members)
    }

    /// The fraction of `input`'s rows the equality of two of its columns
    /// `c` keeps, if it is one: one in as many as the more distinct has
    /// values.
    pub(super) fn equality_share(&self, over: Over<'_>, c: &Cond) -> Option<f64> {
        let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c else { return None };
        if one_table(over, &[*a, *b]).is_some() {
            return None;
        }
        let distinct = |column: usize| {
            let (plan, at) = over.side(column);
            self.distinct(plan, at)
        };
        Some(1.0_f64 / distinct(*a).max(distinct(*b)).max(1.0_f64))
    }

}

/// The width of the chain's columns.
fn span(chain: &Chain<'_>) -> usize {
    chain.leaves.last().map_or(0, |(leaf, start)| start.saturating_add(leaf.width()))
}

/// The columns of `c`, if it is an equality of columns of two of the
/// chain's relations.
fn equated(c: &Cond, chain: &Chain<'_>) -> Option<(usize, usize)> {
    let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c else { return None };
    (chain.rel_of(*a)? != chain.rel_of(*b)?).then_some((*a, *b))
}

/// The columns `c` reads.
pub(super) fn columns_of(c: &Cond) -> Vec<usize> {
    let mut columns = Vec::new();
    cond_columns(c, &mut columns);
    columns.sort_unstable();
    columns.dedup();
    columns
}
