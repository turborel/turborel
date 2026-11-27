//! \[DESIGN\] (RULES.md D1242) A probe of a table built for a pipeline's
//! morsels that pairs some of a batch's rows alone, as a probe in passes
//! does: each pass pairs the rows whose keys fall in the parts it holds
//! and leaves the rest to the pass that holds theirs. Which rows it keeps
//! the caller says of each row's key hash, the hash a hashed table's
//! probe makes, seeded as the table's, never a direct table's number of a
//! key (RULES.md D601), so a row falls in the same pass whatever kind of
//! table the pass's built rows make. A key that matches no row, a null's
//! where a null matches nothing, has no hash, and the caller is told so.
//!
//! The rows it does not keep are dropped once the batch is hashed, before
//! any is located, screened by a Bloom filter or paired, so none is looked
//! up in vain and a LEFT join pairs none of them with nulls. A probe given
//! nothing to keep by hashes its rows once and drops none, at no cost of a
//! row.

use super::SharedTable;
use crate::error::Result;
use crate::exec::table::KeyHashes;
use crate::storage::Batch;

/// Of a probing row, whether a pass pairs it: told its key's seeded hash,
/// or none of a key that matches no row.
pub(in crate::exec) type PassKept<'k> = &'k dyn Fn(Option<u64>) -> bool;

impl SharedTable {
    /// Drops from `given`, the rows of `batch` a probe pairs, and from
    /// `hashes`, their hashes in order, those `kept` does not keep, told
    /// each row's key hash of its columns at `left` as a hashed table's.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value nests deeper than a type may.
    pub(super) fn kept_rows(
        &self,
        (batch, left): (&Batch, &[usize]),
        (given, hashes): (&mut Vec<usize>, &mut KeyHashes),
        kept: PassKept<'_>,
    ) -> Result<()> {
        let keep: Vec<bool> = if self.table.is_direct() {
            let mut seeded = KeyHashes::default();
            seeded.of_batch((batch, Some(given)), (left, &self.nulls), self.hashed())?;
            keeps(&seeded, kept)
        } else {
            keeps(hashes, kept)
        };
        retained(given, &keep);
        retained(&mut hashes.hashes, &keep);
        retained(&mut hashes.misses, &keep);
        Ok(())
    }
}

/// Of each row of `hashes`, whether `kept` keeps it, told its hash, or
/// none of a miss.
fn keeps(hashes: &KeyHashes, kept: PassKept<'_>) -> Vec<bool> {
    hashes.hashes.iter().zip(&hashes.misses).map(|(&h, &miss)| kept((!miss).then_some(h))).collect()
}

/// Keeps of `values` those `keep` marks, in order.
fn retained<V>(values: &mut Vec<V>, keep: &[bool]) {
    let mut marks = keep.iter();
    values.retain(|_| marks.next().copied().unwrap_or(false));
}

