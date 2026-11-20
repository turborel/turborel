//! \[DESIGN\] Groups estimated (RULES.md D370): of grouping columns
//! c1..ck over n input rows, the distinct combinations D are the product
//! of each column's distinct values, at most the rows of their table if
//! one table has them all, or, if an index's key is exactly those columns,
//! the distinct keys its sketch counts, which is that set's distinct
//! count, correlation and all. The groups n rows fall into are then
//! D(1 - (1 - 1/D)^n), Cardenas' formula ("Analysis and Performance of
//! Inverted Data Base Structures", 1975): the distinct values n draws
//! from D equally likely ones meet, at most n and, of one row, one.

use super::lineage::{Over, one_table};
use super::{Card, Estimates};
use crate::catalog::QualifiedName;
use crate::logical::LogicalPlan;
use crate::storage::cost_of;

/// The distinct values `n` rows drawn from `distinct` equally likely
/// ones are expected to meet: none of no rows, one of one, at most `n`.
pub(in crate::physical) fn cardenas(distinct: f64, n: f64) -> f64 {
    if n.is_nan() || n < 1.0_f64 {
        return 0.0_f64;
    }
    let d = distinct.max(1.0_f64);
    // (1 - 1/d)^n, by its logarithm, exact for a large d.
    let missed = ((-1.0_f64 / d).ln_1p() * n).exp();
    (d * (1.0_f64 - missed)).clamp(1.0_f64, n)
}

impl Estimates<'_> {
    /// The card of a grouping by `groups` of `input`, with `seeds` groups
    /// made before any row (the empty grouping sets'): one row of no
    /// grouping column, even of no input row.
    pub(super) fn grouped(&self, groups: &[usize], seeds: usize, input: &LogicalPlan, depth: usize) -> Card {
        let seeded = cost_of(seeds);
        if groups.is_empty() {
            return Card::exact(seeded.max(1.0_f64));
        }
        let n = self.card_at(input, depth);
        let rows = cardenas(self.combinations(input, groups), n.rows) + seeded;
        Card { rows, upper: n.upper + seeded }
    }

    /// The distinct combinations of `input`'s columns `columns`.
    pub(super) fn combinations(&self, input: &LogicalPlan, columns: &[usize]) -> f64 {
        let product = columns.iter().map(|&c| self.distinct(input, c).max(1.0_f64)).product::<f64>();
        let Some((table, places)) = one_table(Over::One(input), columns) else { return product };
        let mut keys: Vec<usize> = places.iter().map(|&(_, place)| place).collect();
        keys.sort_unstable();
        keys.dedup();
        let rows = self.table_rows(table).unwrap_or(product);
        self.sketched(table, &keys).unwrap_or_else(|| product.min(rows.max(1.0_f64)))
    }

    /// The distinct keys an index of `table` whose key is exactly the
    /// columns `keys`, sorted, counts, if one does and knows.
    fn sketched(&self, table: &QualifiedName, keys: &[usize]) -> Option<f64> {
        let db = self.db();
        let exact = |columns: &[usize]| {
            let mut sorted = columns.to_vec();
            sorted.sort_unstable();
            sorted == keys
        };
        let indexes = db.catalog.indexes(table);
        let ix = indexes.iter().find(|ix| !ix.is_collated() && !db.building.contains(&ix.name) && exact(&ix.columns))?;
        let found = std::sync::Arc::clone(db.index_of(&ix.name).ok()?);
        db.distinct_keys(table, &*found)
    }
}
