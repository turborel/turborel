//! \[DESIGN\] How often a column's values repeat (RULES.md D151): of the
//! rows holding each distinct non-null value, the most, the median and
//! the 90th and 99th percentiles. Of an edge table's endpoint column they
//! are its degree distribution, each vertex with an edge counted once
//! (`stats/graph.rs`).
//!
//! They are drawn from the column's sample (`parts::Reservoir`), sorted:
//! each run of equal values is one value's repeats. A sample of every
//! value gives them exactly; a sample of some scales each run by the
//! values in all to those in the sample, so a value seen once in a large
//! table counts as many rows as the sample stands for.

use std::cmp::Ordering;

use super::parts::count;
use crate::value::{Value, compare};

/// \[DESIGN\] The percentiles kept, in hundredths.
const MEDIAN: u64 = 50;
/// The 90th percentile.
const P90: u64 = 90;
/// The 99th percentile.
const P99: u64 = 99;
/// The hundredths a percentile is of.
const HUNDRED: u64 = 100;

/// How often a column's distinct values repeat: rows a value is in.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Frequencies {
    /// The most rows of one value.
    pub most: f64,
    /// The median of each value's rows.
    pub median: f64,
    /// Their 90th percentile.
    pub p90: f64,
    /// Their 99th percentile.
    pub p99: f64,
}

impl Frequencies {
    /// Of `sorted`, a sample in ascending order of `present` non-null
    /// values: each run of equal values one value's rows, scaled by
    /// `present` to the sample's length. None of an empty sample.
    pub fn of_sorted(sorted: &[Value], present: f64) -> Option<Frequencies> {
        let mut runs: Vec<u64> = Vec::new();
        let mut last: Option<&Value> = None;
        for value in sorted {
            let same = last.is_some_and(|prev| compare(prev, value) == Some(Ordering::Equal));
            if let Some(run) = runs.last_mut().filter(|_| same) {
                *run = run.saturating_add(1);
            } else {
                runs.push(1);
            }
            last = Some(value);
        }
        if runs.is_empty() {
            return None;
        }
        runs.sort_unstable();
        let scale = present / count(u64::try_from(sorted.len()).unwrap_or(u64::MAX)).max(1.0_f64);
        let at = |hundredths: u64| {
            let n = u64::try_from(runs.len()).unwrap_or(u64::MAX);
            // The run at that share of the runs, the least first.
            let place = n.saturating_sub(1).saturating_mul(hundredths).checked_div(HUNDRED).unwrap_or(0);
            let run = usize::try_from(place).ok().and_then(|p| runs.get(p)).copied().unwrap_or(0);
            count(run) * scale
        };
        Some(Frequencies { most: at(HUNDRED), median: at(MEDIAN), p90: at(P90), p99: at(P99) })
    }

    /// Each scaled by `factor`, as a sample's are to a table's.
    #[must_use]
    pub fn scaled(self, factor: f64) -> Frequencies {
        Frequencies { most: self.most * factor, median: self.median * factor, p90: self.p90 * factor, p99: self.p99 * factor }
    }
}
