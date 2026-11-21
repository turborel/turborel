//! \[DESIGN\] (RULES.md D359) EXCEPT and INTERSECT counted in the
//! executor's table of groups (`exec/table.rs`): the second operand's rows
//! each found by its key, hashed by value a vector at a time where read by
//! batch, and counted, the count of a group in a column beside the table;
//! then each of the first operand's rows found there, by the same hash and
//! lanes, no key made into bytes. A key of the first operand not counted
//! is made a group of no count, so a row given once is known again; once
//! the table holds the work area of both, such a key goes to a bag of its
//! own, which spills, by its bytes. Past the work area while counting, the
//! counts and the keys after them go to the bag or the partitions by their
//! bytes, as before (`set.rs`).

use std::sync::Arc;

use super::super::RowIter;
use super::super::table::{GroupTable, Key, hash_rows, hash_values};
use super::super::work::{Bag, Work};
use super::{Each, Kept, Operand};
use crate::db::State;
use crate::error::Result;
use crate::storage::sel::nth;
use crate::storage::{Batch, Row, Sel};

/// The second operand's rows: counted in a table, or, past the work area,
/// each key counted so far and after it, by its bytes.
pub(super) enum Tabled {
    Held(Box<Counts>),
    Past(Each<(Vec<u8>, u64)>),
}

/// Rows counted by their keys in a table of groups: each group's count and
/// whether a row of it was given.
pub(super) struct Counts {
    db: Arc<State>,
    /// None before the first row, whose width it is made of.
    table: Option<GroupTable<()>>,
    tallies: Vec<u64>,
    given: Vec<bool>,
    hashes: Vec<u64>,
    /// The columns of a batch's rows, all of them.
    columns: Vec<usize>,
    /// The bytes it may hold.
    limit: usize,
}

impl Counts {
    /// None counted yet, in `limit` bytes, hashed by `db`'s seed.
    fn new(db: &Arc<State>, limit: usize) -> Counts {
        Counts { db: Arc::clone(db), table: None, tallies: Vec::new(), given: Vec::new(), hashes: Vec::new(), columns: Vec::new(), limit }
    }

    /// Its table, made of keys `width` wide if it has none.
    fn table(&mut self, width: usize) -> &mut GroupTable<()> {
        let seed = self.db.functions.seed();
        self.table.get_or_insert_with(|| GroupTable::of_keys(width, seed))
    }

    /// Whether it holds more than its limit.
    fn full(&self) -> bool {
        let columns = self.tallies.capacity().saturating_mul(size_of::<u64>()).saturating_add(self.given.capacity());
        self.table.as_ref().map_or(0, GroupTable::bytes).saturating_add(columns) > self.limit
    }

    /// The group `g`, its count and flag made if new.
    fn grown(&mut self, g: u32) -> usize {
        let g = crate::storage::mask::index(g);
        if g >= self.tallies.len() {
            self.tallies.push(0);
            self.given.push(false);
        }
        g
    }

    /// Counts `row`, of its first `compared` columns or all: whether it
    /// made a group.
    fn count_row(&mut self, row: &Row, compared: Option<usize>) -> bool {
        let key = compared.and_then(|n| row.get(..n)).unwrap_or(row);
        let hash = hash_values(self.db.functions.seed(), key);
        let before = self.tallies.len();
        let g = self.table(key.len()).found_or_made(hash, Key::Values(key), 0);
        let g = self.grown(g);
        if let Some(tally) = self.tallies.get_mut(g) {
            *tally = tally.saturating_add(1);
        }
        self.tallies.len() > before
    }

    /// Counts the rows `rows`, or all, of `batch`, keyed by every column.
    fn count_batch(&mut self, batch: &Batch, rows: Option<&[u32]>) {
        self.hash_batch(batch, rows);
        let hashes = std::mem::take(&mut self.hashes);
        let columns = std::mem::take(&mut self.columns);
        for (n, &hash) in hashes.iter().enumerate() {
            let g = self.table(columns.len()).found_or_made(hash, Key::Row(batch, &columns, nth(rows, n)), 0);
            let g = self.grown(g);
            if let Some(tally) = self.tallies.get_mut(g) {
                *tally = tally.saturating_add(1);
            }
        }
        (self.hashes, self.columns) = (hashes, columns);
    }

    /// Hashes the rows `rows`, or all, of `batch` by every column.
    fn hash_batch(&mut self, batch: &Batch, rows: Option<&[u32]>) {
        self.columns.clear();
        self.columns.extend(0..batch.columns.len());
        hash_rows(batch, (&self.columns, rows), (self.db.functions.seed(), &*crate::db::kernels::chosen(&self.db)), &mut self.hashes);
    }

    /// Each group's key's bytes and count, then the keys `rest`.
    fn dumped(self, rest: Each<(Vec<u8>, u64)>) -> Each<(Vec<u8>, u64)> {
        let Some(table) = self.table else { return rest };
        let counted = self.tallies.into_iter().enumerate().map(move |(g, n)| {
            let mut key = Vec::new();
            table.key_bytes(g, &mut key);
            Ok((key, n))
        });
        Box::new(counted.collect::<Vec<_>>().into_iter().chain(rest))
    }
}

