//! \[DESIGN\] Semi joins and antijoins planned (RULES.md D540): the rows of
//! the left operand that some row of the right matches, or that none does,
//! each once, in the left's order but of a right semi join.
//!
//! * A hash semi join builds the right's rows by its keys, the equalities
//!   of a column of each operand, and stops at the first match of each
//!   left row (`exec/join/semi.rs`). Of no other condition only the
//!   first row of each key is kept.
//! * A right semi join builds the left instead, when the left is the
//!   operand of fewer estimated rows, each row with a mark of whether a
//!   right row matched it; the right is read once, and the left's rows
//!   given in their order, those marked, or of an antijoin those not.
//! * An index semi join probes an index of the right's table for each
//!   left row and stops at the first visible row the condition is true of
//!   (`Preserved::Semi`, `exec/join/indexed.rs`), when the left's rows are
//!   estimated few enough (`probed_instead`).
//! * A nested-loop semi join, of a condition of no equality, holds the
//!   right's rows and reads them for each left row until one matches.
//!
//! The condition beyond the keys is evaluated only on the pairs they keep,
//! and of those only until one matches, so the optimizer makes semi joins
//! only of conditions that cannot raise (D15).

mod probed;

use super::built::Built;
use super::indexed::{EquiKind, Equijoin, Probing, index_join};
use super::join::{JoinKey, equated, keys_detail};
use super::shown::Shown;
use super::{PhysicalPlanner, Plan, of_left};
use crate::logical::{Cond, LogicalPlan, SortKey, and_all, judged_by_sets, render_cond, split_and};

/// \[DESIGN\] How a semi join or an antijoin is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemiMethod {
    /// The right built by its keys, each left row probing it.
    Hash,
    /// The left built, each row marked as a right row matches it.
    BuildLeft,
    /// The right held, each left row read against its rows.
    Nested,
}

/// \[DESIGN\] A semi join or an antijoin (`LogicalPlan::SemiJoin`).
#[derive(Debug, Clone)]
pub struct SemiJoin {
    /// An antijoin: a left row is kept if no right row matches it.
    pub anti: bool,
    /// How it is made.
    pub method: SemiMethod,
    /// The rows kept or not.
    pub left: Plan,
    /// The rows matched against.
    pub right: Plan,
    /// Pairs of a column of `left` and one of `right` whose values match.
    pub keys: Vec<JoinKey>,
    /// The rest of the condition, over the pair's columns.
    pub cond: Option<Cond>,
    /// The degrees of `left` and `right`.
    pub widths: (usize, usize),
    /// The condition, as text.
    pub detail: String,
    /// Its estimated rows (`physical/cardinality/semi.rs`).
    pub est: f64,
}

impl SemiJoin {
    /// The name EXPLAIN gives it.
    pub fn operator(&self) -> &'static str {
        match (self.method, self.anti) {
            (SemiMethod::Hash, false) => "HashSemiJoin",
            (SemiMethod::Hash, true) => "HashAntiJoin",
            (SemiMethod::BuildLeft, false) => "HashRightSemiJoin",
            (SemiMethod::BuildLeft, true) => "HashRightAntiJoin",
            (SemiMethod::Nested, false) => "NestedLoopSemiJoin",
            (SemiMethod::Nested, true) => "NestedLoopAntiJoin",
        }
    }
}

/// An index semi join's probing, its left's estimated rows, the equalities
/// it probes by and the rest of its condition.
type IndexSemi = (Probing, f64, Vec<(usize, usize)>, Vec<Cond>);

/// What a semi join is made of: whether an antijoin, its operands' plans,
/// its condition over the pair's columns, and their degrees.
pub(super) struct Semi {
    pub(super) anti: bool,
    pub(super) cond: Cond,
    pub(super) widths: (usize, usize),
    /// Whether the left is built, a right semi join, if it has keys.
    pub(super) build_left: bool,
}

