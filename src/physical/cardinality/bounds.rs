//! \[DESIGN\] Upper bounds of rows (RULES.md D385), beside each estimate,
//! which no estimate passes.
//!
//! A table's read is at most its rows. A filter holding an equality of a
//! column to one value, a constant, a parameter or an outer reference,
//! keeps at most the most rows of one value of that column, and one row
//! if such columns hold a key. A join of a set of relations to one more
//! on an equality gives each row of the set at most the most rows of one
//! value of the new relation's column, 1 of a key: the degree bound of
//! Cai, Balazinska and Suciu, "Pessimistic Cardinality Estimation",
//! SIGMOD 2019, and Hertzschuch et al., "Simplicity Done Right for Join
//! Ordering", CIDR 2021. The most rows of one value are the statistics'
//! (`Frequencies::most`), scaled from a sample to the table, and of a
//! filtered or derived relation still at most that, as a filter only
//! drops rows. What has no statistic is bounded by its relation's bound.
//!
//! The bound is the planner's proof as far as its statistics are true, so
//! it clamps the estimate and tells how far an estimate may be wrong
//! (`physical/robust.rs`); it is never taken as the estimate, which at
//! depth it overstates many times.

use super::lineage::origin;
use super::{Card, Estimates};
use crate::logical::{Cond, Expr, LogicalPlan, split_and};
use crate::value::CompOp;

/// An equality of two relations' columns: of each side, the relation and
/// its column's place among the relation's.
pub(super) type Equality = ((usize, usize), (usize, usize));

/// The column `c` holds equal to one value, if it is such an equality.
fn pinned(cond: &Cond) -> Option<usize> {
    let Cond::Compare(left, CompOp::Eq, right) = cond else { return None };
    let one = |e: &Expr| matches!(e, Expr::Const(_) | Expr::Outer(..) | Expr::Dynamic(_));
    let (column, value) = if let Expr::Column(at) = left {
        (*at, right)
    } else if let Expr::Column(at) = right {
        (*at, left)
    } else {
        return None;
    };
    one(value).then_some(column)
}

impl Estimates<'_> {
    /// The most rows of `plan` one value of its column `column` is in: 1
    /// if it is a key, else as its table's statistics count, at most
    /// `plan`'s bound.
    pub(in crate::physical) fn most_of(&self, plan: &LogicalPlan, column: usize) -> f64 {
        self.kept_most(plan, column, || self.most_found(plan, column))
    }

    /// The most rows of `most_of`, found.
    fn most_found(&self, plan: &LogicalPlan, column: usize) -> f64 {
        if self.key_of(plan, &[column]).is_some() {
            return 1.0_f64;
        }
        let upper = self.card_at(plan, 1).upper;
        let counted = origin(plan, column).and_then(|o| self.statistics(o.table)?.frequencies(o.column));
        counted.map_or(upper, |f| f.most.max(1.0_f64).min(upper))
    }

    /// The bound of the rows of the filter of `cond` over `input`, as the
    /// module says; infinite if none of its conjuncts tells one.
    pub(super) fn filter_upper(&self, cond: &Cond, input: &LogicalPlan) -> f64 {
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let mut columns: Vec<usize> = conjuncts.iter().filter_map(pinned).collect();
        columns.sort_unstable();
        columns.dedup();
        if columns.is_empty() {
            return f64::INFINITY;
        }
        if self.key_of(input, &columns).is_some() {
            return 1.0_f64;
        }
        columns.iter().map(|&c| self.most_of(input, c)).fold(f64::INFINITY, f64::min)
    }

    /// The bound of the join of the relations `leaves`, of cards `cards`,
    /// by the equalities `keys`, each a relation and its column on either
    /// side: from the relation of least bound, each relation an equality
    /// joins to those reached multiplying the bound by the most rows of
    /// one value of its column, the least such first; a relation no
    /// equality reaches by its own bound. At most the bounds' product.
    pub(super) fn chain_upper(&self, leaves: &[&LogicalPlan], cards: &[Card], keys: &[Equality]) -> f64 {
        let product = cards.iter().map(|c| c.upper).product::<f64>();
        let least = |reached: &[bool]| {
            let open = cards.iter().enumerate().filter(|(r, _)| !reached.get(*r).copied().unwrap_or(true));
            open.min_by(|a, b| a.1.upper.total_cmp(&b.1.upper)).map(|(r, c)| (r, c.upper))
        };
        let mut reached = vec![false; cards.len()];
        let mut bound = 1.0_f64;
        // Each equality's most rows of one value of either side, asked once.
        let most = |(to, column): (usize, usize)| leaves.get(to).map(|leaf| self.most_of(leaf, column));
        let steps: Vec<((usize, usize), Option<f64>)> =
            keys.iter().flat_map(|&(a, b)| [(a, b), (b, a)]).map(|(f, t)| ((f.0, t.0), most(t))).collect();
        while let Some(open) = least(&reached) {
            let joined = steps
                .iter()
                .filter(|((from, to), _)| reached.get(*from).copied().unwrap_or(false) && !reached.get(*to).copied().unwrap_or(true))
                .filter_map(|&((_, to), times)| Some((to, times?)))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let (rel, times) = joined.unwrap_or(open);
            if let Some(r) = reached.get_mut(rel) {
                *r = true;
            }
            bound *= times;
        }
        bound.min(product)
    }
}
