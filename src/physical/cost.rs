//! The weights of the cost model the planner cannot get from a plugin, and
//! what it guesses when statistics do not tell, each a setting's (RULES.md
//! D36). Split from physical.rs (RULES.md R-SIZE-2).

use crate::error::{Code, Result, fail};

/// \[DESIGN\] The default cost of one level of an index descent: as much as
/// reading one row in sequence.
pub const DEFAULT_INDEX_DESCENT: f64 = 1.0;
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

/// Weights the planner cannot get from a plugin. Units are those of the
/// storage engines: 1.0 is reading one whole row sequentially.
///
/// Invariant: every weight is finite and not negative, and every fraction
/// from 0 to 1, the gain below 1. A NaN or negative weight would make plan
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
    /// The fraction an order comparison, a pattern or a subquery keeps when
    /// the statistics do not tell.
    pub range_selectivity: f64,
    /// The fraction of a column's values null when the statistics do not
    /// tell.
    pub null_fraction: f64,
    /// How much cheaper than the FROM clause's order a join order must be
    /// estimated, to be taken.
    pub reorder_gain: f64,
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
        }
    }
}

impl CostModel {
    /// Checks the invariant: every weight finite and not negative, every
    /// fraction from 0 to 1, the gain below 1.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 naming the first weight or fraction out of its range.
    pub fn check(&self) -> Result<()> {
        for (name, weight) in [("index_descent", self.index_descent), ("sort_compare", self.sort_compare)] {
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
        Ok(())
    }

    /// The cost of sorting `rows` rows.
    pub fn sort(&self, rows: f64) -> f64 {
        let n = rows.max(0.0);
        self.sort_compare * n * (n + 1.0).log2()
    }
}
