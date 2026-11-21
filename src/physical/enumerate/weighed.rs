//! \[DESIGN\] What a hash join and a sort cost in the enumeration beyond
//! their rows (RULES.md D651 to D653): the bytes the operator holds past
//! its grant, spilled and read back (`cost/spill.rs`); the threads of the
//! part a hash join runs in sharing its work (`cost/parallel.rs`); and the
//! probing rows its runtime filter drops before they are hashed
//! (`cost/runtime.rs`). Each relation carries the rows its read would give
//! a parallel part, its stream's, and the bytes each of its rows holds, and
//! a set of relations the most rows of one's stream and the sum of their
//! bytes, as the pipeline of joins over them reads and holds them.

#[cfg(test)]
mod tests;

use super::bits::{Set, bit, members};
use super::conflicts::Kind;
use super::model::{Model, Rel};
use super::space::{Entry, Space};
use crate::logical::LogicalPlan;
use crate::physical::PhysicalPlanner;
use crate::physical::cost::hashing::hash_row;
use crate::physical::cost::parallel::{PART_START, gain_of};
use crate::physical::cost::runtime::{kept, probed};
use crate::physical::cost::spill::hash_spilled;
use crate::physical::robust::Linear;
use crate::storage::cost_of;

impl PhysicalPlanner<'_> {
    /// `rel`, relation `plan` of `width` columns, with the rows its read
    /// in no order gives a parallel part, the bytes its rows hold
    /// (`PhysicalPlanner::row_bytes_of`), and what its part's threads gain.
    pub(super) fn weigh(&self, rel: &mut Rel, (plan, width): (&LogicalPlan, usize)) {
        rel.held = rel.plans.first().and_then(|(_, read)| crate::physical::parallel::stream(read)).map_or(0.0_f64, cost_of);
        let LogicalPlan::Scan { table, .. } = scanned(plan) else { return };
        rel.bytes = self.row_bytes_of(table, width);
        rel.gain = gain_of(self.db, table);
    }
}

/// The table read under `plan`'s filters, or `plan`.
fn scanned(plan: &LogicalPlan) -> &LogicalPlan {
    let mut at = plan;
    while let LogicalPlan::Filter { input, .. } = at {
        at = input;
    }
    at
}

impl Model {
    /// The most rows the stream of one relation of `s` reads, the rows of
    /// the part reading their joins.
    pub(super) fn held(&self, s: Set) -> f64 {
        members(s).filter_map(|r| self.rels.get(r)).map(|r| r.held).fold(0.0_f64, f64::max)
    }

    /// The least share of each thread past the first a part reading the
    /// relations of `s` gains, the slowest's.
    pub(super) fn gain(&self, s: Set) -> f64 {
        members(s).filter_map(|r| self.rels.get(r)).map(|r| r.gain).fold(1.0_f64, f64::min)
    }

    /// The bytes a row of the join of the relations of `s` holds.
    pub(super) fn bytes(&self, s: Set) -> f64 {
        members(s).filter_map(|r| self.rels.get(r)).map(|r| r.bytes).sum()
    }

    /// The cost of sorting `rows` rows of the join of `s`: compared, and
    /// past the grant written and merged (RULES.md D651).
    pub(super) fn sorted(&self, s: Set, rows: f64) -> f64 {
        self.cost.sort_held(rows, self.bytes(s))
    }

    /// The share of the probing rows of `probing` a runtime filter of the
    /// built `built`, giving `rows` rows, keeps: of each class of the
    /// inner joins' equalities with members on both sides, the built side's
    /// least distinct, at most its rows, over the probing side's.
    fn filter_kept(&self, (built, rows): (Set, f64), probing: Set) -> f64 {
        let least = |members: &[(usize, f64)], s: Set| {
            members.iter().filter(|(r, _)| bit(*r) & s != 0).map(|&(_, d)| d).fold(f64::INFINITY, f64::min)
        };
        let classes: Vec<(f64, f64)> = self
            .classes
            .members()
            .iter()
            .map(|m| (least(m, built), least(m, probing)))
            .filter(|(b, p)| b.is_finite() && p.is_finite())
            .collect();
        kept(&classes, rows)
    }
}

impl Space<'_> {
    /// The cost of a hash join of the left `l` and the right `r`, a join
    /// of `kind`, the right's best plan costing `read`, linear in the
    /// left's rows: the right read, its part's threads sharing the read if
    /// it is one relation; each row of both hashed, shared by its part's
    /// threads, those of the probing operand, the larger, after its runtime
    /// filter; past the grant both operands spilled; and a part's start.
    pub(super) fn hash_linear(&self, (left, right): (&Entry, &Entry), (l, r): (Set, Set), (kind, read): (Kind, f64)) -> Linear {
        let (m, w) = (self.model, &self.model.cost);
        let probes_left = left.rows >= right.rows;
        let (built, probing) = if probes_left { ((r, right.rows), l) } else { ((l, left.rows), r) };
        let share = if kind == Kind::Inner && w.runtime_filters { m.filter_kept(built, probing) } else { 1.0_f64 };
        let each = hash_row(built.1 * m.bytes(built.0));
        let per_row = |probes: bool| if probes { probed(1.0_f64, each, share) } else { each };
        let (on_left, on_right) = (w.degrees.speedup(m.held(l), m.gain(l)), w.degrees.speedup(m.held(r), m.gain(r)));
        let right_read = if Space::single(r).is_some() { read / on_right } else { read };
        let threads = w.degrees.degree_of(m.held(l)).max(w.degrees.degree_of(m.held(r)));
        let start = if threads > 1.0_f64 { PART_START } else { 0.0_f64 };
        let spill = hash_spilled((built.1, m.bytes(built.0)), left.rows.max(right.rows), w.grant_bytes);
        Linear {
            fixed: right_read + per_row(!probes_left) * right.rows / on_right + start + spill,
            per_row: per_row(probes_left) / on_left,
        }
    }
}
