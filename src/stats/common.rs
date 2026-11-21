//! \[DESIGN\] A column's most common values (RULES.md D381): up to
//! `COMMON_VALUES` values, each with the share of the table's rows it
//! holds, drawn from the column's sample, and a histogram of the rest
//! (an end-biased histogram, Ioannidis and Poosala, "Balancing histogram
//! optimality and practicality for query result size estimation",
//! SIGMOD 1995). A histogram alone gives every value of a bucket the
//! bucket's average share, so a value held by half the rows is taken to
//! be as rare as any other; kept apart, it is estimated by its own share,
//! and the others by what is left.
//!
//! A value is common when the sample holds it at least twice and at
//! least `COMMON_SHARE` times the share each distinct value would hold
//! were all as frequent. The sample is sorted, so each value's repeats
//! are one run of it, and the list costs no second pass. A character
//! string of `parts::SAMPLED_CHARACTERS` or more characters, a binary
//! string as long as `parts::SAMPLED_OCTETS` and a large object are
//! never common, as the sample keeps them cut and no value would equal
//! the kept one.

use std::cmp::Ordering;

use super::parts::{SAMPLED_CHARACTERS, SAMPLED_OCTETS, count};
use crate::value::{Value, compare};

/// \[DESIGN\] The most common values a column keeps: enough for the
/// skewed columns of the shapes the planner meets (a status, a country,
/// a few heavy keys), few enough that looking one up costs little.
pub const COMMON_VALUES: usize = 32;
/// \[DESIGN\] How many times the average share of a distinct value a
/// value's share must be for it to be kept as common: one and a half, so
/// a column of evenly spread values keeps none and its histogram is the
/// whole answer.
pub const COMMON_SHARE: f64 = 1.5;
/// The fewest times the sample must hold a value for it to be common:
/// a value seen once may be no more frequent than any other.
const LEAST_SEEN: usize = 2;

/// A column's most common values and the share of all rows each holds.
pub type Common = Vec<(Value, f64)>;

/// Of `sorted`, a sample in ascending order of non-null values, `present`
/// the share of rows not null and `distinct` the column's distinct
/// values: its most common values, each with its share of all rows, the
/// most frequent first, and the sample without them, still sorted.
pub fn common_of(sorted: Vec<Value>, present: f64, distinct: f64) -> (Common, Vec<Value>) {
    let n = count(u64::try_from(sorted.len()).unwrap_or(u64::MAX));
    if n < 1.0_f64 {
        return (Vec::new(), sorted);
    }
    let least = COMMON_SHARE / distinct.max(1.0_f64);
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start = 0_usize;
    for at in 1..=sorted.len() {
        let same = sorted.get(at).zip(sorted.get(start)).is_some_and(|(a, b)| compare(a, b) == Some(Ordering::Equal));
        if same {
            continue;
        }
        let length = at.saturating_sub(start);
        let share = count(u64::try_from(length).unwrap_or(u64::MAX)) / n;
        if length >= LEAST_SEEN && share >= least && sorted.get(start).is_some_and(kept_whole) {
            runs.push((start, length));
        }
        start = at;
    }
    // The most frequent first; of as frequent, the least value first.
    runs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    runs.truncate(COMMON_VALUES);
    let common = runs
        .iter()
        .filter_map(|&(first, length)| {
            let share = count(u64::try_from(length).unwrap_or(u64::MAX)) / n * present;
            Some((sorted.get(first)?.clone(), share))
        })
        .collect();
    let mut taken = runs;
    taken.sort_unstable();
    (common, without(sorted, &taken))
}

/// `sorted` without the runs `taken`, each a start and a length, in order.
fn without(sorted: Vec<Value>, taken: &[(usize, usize)]) -> Vec<Value> {
    if taken.is_empty() {
        return sorted;
    }
    let mut runs = taken.iter().peekable();
    let mut rest = Vec::with_capacity(sorted.len());
    for (at, value) in sorted.into_iter().enumerate() {
        while runs.next_if(|&&(start, length)| start.saturating_add(length) <= at).is_some() {}
        let inside = runs.peek().is_some_and(|&&(start, _)| start <= at);
        if !inside {
            rest.push(value);
        }
    }
    rest
}

/// Whether the sample keeps `value` whole, so a value equal to it is.
fn kept_whole(value: &Value) -> bool {
    match value {
        Value::Char(s) => s.chars().nth(SAMPLED_CHARACTERS.saturating_sub(1)).is_none(),
        Value::Binary(b) => b.len() < SAMPLED_OCTETS,
        Value::Lob(_) => false,
        Value::Null
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Json(_)
        | Value::Extension(_)
        | Value::Xml(_) => true,
    }
}

/// The share of all rows `common` gives `value`, if it is among them.
pub fn share_of(common: &[(Value, f64)], value: &Value) -> Option<f64> {
    common.iter().find(|(v, _)| compare(v, value) == Some(Ordering::Equal)).map(|&(_, share)| share)
}

/// The share of all rows `common` gives the values below `value`.
pub fn share_below(common: &[(Value, f64)], value: &Value) -> f64 {
    common.iter().filter(|(v, _)| compare(v, value) == Some(Ordering::Less)).map(|&(_, share)| share).sum()
}

/// The share of all rows `common` holds together.
pub fn share_all(common: &[(Value, f64)]) -> f64 {
    common.iter().map(|&(_, share)| share).sum()
}

// Unit tests: a skewed column's common values against their true shares,
// an even column's none, and long values never common (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