// Unit tests: a probe in passes (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::super::SharedTable;
    use super::PassKept;
    use crate::exec::table::{Hashing, Seed, values_hash};
    use crate::physical::JoinKey;
    use crate::storage::{Batch, Column, Ints, Mask};
    use crate::value::{Decimal, Value};

    /// The pairs a probe gives, its left key and built tag shown, and
    /// those a whole batch's match gives, left row and built row.
    type Probed = (Vec<(String, String)>, Vec<(usize, usize)>);

    /// The key of number `k` of a table of keys `stride` apart.
    fn key(k: usize, stride: usize) -> i64 {
        i64::try_from(k.saturating_mul(stride)).unwrap()
    }

    /// The pairs, left key and built tag, of the probe of built rows of
    /// keys k times `stride` tagged k for k below 300, direct if `stride`
    /// is 1, by the left `keys`, of a LEFT join, the rows `kept` keeps
    /// alone; then those `matched_kept` gives of the same.
    fn probed(keys: &[Option<usize>], stride: usize, kept: Option<PassKept<'_>>) -> Probed {
        let built: Vec<i64> = (0..300).map(|k| key(k, stride)).collect();
        let tags: Vec<i64> = (0..300).collect();
        let right = Batch { columns: vec![Column::Ints(Ints::new(built, 0, None)), Column::Ints(Ints::new(tags, 0, None))], rows: 300 };
        let join = [JoinKey { left: 0, right: 0, nulls: false }];
        let table = SharedTable::of(vec![right], (vec![0, 1], 2), &join, (Seed::fixed(0x5EED), true)).unwrap();
        assert_eq!(table.is_direct(), stride == 1);
        let left = || {
            let units: Vec<i64> = keys.iter().map(|k| k.map_or(0, |k| key(k, stride))).collect();
            let nulls: Vec<bool> = keys.iter().map(Option::is_none).collect();
            Batch { columns: vec![Column::Ints(Ints::new(units, 0, Some(Mask::from_bools(&nulls))))], rows: keys.len() }
        };
        let mut pairs = Vec::new();
        for got in SharedTable::probing_kept(&table, (left(), None), (vec![0], None), (true, kept)) {
            let (batch, _) = got.unwrap();
            pairs.extend((0..batch.rows).map(|r| (batch.cell(0, r).to_value().to_string(), batch.cell(2, r).to_value().to_string())));
        }
        (pairs, table.matched_kept((&left(), &[0]), kept).unwrap())
    }

    /// The seeded hash, a hashed table's, of the key of number `k` of a
    /// table of keys `stride` apart.
    fn hash_of(k: usize, stride: usize) -> u64 {
        let seed = Seed::fixed(0x5EED);
        let how = Hashing { seed: &seed, keyed: false, direct: false, kernels: None };
        let value = Value::Exact(Decimal::new(i128::from(key(k, stride)), 0));
        values_hash([&value].into_iter(), &[false], how).unwrap().unwrap()
    }

    /// Verifies: [DESIGN] RULES.md D1242: a probe told which rows to keep
    /// pairs those alone, a row it drops neither paired nor, of a LEFT
    /// join, paired with nulls; it is told each row's seeded hash, a
    /// hashed table's, of a direct table too, and none of a null key's
    /// row, which it keeps or drops as told; told nothing it drops none.
    ///
    /// Decision: a row is kept, or not; its key is null, or not; the
    /// table is hashed, or direct; a probe is given what it keeps, or not.
    #[test]
    fn a_probe_in_passes_pairs_the_rows_kept_alone() {
        let keys: Vec<Option<usize>> = vec![Some(3), None, Some(7), Some(500), Some(42), Some(3), Some(299), Some(11)];
        let null = Value::Null.to_string();
        for stride in [1_000, 1] {
            let each: Vec<Option<u64>> = keys.iter().map(|k| k.map(|k| hash_of(k, stride))).collect();
            let mut sorted: Vec<u64> = each.iter().flatten().copied().collect();
            sorted.sort_unstable();
            // The middle of the six keys that are not null.
            let cut = sorted[3];
            let pass = |h: u64| h < cut;
            for nulls in [false, true] {
                let told = std::cell::RefCell::new(Vec::new());
                let kept = |h: Option<u64>| {
                    told.borrow_mut().push(h);
                    h.map_or(nulls, pass)
                };
                let (pairs, matched) = probed(&keys, stride, Some(&kept));
                let (mut want, mut want_matched) = (Vec::new(), Vec::new());
                for (r, k) in keys.iter().enumerate().filter(|&(r, _)| each[r].map_or(nulls, pass)) {
                    let tag = k.filter(|&k| k < 300);
                    want.push((k.map_or(null.clone(), |k| key(k, stride).to_string()), tag.map_or(null.clone(), |t| t.to_string())));
                    want_matched.extend(tag.map(|t| (r, t)));
                }
                assert!(want.len() > 1 && want.len() < keys.len(), "stride {stride}: some rows kept, some dropped");
                assert_eq!(pairs, want, "stride {stride}, nulls kept {nulls}");
                assert_eq!(matched, want_matched, "stride {stride}, nulls kept {nulls}");
                assert_eq!(told.borrow()[..keys.len()], each[..], "stride {stride}: told the seeded hashes in order");
            }
            let (all, matched) = probed(&keys, stride, None);
            assert_eq!(all.len(), keys.len(), "stride {stride}: told nothing, every row paired once");
            assert_eq!(matched.len(), 6, "stride {stride}: told nothing, every matching row paired");
        }
    }
}
