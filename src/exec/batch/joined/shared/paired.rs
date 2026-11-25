//! \[DESIGN\] (RULES.md D1083) A probing batch paired with a shared table
//! on a key of one column of integers, of one scale in the batch and in
//! the built operand, a null matching nothing: the usual key of a join,
//! a number of a row. Such a key is paired in its vectors' units, one
//! loop of one kind of vector. A probing row's units are compared with a
//! built row's as they stand, equal units of one scale being equal
//! numbers, with no vector's kind asked again for each pair. Of a direct
//! table of whole numbers (RULES.md D601) a row finds its key's rows by
//! its units themselves, the key's place in the span, with no number
//! made of them and nothing compared, as every row at a place holds that
//! key.
//!
//! Any other key, of several columns, of texts, of scales apart, or one
//! whose nulls match, is paired as before (`shared.rs`), hashed a column
//! at a time and compared a pair at a time (`table/join/equal.rs`). The
//! pairs are the same, in the same order, of either.

use super::{SharedTable, pick};
use crate::error::Result;
use crate::exec::table::KeyHashes;
use crate::storage::{Batch, Column, Ints};

impl SharedTable {
    /// The vectors of the key of `batch`, of its columns at `left`, and of
    /// the built operand's, if the key is one column of integers of one
    /// scale in both and its nulls match nothing.
    pub(super) fn unit_keys<'k>(&'k self, batch: &'k Batch, left: &[usize]) -> Option<(&'k Ints, &'k Ints)> {
        let ([l], [at], [false]) = (left, self.places.as_slice(), self.nulls.as_slice()) else { return None };
        let (Some(Column::Ints(probe)), Some(Column::Ints(built))) = (batch.columns.get(*l), self.right.columns.get(*at)) else {
            return None;
        };
        (probe.scale == built.scale).then_some((probe, built))
    }

    /// Gives `found` each pair of a row of `batch` at `given`, its key the
    /// vector `probe` at `left`, and a built row whose key, of `built`,
    /// has its units, in order: as picks, the batch's row first.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value nests deeper than a type may.
    pub(super) fn paired_units(
        &self,
        (probe, built): (&Ints, &Ints),
        (batch, given, left): (&Batch, &[usize], &[usize]),
        mut found: impl FnMut(u32, u32),
    ) -> Result<()> {
        if self.table.is_direct() && probe.scale == 0 {
            for &r in given {
                let Some(&key) = probe.units.get(r).filter(|_| !probe.is_null(r)) else { continue };
                for &p in self.table.rows_of(key) {
                    found(pick(r), p);
                }
            }
            return Ok(());
        }
        let mut hashes = KeyHashes::default();
        hashes.of_batch((batch, Some(given)), (left, &self.nulls), self.hashing())?;
        for ((&r, &h), _) in given.iter().zip(&hashes.hashes).zip(&hashes.misses).filter(|(_, miss)| !**miss) {
            let Some(key) = probe.units.get(r) else { continue };
            for p in self.table.candidates(h) {
                if built.units.get(p) == Some(key) {
                    found(pick(r), pick(p));
                }
            }
        }
        Ok(())
    }
}

