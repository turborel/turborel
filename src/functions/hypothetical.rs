//! The hypothetical set functions [ISO/IEC 9075-2:2023 §10.9]: RANK,
//! DENSE_RANK, PERCENT_RANK and CUME_DIST of a hypothetical row, the value
//! the window function of that name would give it, were it added to its
//! group's rows and ordered among them by the `<within group
//! specification>` \[GR9\].
//!
//! Its argument is the row of the hypothetical values, then the sort keys,
//! each pair cast to the type the two have in common and, where a
//! collation orders them, made its sort keys, so they compare as GR9's
//! window ordering of their UNION ALL would. A null is placed as its key's
//! null ordering says, and never eliminated. Of the group's rows it counts
//! those before the hypothetical row, its peers and all, and holds nothing
//! else. DENSE_RANK is fed each distinct row once, as a DISTINCT set
//! function is (`exec/group.rs`), so those it counts before are distinct.

use std::cmp::Ordering;
use std::sync::Arc;

use super::{Accumulator, AggregateFunction};
use crate::error::{Code, Result, SqlError};
use crate::value::{DataType, Decimal, SortOrder, Value};

#[cfg(test)]
mod tests;

/// [ISO/IEC 9075-2:2023 §10.9] Which `<rank function type>` a
/// hypothetical set function is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hypothesis {
    /// RANK, one more than the rows before.
    Rank,
    /// DENSE_RANK, one more than the distinct rows before.
    DenseRank,
    /// PERCENT_RANK, the rows before over the others.
    PercentRank,
    /// CUME_DIST, the rows not after over all.
    CumeDist,
}

/// [ISO/IEC 9075-2:2023 §10.9] The `<rank function type>`s, by name, no
/// key words of SQL-92.
pub const RANK_FUNCTION_TYPES: [(&str, Hypothesis); 4] = [
    ("RANK", Hypothesis::Rank),
    ("DENSE_RANK", Hypothesis::DenseRank),
    ("PERCENT_RANK", Hypothesis::PercentRank),
    ("CUME_DIST", Hypothesis::CumeDist),
];

impl Hypothesis {
    /// The function named `name`, if one is.
    pub fn named(name: &str) -> Option<Hypothesis> {
        RANK_FUNCTION_TYPES.into_iter().find(|(n, _)| *n == name).map(|(_, h)| h)
    }

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Hypothesis::Rank => "RANK",
            Hypothesis::DenseRank => "DENSE_RANK",
            Hypothesis::PercentRank => "PERCENT_RANK",
            Hypothesis::CumeDist => "CUME_DIST",
        }
    }
}

/// [ISO/IEC 9075-2:2023 §10.9 SR10, GR9] A call of a hypothetical set
/// function: which, of what result type \[SR10d\], ordered by keys of
/// what directions and null orderings.
pub struct Hypothetical {
    /// The function.
    pub hypothesis: Hypothesis,
    /// The type of its result.
    pub result: DataType,
    /// Per sort key, its direction and null ordering.
    pub orders: Arc<[SortOrder]>,
}

impl AggregateFunction for Hypothetical {
    fn name(&self) -> &str {
        self.hypothesis.name()
    }
    fn result_type(&self, _arg: Option<DataType>) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(Ranked { hypothesis: self.hypothesis, orders: Arc::clone(&self.orders), row: None, before: 0, peers: 0, rows: 0 })
    }
}

/// The rows of a group counted against its hypothetical row.
struct Ranked {
    hypothesis: Hypothesis,
    orders: Arc<[SortOrder]>,
    /// The hypothetical row's values, of the first row fed: alike of every
    /// row of the group, as of its grouping columns \[SR14\], and as its
    /// sort keys where a collation would tell them apart.
    row: Option<Vec<Value>>,
    /// The rows before it.
    before: u64,
    /// The rows that are its peers.
    peers: u64,
    /// The rows.
    rows: u64,
}

impl Accumulator for Ranked {
    fn update(&mut self, v: &Value) -> Result<()> {
        let Value::Row(fields) = v else { return Err(not_bound()) };
        let k = self.orders.len();
        let (values, keys) = fields.split_at_checked(k).filter(|(_, keys)| keys.len() == k).ok_or_else(not_bound)?;
        let row = self.row.get_or_insert_with(|| values.to_vec());
        match placed(&self.orders, keys, row) {
            Ordering::Less => self.before = self.before.saturating_add(1),
            Ordering::Equal => self.peers = self.peers.saturating_add(1),
            Ordering::Greater => {}
        }
        self.rows = self.rows.saturating_add(1);
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §10.9 GR9, §9.23 SR5e to SR5h] The window
    /// function's value for the hypothetical row, of a partition of the
    /// group's rows and it: RANK and DENSE_RANK one more than the rows (or
    /// the distinct rows) before it; PERCENT_RANK those over the partition's
    /// other rows, 0 of none; CUME_DIST the rows not after it, itself among
    /// them, over the partition's. Of no rows, 1, 1, 0 and 1.
    fn finish(&self) -> Result<Value> {
        let rank = self.before.saturating_add(1);
        Ok(match self.hypothesis {
            Hypothesis::Rank | Hypothesis::DenseRank => Value::Exact(Decimal::new(i128::from(rank), 0)),
            Hypothesis::PercentRank => Value::Approx(if self.rows == 0 { 0.0_f64 } else { real(self.before) / real(self.rows) }),
            Hypothesis::CumeDist => Value::Approx(real(rank.saturating_add(self.peers)) / real(self.rows.saturating_add(1))),
        })
    }
}

/// Where `keys`, a row's sort keys, are against `row`, the hypothetical
/// row's values: by the first key they differ in, as its direction and
/// null ordering in `orders` place them [ISO/IEC 9075-2:2023 §10.10 GR2].
fn placed(orders: &[SortOrder], keys: &[Value], row: &[Value]) -> Ordering {
    let each = orders.iter().zip(keys.iter().zip(row));
    each.map(|(order, (key, value))| order.cmp(key, value)).find(|o| o.is_ne()).unwrap_or(Ordering::Equal)
}

/// `n` as an approximate number.
fn real(n: u64) -> f64 {
    Decimal::new(i128::from(n), 0).to_f64()
}

fn not_bound() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a hypothetical set function was bound without its row of values and sort keys")
}
