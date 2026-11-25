//! \[DESIGN\] (RULES.md D1151) A probe of a table built for a pipeline's
//! morsels that makes its pairs a batch at a time, each as it is asked
//! for. A row of a left batch matches every built row of its key, so the
//! pairs of one batch are as many as its rows times their keys' rows:
//! made all at once, as they were, a morsel whose keys each match
//! thousands of built rows held millions of pairs before the first was
//! read, outside any grant. A probe made a batch at a time holds one batch
//! of pairs and where it stopped, the row of the left batch it pairs and
//! how many of its key's built rows it looked at, so what reads it holds
//! what it keeps and the grant it is charged to sees it grow
//! (`exec/parallel/charged.rs`). The pairs come in the order they did,
//! the rows paired before an error first, then the error.

use std::ops::Deref;

use super::super::super::stream::Kept;
use super::{Pairs, SharedTable, pick};
use crate::error::SqlError;
use crate::exec::table::{KeyHashes, keys_equal};
use crate::storage::sel::{kept_count, nth};
use crate::storage::{BATCH_ROWS, Batch, NO_ROW, Sel};

/// The batches of the pairs of a left batch's rows and the built rows of
/// `T`, a table, their keys match, made a batch at a time.
pub(in crate::exec) struct Probing<T> {
    table: T,
    batch: Batch,
    /// The places in `batch` of the rows it pairs.
    given: Vec<usize>,
    hashes: KeyHashes,
    /// Of a table past the caches, the places in `given` of the rows whose
    /// keys its Bloom filter may hold, in order, and how many of them lie
    /// before the row it pairs now (RULES.md D1087); none if it looks up
    /// every row.
    screened: Option<(Vec<u32>, usize)>,
    /// The places of the left's key vectors, and of the vectors its
    /// batches hold, or all.
    left: Vec<usize>,
    left_shown: Option<Vec<usize>>,
    /// Of a LEFT join, a row no built row matches paired with none.
    outer: bool,
    /// The place in `given` of the row it pairs now, how many of its
    /// key's built rows it looked at, and whether one matched.
    at: usize,
    seen: usize,
    any: bool,
    /// The pairs of the batch it makes, empty between batches.
    pairs: Pairs,
    failed: Option<SqlError>,
    ended: bool,
}

impl SharedTable {
    /// The probe of `table` by the rows of `batch` at `rows`, or all, its
    /// key columns at `left`, its batches holding the left's vectors at
    /// `left_shown`, or all; of a LEFT join if `outer`, each row no built
    /// row matches paired with none [ISO/IEC 9075:1992 §7.5 GR5b].
    pub(in crate::exec) fn probing<T: Deref<Target = SharedTable>>(
        table: T,
        (batch, rows): (Batch, Option<Sel>),
        (left, left_shown): (Vec<usize>, Option<Vec<usize>>),
        outer: bool,
    ) -> Probing<T> {
        let rows = rows.as_deref();
        let given: Vec<usize> = (0..kept_count(rows, batch.rows)).map(|n| nth(rows, n)).collect();
        let mut hashes = KeyHashes::default();
        let failed = hashes.of_batch((&batch, Some(&given)), (&left, &table.nulls), table.hashing()).err();
        let screened = failed.is_none().then(|| table.screened(&hashes.hashes)).flatten().map(|held| (held, 0));
        let pairs = (Vec::with_capacity(BATCH_ROWS), Vec::with_capacity(BATCH_ROWS));
        Probing { table, batch, given, hashes, screened, left, left_shown, outer, at: 0, seen: 0, any: false, pairs, failed, ended: false }
    }
}