// Unit tests: a key of integers paired in its units against the pairs
// compared as values (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::SharedTable;
    use crate::exec::table::Seed;
    use crate::physical::JoinKey;
    use crate::storage::{BATCH_ROWS, Batch, Column, Ints, Mask};

    /// A batch of a key of `units` at `scale`, null where `nulls` says,
    /// and each row's number.
    fn batch(units: Vec<i64>, scale: u32, nulls: Option<Vec<bool>>) -> Batch {
        let rows = units.len();
        let numbers = (0_i64..).take(rows).collect();
        let key = Ints::new(units, scale, nulls.map(|n| Mask::from_bools(&n)));
        Batch { columns: vec![Column::Ints(key), Column::Ints(Ints::new(numbers, 0, None))], rows }
    }

    /// The table of `built`, joined on its key, a null matching a null if
    /// `nulls`.
    fn table(built: Batch, nulls: bool) -> SharedTable {
        let keys = [JoinKey { left: 0, right: 0, nulls }];
        SharedTable::of(vec![built], (vec![0, 1], 2), &keys, (Seed::fixed(0x5EED), true)).unwrap()
    }

    /// The rows `probe` gives probing `table` by batch: of each, its
    /// values as text.
    fn probed(table: &SharedTable, probe: &Batch) -> Vec<String> {
        let mut rows = Vec::new();
        for kept in table.probed((probe, None), (&[0], None)) {
            let (joined, _) = kept.unwrap();
            assert!(joined.rows <= BATCH_ROWS);
            rows.extend((0..joined.rows).map(|r| format!("{:?}", (0..4).map(|c| joined.cell(c, r).to_value()).collect::<Vec<_>>())));
        }
        rows
    }

    /// The rows `probe` gives probing `table`, each pair compared as
    /// values (`matched`).
    fn expected(table: &SharedTable, probe: &Batch) -> Vec<String> {
        let pairs = table.matched(probe, &[0]).unwrap();
        let row = |(r, p): (usize, usize)| {
            let mut values: Vec<_> = (0..2).map(|c| probe.cell(c, r).to_value()).collect();
            values.extend(table.row_of(p));
            format!("{values:?}")
        };
        pairs.into_iter().map(row).collect()
    }

    /// Verifies: [DESIGN] RULES.md D1083: a key of integers paired in its
    /// units gives the rows a key compared as values gives, in order: of a
    /// direct table by the units themselves, a null probing row and a key
    /// out of the span pairing with none, a key of many rows with each,
    /// more pairs than a batch holds given in batches; of a hashed table
    /// by its hashes and units; of a decimal key of one scale, not whole.
    #[test]
    fn a_key_of_integers_paired_in_its_units_pairs_as_values_do() {
        let dense: Vec<i64> = (0_i64..3_000).map(|r| r % 700).collect();
        let sparse: Vec<i64> = (0_i64..3_000).map(|r| (r % 700) * 1_000).collect();
        let cents: Vec<i64> = (0_i64..3_000).map(|r| (r % 700) * 1_001).collect();
        let nulls: Vec<bool> = (0_usize..1_000).map(|r| r % 9 == 4).collect();
        for (built, scale, direct) in [(dense, 0, true), (sparse, 0, false), (cents, 2, false)] {
            let probing: Vec<i64> = built.iter().step_by(3).enumerate().map(|(n, k)| k.saturating_add(i64::from(n % 5 == 0))).collect();
            let held = table(batch(built, scale, None), false);
            assert_eq!(held.is_direct(), direct);
            let probe = batch(probing, scale, Some(nulls.clone()));
            assert!(held.unit_keys(&probe, &[0]).is_some());
            let want = expected(&held, &probe);
            assert!(want.len() > BATCH_ROWS, "more pairs than a batch: {}", want.len());
            assert_eq!(probed(&held, &probe), want, "scale {scale}, direct {direct}");
        }
    }

    /// Verifies: [DESIGN] RULES.md D1083 (MC/DC): a key is paired in its
    /// units only if it is one column of integers of one scale on both
    /// sides whose nulls match nothing. Each alone sends it to the pairing
    /// by values, which pairs 1.00 with 1 and a null with a null: scales
    /// apart, and nulls that match.
    #[test]
    fn keys_of_scales_apart_or_matching_nulls_pair_as_values() {
        let built = || batch(vec![1, 2, 0, 3, 0], 0, Some(vec![false, false, true, false, false]));
        let apart = batch(vec![100, 250, 300], 2, None);
        let held = table(built(), false);
        assert!(held.unit_keys(&apart, &[0]).is_none(), "scales apart");
        assert_eq!(probed(&held, &apart), expected(&held, &apart));
        assert_eq!(probed(&held, &apart).len(), 2, "1.00 and 3.00");
        let null = batch(vec![0, 2], 0, Some(vec![true, false]));
        let matching = table(built(), true);
        assert!(matching.unit_keys(&null, &[0]).is_none(), "nulls match");
        assert_eq!(probed(&matching, &null), expected(&matching, &null));
        assert_eq!(probed(&matching, &null).len(), 2, "the null and 2");
        assert_eq!(probed(&held, &null).len(), 1, "a null probing row pairs with none");
    }
}