/// The rows of `second` counted in a table of groups of `db` while it holds
/// no more than `limit` bytes; past that, the keys counted and after.
///
/// # Errors
///
/// As reading the operand fails.
pub(super) fn tabled(second: Operand, db: &Arc<State>, limit: usize) -> Result<Tabled> {
    let mut counts = Counts::new(db, limit);
    match second {
        Operand::Rows(mut rows, compared) => {
            while let Some(row) = rows.next() {
                if counts.count_row(&row?, compared) && counts.full() {
                    return Ok(Tabled::Past(counts.dumped(Operand::Rows(rows, compared).keys())));
                }
            }
        }
        Operand::Batches(mut batches) => {
            while let Some(kept) = batches.next() {
                let (batch, rows) = kept?;
                counts.count_batch(&batch, rows.as_ref().map(Sel::as_slice));
                if counts.full() {
                    return Ok(Tabled::Past(counts.dumped(Operand::Batches(batches).keys())));
                }
            }
        }
    }
    Ok(Tabled::Held(Box::new(counts)))
}

/// The second operand's counts as the first operand's rows are judged, a
/// key not counted and past the table kept in `bag` once given.
struct Judged {
    counts: Counts,
    how: Kept,
    bag: Bag,
}

impl Judged {
    /// Whether the set operation keeps a row of `key`, of hash `hash`, as
    /// `set::keeps` judges it.
    ///
    /// # Errors
    ///
    /// As the bag fails.
    fn keeps(&mut self, (hash, key): (u64, Key<'_>), width: usize) -> Result<bool> {
        let found = self.counts.table(width).find(hash, key).map(crate::storage::mask::index);
        let held = found.and_then(|g| self.counts.tallies.get_mut(g));
        if self.how.all {
            // Each duplicate in the second operand matches one in the first.
            let matched = held.is_some_and(|n| {
                let had = *n > 0;
                *n = n.saturating_sub(1);
                had
            });
            return Ok(matched == self.how.intersect);
        }
        if held.is_some_and(|n| *n > 0) != self.how.intersect {
            return Ok(false);
        }
        let g = if let Some(g) = found {
            g
        } else {
            if self.counts.full() {
                let mut bytes = Vec::new();
                key.put(width, &mut bytes);
                return self.bag.add_new(&bytes);
            }
            let made = self.counts.table(width).make(hash, key, 0);
            self.counts.grown(made)
        };
        Ok(self.counts.given.get_mut(g).is_some_and(|given| !std::mem::replace(given, true)))
    }

    /// The rows of the batch `kept` the set operation keeps, in order, then
    /// the error of the first it cannot judge, if any; or its error.
    fn batch(&mut self, kept: Result<(Batch, Option<Sel>)>) -> Vec<Result<Row>> {
        let (batch, rows) = match kept {
            Ok(kept) => kept,
            Err(e) => return vec![Err(e)],
        };
        let rows = rows.as_ref().map(Sel::as_slice);
        self.counts.hash_batch(&batch, rows);
        let (hashes, columns) = (std::mem::take(&mut self.counts.hashes), std::mem::take(&mut self.counts.columns));
        let mut out = Vec::new();
        for (n, &hash) in hashes.iter().enumerate() {
            let r = nth(rows, n);
            match self.keeps((hash, Key::Row(&batch, &columns, r)), columns.len()) {
                Ok(true) => out.push(Ok(columns.iter().map(|&c| batch.cell(c, r).to_value()).collect())),
                Ok(false) => {}
                Err(e) => {
                    out.push(Err(e));
                    break;
                }
            }
        }
        (self.counts.hashes, self.counts.columns) = (hashes, columns);
        out
    }

    /// `r`, if the set operation keeps it, of its first `compared` columns.
    fn row(&mut self, r: Result<Row>, compared: Option<usize>) -> Option<Result<Row>> {
        let row = match r {
            Ok(row) => row,
            Err(e) => return Some(Err(e)),
        };
        let key = compared.and_then(|n| row.get(..n)).unwrap_or(&row);
        let hash = hash_values(self.counts.db.functions.seed(), key);
        self.keeps((hash, Key::Values(key)), key.len()).map(|kept| kept.then_some(row)).transpose()
    }
}

/// The rows of `first` the set operation `how` keeps, of the second
/// operand's `counts`, a key not counted kept, past the table, in a bag of
/// `given` once given: of a batch, a row made only of one kept.
pub(super) fn keeping(first: Operand, mut counts: Box<Counts>, how: Kept, given: Work) -> RowIter {
    counts.limit = counts.limit.saturating_add(given.limit());
    let mut judged = Judged { counts: *counts, how, bag: Bag::new(given) };
    match first {
        Operand::Rows(rows, compared) => Box::new(rows.filter_map(move |r| judged.row(r, compared))),
        Operand::Batches(batches) => Box::new(batches.flat_map(move |b| judged.batch(b))),
    }
}