impl<T: Deref<Target = SharedTable>> Probing<T> {
    /// Pairs the rows from where it stopped with the built rows their
    /// keys match into `pairs`, until they are `BATCH_ROWS` or every row
    /// is paired.
    fn pair(&mut self, pairs: &mut Pairs) -> Result<(), SqlError> {
        while let Some(&r) = self.given.get(self.at) {
            if pairs.0.len() >= BATCH_ROWS || !self.row_paired(r, pairs)? {
                return Ok(());
            }
            if self.outer && !self.any {
                // No built row: its right columns null (RULES.md D1083).
                pairs.0.push(pick(r));
                pairs.1.push(NO_ROW);
            }
            (self.at, self.seen, self.any) = (self.at.saturating_add(1), 0, false);
        }
        self.ended = true;
        Ok(())
    }

    /// Pairs left row `r`, the one it pairs now, with the built rows its
    /// key matches, from the first it has not looked at, into `pairs`:
    /// whether it looked at them all, not stopped as `pairs` filled. A key
    /// of one column of integers is compared in its units, and of a
    /// direct table found by them (RULES.md D1083).
    fn row_paired(&mut self, r: usize, pairs: &mut Pairs) -> Result<bool, SqlError> {
        let table = &*self.table;
        let units = table.unit_keys(&self.batch, &self.left);
        if let Some((probe, _)) = units.filter(|(probe, _)| table.table.is_direct() && probe.scale == 0) {
            let Some(&key) = probe.units.get(r).filter(|_| !probe.is_null(r)) else { return Ok(true) };
            for &p in table.table.rows_of(key).get(self.seen..).unwrap_or_default() {
                self.seen = self.seen.saturating_add(1);
                if !matched(&mut self.any, (r, p), pairs) {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        let (Some(&h), Some(false)) = (self.hashes.hashes.get(self.at), self.hashes.misses.get(self.at).copied()) else { return Ok(true) };
        if !may_match(&mut self.screened, self.at) {
            return Ok(true);
        }
        let mut candidates = table.table.candidates(h);
        // The built rows looked at before it stopped are passed by.
        if let Some(before) = self.seen.checked_sub(1)
            && candidates.nth(before).is_none()
        {
            return Ok(true);
        }
        for p in candidates {
            self.seen = self.seen.saturating_add(1);
            let equal = match units {
                Some((probe, built)) => probe.units.get(r).is_some_and(|k| built.units.get(p) == Some(k)),
                None => keys_equal((&self.batch, &self.left, r), (&table.right, &table.places, p), &table.nulls)?,
            };
            if equal && !matched(&mut self.any, (r, pick(p)), pairs) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Whether the row at place `at` of a probe may match a built row: of a
/// probe `screened`, whether its key's Bloom filter may hold it, so a row
/// it cannot reads nothing of the table (RULES.md D1087).
fn may_match(screened: &mut Option<(Vec<u32>, usize)>, at: usize) -> bool {
    let Some((held, before)) = screened.as_mut() else { return true };
    while held.get(*before).is_some_and(|&n| usize::try_from(n).is_ok_and(|n| n < at)) {
        *before = before.saturating_add(1);
    }
    held.get(*before).is_some_and(|&n| usize::try_from(n).is_ok_and(|n| n == at))
}

/// Adds the pair of left row `r` and built row `p` to `pairs`, the row
/// then matched (`any`): whether they have room for more.
fn matched(any: &mut bool, (r, p): (usize, u32), pairs: &mut Pairs) -> bool {
    *any = true;
    pairs.0.push(pick(r));
    pairs.1.push(p);
    pairs.0.len() < BATCH_ROWS
}

impl<T: Deref<Target = SharedTable>> Iterator for Probing<T> {
    type Item = Result<Kept, SqlError>;

    fn next(&mut self) -> Option<Result<Kept, SqlError>> {
        loop {
            if let Some(e) = self.failed.take() {
                self.ended = true;
                return Some(Err(e));
            }
            if self.ended {
                return None;
            }
            let mut pairs = std::mem::take(&mut self.pairs);
            if let Err(e) = self.pair(&mut pairs) {
                self.failed = Some(e);
            }
            let made = (!pairs.0.is_empty()).then(|| self.table.gathered((&self.batch, self.left_shown.as_deref()), &mut pairs));
            // Emptied by `gathered`, the lists keep their room for the next.
            self.pairs = pairs;
            if let Some(kept) = made {
                return Some(Ok(kept));
            }
        }
    }
}

// Unit tests: a probe made a batch at a time (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::super::SharedTable;
    use crate::exec::table::Seed;
    use crate::physical::JoinKey;
    use crate::storage::{BATCH_ROWS, Batch, Row};
    use crate::value::{Decimal, Value};

    /// A whole number, as a value.
    fn number(n: usize) -> Value {
        Value::Exact(Decimal::new(i128::try_from(n).unwrap(), 0))
    }

    /// The pairs, left key and built tag, the probe of the built rows
    /// `built`, (key, tag), by left keys `keys` gives, of a LEFT join if
    /// `outer`, and the most rows one of its batches held.
    fn pairs_of(built: &[(usize, usize)], keys: &[usize], (outer, dense): (bool, bool)) -> (Vec<(String, String)>, usize) {
        let rows: Vec<Row> = built.iter().map(|&(k, t)| vec![number(k), number(t)]).collect();
        let join = [JoinKey { left: 0, right: 0, nulls: false }];
        let table = SharedTable::of_rows(&rows, 2, &join, (Seed::fixed(0x5EED), vec![1], dense)).unwrap();
        let mut left = Batch::empty(1);
        for &k in keys {
            left.push(&vec![number(k)], &[0]);
        }
        let (mut pairs, mut most) = (Vec::new(), 0);
        for kept in SharedTable::probing(&table, (left, None), (vec![0], None), outer) {
            let (batch, _) = kept.unwrap();
            most = most.max(batch.rows);
            pairs.extend((0..batch.rows).map(|r| (batch.cell(0, r).to_value().to_string(), batch.cell(1, r).to_value().to_string())));
        }
        (pairs, most)
    }

    /// Verifies: [DESIGN] RULES.md D1151: a probe whose left rows each match
    /// thousands of built rows gives their pairs in batches of at most
    /// `BATCH_ROWS`, a batch made as it is asked for, a key's rows taken up
    /// where the batch before stopped, so the pairs come in the order the
    /// left rows and the built rows give them, every one once; of a LEFT
    /// join, a row no built row matches paired with nulls in its place;
    /// of a direct table too (D601).
    ///
    /// Decision: the left rows' matches pass a batch's end, or do not;
    /// a left row matches, or not; the table is hashed, or direct.
    #[test]
    fn a_probe_gives_its_pairs_a_batch_at_a_time() {
        let built: Vec<(usize, usize)> = (0..3_000).map(|t| (t % 3, t)).collect();
        let keys = [1, 7, 2, 1];
        let null = Value::Null.to_string();
        let want = |outer: bool| -> Vec<(String, String)> {
            let mut want = Vec::new();
            for &k in &keys {
                let matched: Vec<(String, String)> = built.iter().filter(|b| b.0 == k).map(|b| (k.to_string(), b.1.to_string())).collect();
                if matched.is_empty() && outer {
                    want.push((k.to_string(), null.clone()));
                }
                want.extend(matched);
            }
            want
        };
        for dense in [false, true] {
            for outer in [false, true] {
                let (pairs, most) = pairs_of(&built, &keys, (outer, dense));
                assert_eq!(pairs, want(outer), "outer {outer}, dense {dense}");
                assert!(most <= BATCH_ROWS && most > 0, "a batch of {most} rows");
            }
        }
        let (few, _) = pairs_of(&[(4, 0), (4, 1)], &[4, 5], (true, false));
        let pair = |k: &str, t: &str| (k.to_owned(), t.to_owned());
        assert_eq!(few, vec![pair("4", "0"), pair("4", "1"), pair("5", &null)]);
    }
}
