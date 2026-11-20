//! \[DESIGN\] The share of rows a comparison of a column with a value
//! keeps, from the column's statistics (RULES.md D31, D381): a common
//! value its own share, any other the rest shared among the other
//! distinct values; an order, the common values below the value and the
//! histogram's share of the rest; none outside the least and greatest
//! values while they are bounds. Once rows were written since the pass
//! (`stats/since.rs`), a value past the greatest keeps at least what an
//! equality of it would, or, of a column growing with insertion, the
//! share of rows written since, not none.

use std::cmp::Ordering;

use super::ColumnStats;
use super::common::{share_all, share_below, share_of};
use super::since::Since;
use crate::value::{CompOp, Value, compare};

/// Where a value lies against a column's least and greatest values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Before,
    Within,
    Past,
}

impl ColumnStats {
    /// The fraction of rows for which "column `op` `value`" is true, a
    /// comparison with NULL being true for none [X3.135-1986 §5.11 GR2].
    pub fn selectivity(&self, op: CompOp, value: &Value) -> f64 {
        self.selectivity_since(op, value, Since::NONE)
    }

    /// Its distinct non-null values, at least one, as every divisor of
    /// them takes them, a column of nulls alone among them.
    pub fn distinct_at_least_one(&self) -> f64 {
        self.distinct.max(1.0_f64)
    }

    /// As `selectivity`, read after the changes `since`.
    pub fn selectivity_since(&self, op: CompOp, value: &Value, since: Since) -> f64 {
        if value.is_null() {
            return 0.0;
        }
        let present = (1.0_f64 - self.nulls).clamp(0.0, 1.0);
        let place = self.place(value);
        let bounded = !self.unbounded && !since.changed();
        let rest = (present - share_all(&self.common)).max(0.0_f64);
        let others = (self.distinct_now(since) - count_of(&self.common)).max(1.0_f64);
        let average = rest / others;
        let absent = self.min.is_none() || (bounded && place != Place::Within);
        let equal = share_of(&self.common, value).unwrap_or(if absent { 0.0_f64 } else { average });
        // The share below the value, which an equality does not ask.
        let below = || share_below(&self.common, value) + rest * self.histogram.below(value).unwrap_or(0.5_f64);
        let fraction = match op {
            CompOp::Eq => equal,
            CompOp::Ne => present - equal,
            CompOp::Lt => below(),
            CompOp::Le => below() + equal,
            CompOp::Gt => present - below() - equal,
            CompOp::Ge => present - below(),
        };
        let floor = if bounded { 0.0_f64 } else { self.beyond(op, place, (average, since)) };
        fraction.max(floor).clamp(0.0, 1.0)
    }

    /// Where `value` lies against its least and greatest values.
    fn place(&self, value: &Value) -> Place {
        if self.min.as_ref().is_some_and(|min| compare(value, min) == Some(Ordering::Less)) {
            Place::Before
        } else if self.max.as_ref().is_some_and(|max| compare(value, max) == Some(Ordering::Greater)) {
            Place::Past
        } else {
            Place::Within
        }
    }

    /// The least share an order comparison keeps of values beyond its
    /// least or greatest, they not being bounds: as an equality's,
    /// `average`; past the greatest of a column growing with insertion,
    /// the share of rows written since.
    fn beyond(&self, op: CompOp, place: Place, (average, since): (f64, Since)) -> f64 {
        let toward = match op {
            CompOp::Gt | CompOp::Ge => place == Place::Past,
            CompOp::Lt | CompOp::Le => place == Place::Before,
            CompOp::Eq | CompOp::Ne => false,
        };
        if !toward {
            return 0.0_f64;
        }
        let grown = place == Place::Past && self.grows() && since.rows > 0.0_f64;
        if grown { (since.changes / since.rows).max(average) } else { average }
    }
}

/// How many values `common` holds, as a count.
fn count_of(common: &[(Value, f64)]) -> f64 {
    super::parts::count(u64::try_from(common.len()).unwrap_or(u64::MAX))
}
