//! The weights of the cost model the planner cannot get from a plugin, and
//! what it guesses when statistics do not tell, each a setting's (RULES.md
//! D36).

#[cfg(test)]
mod alone_tests;
pub(super) mod conjuncts;
mod hashed;
pub(crate) mod parallel;
pub(crate) mod runtime;
pub(crate) mod spill;
#[cfg(test)]
mod tests;

use crate::error::{Code, Result, fail};

/// \[DESIGN\] The default cost of one level of an index descent, its page
/// held by the buffer pool: as much as reading six rows in sequence, a
/// descent of a tree and a probe begun (`bench/kept.rs`, its weights).
pub const DEFAULT_INDEX_DESCENT: f64 = 6.0;
/// \[DESIGN\] The cost of a row a hash join hashes, built into its table
/// or probing it, in rows of a scan (`bench/kept.rs`, its weights).
pub(super) const HASH_ROW: f64 = 0.5;
/// \[DESIGN\] The default cost of one comparison in a sort: a tenth of reading
/// one row, since it touches no storage.
pub const DEFAULT_SORT_COMPARE: f64 = 0.1;
/// \[DESIGN\] The default fraction an equality keeps when nothing is known:
/// one row in a hundred.
pub const DEFAULT_EQUAL: f64 = 0.01;
/// \[DESIGN\] The default fraction an order comparison keeps when nothing
/// is known: a third.
pub const DEFAULT_RANGE: f64 = 1.0 / 3.0;
/// \[DESIGN\] The default fraction of a column's values null when nothing
/// is known.
pub const DEFAULT_NULL: f64 = 0.01;
/// \[DESIGN\] The default of how much cheaper than the FROM clause's order
/// a join order must be estimated, to be taken (`physical/order.rs`).
/// Strictly less than 1, so a chain already in its best order is never
/// reordered again.
pub const DEFAULT_REORDER_GAIN: f64 = 0.9;
/// \[DESIGN\] The default of how many times its estimate an operand's rows
/// are taken to be at worst, as a hash join and an index nested-loop join
/// are weighed (`physical/robust.rs`, RULES.md D386): eight, about the
/// median error Leis et al., "How Good Are Query Optimizers, Really?",
/// 2015, §3, measure of the estimates of joins of three relations.
pub const DEFAULT_ESTIMATE_MARGIN: f64 = 8.0;
/// The most an estimate's margin may be: past it every estimate would be
/// taken as unbounded.
pub const MOST_ESTIMATE_MARGIN: f64 = 1_000_000.0;

/// \[DESIGN\] The default of the bytes an operator is taken to be granted:
/// MEMORY.WORK_BYTES's default (RULES.md D298).
pub(crate) const DEFAULT_GRANT_BYTES: f64 = 268_435_456.0;

/// Weights the planner cannot get from a plugin. Units are those of the
/// storage engines: 1.0 is reading one whole row sequentially.
///
/// Invariant: every weight is finite and not negative, and every fraction
/// from 0 to 1, the gain below 1, the margin at least 1. A NaN or negative weight would make plan
/// choice meaningless; results would still be right, because every plan
/// computes the same answer.
#[derive(Debug, Clone)]
pub struct CostModel {
    /// Cost per level of an index descent; a descent costs this × log2(rows).
    pub index_descent: f64,
    /// Cost per comparison when sorting; a sort of n rows costs this × n × log2(n).
    pub sort_compare: f64,
    /// The fraction an equality keeps when the statistics do not tell.
    pub equal_selectivity: f64,
    /// The fraction an order comparison, a pattern, a truth test or a
    /// subquery predicate keeps when the statistics do not tell.
    pub range_selectivity: f64,
    /// The fraction of a column's values null when the statistics do not
    /// tell.
    pub null_fraction: f64,
    /// How much cheaper than the FROM clause's order a join order must be
    /// estimated, to be taken.
    pub reorder_gain: f64,
    /// How many times its estimate an operand's rows are taken to be at
    /// worst, as a join's method is chosen; 1 to trust the estimate.
    pub estimate_margin: f64,
    /// \[DESIGN\] (RULES.md D651) The bytes an operator would be granted
    /// as the statement is planned (`cost/spill.rs`).
    pub grant_bytes: f64,
    /// \[DESIGN\] (RULES.md D652) The degrees the statement's parts would
    /// run at (`cost/parallel.rs`).
    pub degrees: parallel::Degrees,
    /// \[DESIGN\] (RULES.md D653) Whether hash joins over batches make
    /// runtime filters (`cost/runtime.rs`).
    pub runtime_filters: bool,
}

impl Default for CostModel {
    fn default() -> Self {
        CostModel {
            index_descent: DEFAULT_INDEX_DESCENT,
            sort_compare: DEFAULT_SORT_COMPARE,
            equal_selectivity: DEFAULT_EQUAL,
            range_selectivity: DEFAULT_RANGE,
            null_fraction: DEFAULT_NULL,
            reorder_gain: DEFAULT_REORDER_GAIN,
            estimate_margin: DEFAULT_ESTIMATE_MARGIN,
            grant_bytes: DEFAULT_GRANT_BYTES,
            degrees: parallel::Degrees::default(),
            runtime_filters: true,
        }
    }
}

impl CostModel {
    /// Checks the invariant: every weight finite and not negative, every
    /// fraction from 0 to 1, the gain below 1, the margin at least 1.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 naming the first weight or fraction out of its range.
    pub fn check(&self) -> Result<()> {
        for (name, weight) in
            [("index_descent", self.index_descent), ("sort_compare", self.sort_compare), ("grant_bytes", self.grant_bytes)]
        {
            if !weight.is_finite() || weight < 0.0_f64 {
                return fail(Code::Api, format!("[DESIGN] the cost weight {name} is {weight}; weights are finite and not negative"));
            }
        }
        let fractions = [
            ("equal_selectivity", self.equal_selectivity, 1.0_f64),
            ("range_selectivity", self.range_selectivity, 1.0_f64),
            ("null_fraction", self.null_fraction, 1.0_f64),
            ("reorder_gain", self.reorder_gain, 0.999_f64),
        ];
        for (name, fraction, most) in fractions {
            if !(0.0_f64..=most).contains(&fraction) {
                return fail(Code::Api, format!("[DESIGN] {name} is {fraction}; it is from 0 to {most}"));
            }
        }
        if !(1.0_f64..=MOST_ESTIMATE_MARGIN).contains(&self.estimate_margin) {
            return fail(
                Code::Api,
                format!("[DESIGN] estimate_margin is {}; it is from 1 to {MOST_ESTIMATE_MARGIN}", self.estimate_margin),
            );
        }
        Ok(())
    }

    /// The cost of sorting `rows` rows of `bytes` bytes each: comparing
    /// them, and writing and merging runs past the grant (RULES.md D651).
    pub fn sort_held(&self, rows: f64, bytes: f64) -> f64 {
        self.sort(rows) + spill::sort_spilled((rows, bytes), self.grant_bytes)
    }

    /// The cost of sorting `rows` rows.
    pub fn sort(&self, rows: f64) -> f64 {
        let n = rows.max(0.0);
        self.sort_compare * n * (n + 1.0).log2()
    }
}
