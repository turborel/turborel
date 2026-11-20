//! \[DESIGN\] Equalities of several columns with constants, too rare for
//! the sample to count, estimated by their columns' joint distinct values
//! in the sample (RULES.md D382). A sample of n rows counts a share only
//! down to about 8/n, yet it holds the combinations of the columns'
//! values that occur, each column's correlation with the others
//! included. Their distinct combinations, scaled to the table's rows as a
//! column's distinct values are (`stats::scaled_distinct`), give a
//! combination the sample holds its average share, where the product of
//! each column's share takes every pair as possible. A combination the
//! sample does not hold shares what the sample did not see: by Good and
//! Turing, the share of rows of combinations seen once, among those
//! combinations never seen. Of columns whose every combination the sample
//! saw more than once, that is none, and a conjunction of values that
//! never go together is estimated to keep no row.

use std::cmp::Ordering;
use std::collections::HashMap;

use super::sampled::{as_given, key_of, remembered_as};
use crate::stats::{RowSample, scaled_distinct};
use crate::storage::cost_of;
use crate::value::{NormKey, Value, compare};

/// The share of a table of `rows` rows whose columns `columns` hold the
/// values `values`, by the joint distinct values of those columns in
/// `sample`; none if the sample holds no row of them all present.
pub(super) fn share(columns: &[usize], values: &[&Value], sample: &RowSample, rows: f64) -> Option<f64> {
    let key = key_of("joint", &(columns, values, rows.to_bits()));
    as_given(sample.remembered(key, || remembered_as(found(columns, values, sample, rows))))
}

/// `share`, found from `sample`'s rows.
fn found(columns: &[usize], values: &[&Value], sample: &RowSample, rows: f64) -> Option<f64> {
    let mut held: HashMap<Vec<NormKey>, u32> = HashMap::new();
    let (mut present, mut seen) = (0_usize, false);
    for row in sample.rows() {
        let Some(tuple) = columns.iter().map(|&c| row.get(c).filter(|v| !v.is_null())).collect::<Option<Vec<&Value>>>() else { continue };
        present = present.saturating_add(1);
        seen = seen || tuple.iter().zip(values).all(|(v, c)| compare(v, c) == Some(Ordering::Equal));
        let times = held.entry(tuple.iter().map(|v| v.norm()).collect()).or_insert(0);
        *times = times.saturating_add(1);
    }
    if present == 0 {
        return None;
    }
    let (n, present) = (cost_of(sample.rows().len()).max(1.0_f64), cost_of(present));
    let distinct = cost_of(held.len());
    let once = cost_of(held.values().filter(|&&t| t == 1).count());
    let scaled = scaled_distinct((distinct, once), present, rows * present / n).max(distinct);
    if seen {
        return Some(present / n / scaled.max(1.0_f64));
    }
    // What the sample did not see, among the combinations it did not.
    let unseen = scaled - distinct;
    Some(if unseen < 1.0_f64 { 0.0_f64 } else { once / n / unseen })
}