/// The semi join `semi` of `left` and `right`, described as `shown`
/// says of the pair's columns, estimated to give `est` rows.
pub(super) fn semi_plan(semi: Semi, (left, right): (Plan, Plan), (shown, est): (Shown<'_>, f64)) -> Plan {
    let Semi { anti, cond, widths, build_left } = semi;
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    let (mut keys, mut rest) = (Vec::new(), Vec::new());
    for c in conjuncts {
        match equated(&c, widths.0) {
            Some(key) => keys.push(key),
            None => rest.push(c),
        }
    }
    let method = match (keys.is_empty(), build_left) {
        (true, true | false) => SemiMethod::Nested,
        (false, true) => SemiMethod::BuildLeft,
        (false, false) => SemiMethod::Hash,
    };
    let rendered = shown.text(|names| render_cond(&cond, names));
    let detail = if keys.is_empty() {
        rendered
    } else {
        shown.text(|names| format!("{rendered}  (hashed on {})", keys_detail(&keys, names, widths.0)))
    };
    let residual = and_all(rest).map(judged_by_sets);
    Plan::SemiJoin(Box::new(SemiJoin { anti, method, left, right, keys, cond: residual, widths, detail, est }))
}

impl PhysicalPlanner<'_> {
    /// The semi join, or of `anti` the antijoin, of `left` and `right` on
    /// `cond`, not of a chain the enumeration plans: by an index of the
    /// right's table if its probes cost less, else hashed, the left built
    /// if it has fewer rows than the right and its order is not asked.
    pub(super) fn semi_join(&self, (anti, cond): (bool, &Cond), (left, right): (&LogicalPlan, &LogicalPlan), want: &[SortKey]) -> Built {
        let width = left.width();
        let est = self.estimating(|e| e.semi_card((anti, cond), (left, right))).rows;
        let both = self.shown.then(|| left.names().into_iter().chain(right.names()).collect::<Vec<String>>());
        let shown = Shown::of(both.as_deref());
        let want_left = of_left(want, width);
        let (l, r) = (self.build_node(left, want_left), self.build_node(right, &[]));
        let ordered = !want_left.is_empty() && l.ordered;
        if let Some((probing, rows, pairs, rest)) = self.index_semi(cond, (left, right), &r.plan) {
            let kind = if anti { EquiKind::Anti } else { EquiKind::Semi };
            let join = Equijoin { width, keys: &pairs, rest, outer: kind };
            return Built { plan: index_join(probing, join, (l.plan, rows), shown), ordered };
        }
        // The right's few rows probing the left's table (`semi/probed.rs`).
        if let Some(built) = (!anti).then(|| self.probed_from_right(cond, (left, right))).flatten() {
            return built;
        }
        // Either way its rows come in the left's order.
        let build_left = self.rows_of(left) < self.rows_of(right);
        let semi = Semi { anti, cond: cond.clone(), widths: (width, right.width()), build_left };
        Built { plan: semi_plan(semi, (l.plan, r.plan), (shown, est)), ordered }
    }

    /// How `left`'s rows probe an index of the table `right` reads, its
    /// estimated rows, the equalities of `cond` probed and the rest, if it
    /// has an equality of a column of each and the probes cost less than
    /// the right's read `read`.
    fn index_semi(&self, cond: &Cond, (left, right): (&LogicalPlan, &LogicalPlan), read: &Plan) -> Option<IndexSemi> {
        let width = left.width();
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let (mut pairs, mut rest) = (Vec::new(), Vec::new());
        for c in conjuncts {
            match equated(&c, width) {
                Some(JoinKey { left: a, right: b, nulls: false }) => pairs.push((a, b)),
                Some(JoinKey { nulls: true, .. }) | None => rest.push(c),
            }
        }
        if pairs.is_empty() {
            return None;
        }
        let (probing, rows) = self.probed_instead((left, right), &pairs, read)?;
        Some((probing, rows, pairs, rest))
    }
}
