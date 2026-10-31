//! The FROM product [X3.135-1986 §5.20 GR2b], the hash join, which gives
//! the rows of a product that equalities of its columns keep
//! (`physical/join.rs`), and the outer join [ISO/IEC 9075:1992 §7.5]. A
//! hash join whose right operand outgrows its work area is partitioned
//! (`exec/join/partitioned.rs`, RULES.md D38); a product's right operand is
//! read again for each left row, from a run of the spill area once it
//! outgrows the work area (`exec/join/rescan.rs`); and an outer join's is
//! either (`exec/join/outer.rs`); an index nested-loop join holds nothing,
//! each left row probing an index (`exec/join/indexed.rs`). Split from
//! exec.rs (RULES.md R-SIZE-2).

mod indexed;
mod outer;
mod partitioned;
mod rescan;

pub(super) use indexed::indexed;
pub(super) use outer::outer;
pub(in crate::exec) use partitioned::joined as partitioned_joined;
pub use partitioned::{DEFAULT_PARTITION_DEPTH, DEFAULT_PARTITIONS};
pub(super) use rescan::Rescanned;

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use super::work::Work;
use super::{RowIter, broken, column, execute, failed};
use crate::db::State;
use crate::error::Result;
use crate::physical::Plan;
use crate::spill::{Records, SpillTable, bucket_key, record_row, row_bytes, row_record};
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::types::TypeRegistry;
use crate::value::{Bucket, bucket, compare};

/// The rows of `left` joined with those of `right`: on `keys` if given
/// (`HashJoin`), else with every one (`product`). The right operand is
/// read first, and the left only if it succeeded. Never inlined into
/// `execute`, whose frame every level of a plan adds to the stack (RULES.md
/// R-CF-6).
#[inline(never)]
pub(super) fn joined(left: Plan, right: Plan, keys: Option<Vec<(usize, usize)>>, db: &Arc<State>, tx: TxId) -> RowIter {
    let mut work = Work::of(db, tx);
    let Some(keys) = keys else {
        return match Rescanned::of(execute(right, db, tx), &mut work) {
            Ok((right, _)) => rescan::product(execute(left, db, tx), right),
            Err(e) => failed(e),
        };
    };
    match Built::of(execute(right, db, tx), keys, (&mut work, Full::Partition)) {
        Ok(Right::Held(built)) => Box::new(HashJoin { left: execute(left, db, tx), right: built, current: None }),
        Ok(Right::Partitioned(parts)) => partitioned::joined(execute(left, db, tx), parts, work),
        Err(e) => failed(e),
    }
}

/// The right operand of a hash join, its rows with a null key left out, as
/// none matches [X3.135-1986 §5.11 GR2]: by bucket of key values, in
/// memory, or in a table of its spill area keyed by bucket if they outgrew
/// it. Either way a bucket's rows are in order, and read one at a time
/// (`Probe`), so a bucket of more rows than the work area holds is never in
/// memory whole.
pub(in crate::exec) struct Built {
    keys: Vec<(usize, usize)>,
    /// The place in `held` of each bucket's rows.
    buckets: HashMap<Vec<Bucket>, usize>,
    held: Vec<Vec<Row>>,
    spilled: Option<Box<dyn SpillTable>>,
    /// The types beyond the standard its spilled rows may hold values of.
    types: TypeRegistry,
}

/// What a hash join's right operand does once it outgrows its work area:
/// it is partitioned, or moves to a table, as one already a partition,
/// a LEFT join's read by batches and a subquery's rows do.
#[derive(Clone, Copy)]
pub(in crate::exec) enum Full {
    Partition,
    Table,
}

/// A hash join's right operand: built, or in partitions.
pub(in crate::exec) enum Right {
    Held(Built),
    Partitioned(partitioned::Partitions),
}

/// The rows of a bucket of a `Built`, as they are read: the place of the
/// bucket held in memory, if it has one, and of its next row; or its
/// records, read from the spill area's table as they are taken.
pub(in crate::exec) enum Probe {
    Held(Option<usize>, usize),
    Spilled(Records),
}

impl Built {
    /// The right operand `rows`, joined on `keys`, built in `work`, and
    /// once it outgrows it, done with as `full` says.
    pub(in crate::exec) fn of(
        mut rows: impl Iterator<Item = Result<Row>>,
        keys: Vec<(usize, usize)>,
        (work, full): (&mut Work, Full),
    ) -> Result<Right> {
        let mut built = Built { keys, buckets: HashMap::new(), held: Vec::new(), spilled: None, types: work.types().clone() };
        let mut bytes = 0_usize;
        for r in rows.by_ref() {
            let row = r?;
            let Some(b) = key(&row, built.keys.iter().map(|k| k.1))? else { continue };
            bytes = bytes.saturating_add(row_bytes(&row));
            built.add(b, row)?;
            if bytes > work.limit() {
                return match full {
                    Full::Partition => Ok(Right::Partitioned(partitioned::Partitions::of(built, rows, work)?)),
                    Full::Table => built.spilled(rows, work).map(Right::Held),
                };
            }
        }
        Ok(Right::Held(built))
    }

    /// Adds `row`, of key values `b`, after the rows of its bucket.
    fn add(&mut self, b: Vec<Bucket>, row: Row) -> Result<()> {
        let next = self.held.len();
        let at = *self.buckets.entry(b).or_insert(next);
        if at == next {
            self.held.push(Vec::new());
        }
        self.held.get_mut(at).ok_or_else(|| broken("a hash join's bucket is missing"))?.push(row);
        Ok(())
    }

    /// Its buckets held in memory, each with its rows, in order.
    fn into_buckets(self) -> impl Iterator<Item = (Vec<Bucket>, Vec<Row>)> {
        let mut held = self.held;
        self.buckets.into_iter().map(move |(b, at)| (b, held.get_mut(at).map(std::mem::take).unwrap_or_default()))
    }

    /// It and the rest of its operand, `rows`, in a table of `work`'s
    /// spill area.
    fn spilled(mut self, rows: impl Iterator<Item = Result<Row>>, work: &mut Work) -> Result<Built> {
        let mut table = work.table()?;
        let (buckets, mut held) = (std::mem::take(&mut self.buckets), std::mem::take(&mut self.held));
        for (bucket, at) in buckets {
            let key = bucket_key(&bucket);
            for one in held.get_mut(at).map(std::mem::take).unwrap_or_default() {
                table.insert(&key, &row_record(&one))?;
            }
        }
        for r in rows {
            let row = r?;
            if let Some(b) = key(&row, self.keys.iter().map(|k| k.1))? {
                table.insert(&bucket_key(&b), &row_record(&row))?;
            }
        }
        self.spilled = Some(table);
        Ok(self)
    }

    /// The rows of bucket `b`, read as they are taken.
    pub(in crate::exec) fn bucket_rows(&self, b: &[Bucket]) -> Result<BucketRows<'_>> {
        Ok(BucketRows { built: self, probe: self.probe(b)? })
    }

    /// The pairs of a left column and a right column it is joined on.
    pub(in crate::exec) fn keys(&self) -> &[(usize, usize)] {
        &self.keys
    }

    /// The rows of bucket `b`, to be read with `next_of`.
    fn probe(&self, b: &[Bucket]) -> Result<Probe> {
        match &self.spilled {
            Some(table) => Ok(Probe::Spilled(table.values(&bucket_key(b))?)),
            None => Ok(Probe::Held(self.buckets.get(b).copied(), 0)),
        }
    }

    /// The next row of `probe`, borrowed if held in memory.
    fn next_of(&self, probe: &mut Probe) -> Option<Result<Cow<'_, Row>>> {
        match probe {
            Probe::Held(at, next) => {
                let row = self.held.get((*at)?)?.get(*next)?;
                *next = next.saturating_add(1);
                Some(Ok(Cow::Borrowed(row)))
            }
            Probe::Spilled(records) => Some(records.next()?.and_then(|r| record_row(&r, &self.types)).map(Cow::Owned)),
        }
    }
}

/// The rows of a bucket of a `Built`, as they are read.
pub(in crate::exec) struct BucketRows<'a> {
    built: &'a Built,
    probe: Probe,
}

impl<'a> Iterator for BucketRows<'a> {
    type Item = Result<Cow<'a, Row>>;

    fn next(&mut self) -> Option<Result<Cow<'a, Row>>> {
        self.built.next_of(&mut self.probe)
    }
}

/// [X3.135-1986 §5.20 GR2b, §5.21 GR2] each row of the left concatenated
/// with each row of the right whose `keys` (a left column, a right column)
/// are equal [X3.135-1986 §5.11]: a null equals nothing (GR2). The left is
/// read in order, so the joined rows are in its order; a left row's
/// matches are given as its bucket is read, never gathered.
struct HashJoin {
    left: RowIter,
    right: Built,
    /// The left row being joined, and its bucket, being read.
    current: Option<(Row, Probe)>,
}

impl HashJoin {
    /// The next joined row, if there is one.
    fn step(&mut self) -> Result<Option<Row>> {
        loop {
            if let Some((l, probe)) = &mut self.current {
                let Some(r) = self.right.next_of(probe).transpose()? else {
                    self.current = None;
                    continue;
                };
                if equal(&self.right.keys, l, &r)? {
                    return Ok(Some(l.iter().chain(r.iter()).cloned().collect()));
                }
                continue;
            }
            let Some(l) = self.left.next().transpose()? else { return Ok(None) };
            if let Some(b) = key(&l, self.right.keys.iter().map(|k| k.0))? {
                self.current = Some((l, self.right.probe(&b)?));
            }
        }
    }
}

impl Iterator for HashJoin {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        self.step().transpose()
    }
}

/// The buckets of `row`'s values in `columns`, or none if one is null.
///
/// # Errors
///
/// A column the row does not have (SQLCODE -910).
fn key(row: &Row, columns: impl Iterator<Item = usize>) -> Result<Option<Vec<Bucket>>> {
    let mut buckets = Vec::new();
    for c in columns {
        match bucket(column(row, c)?) {
            Some(b) => buckets.push(b),
            None => return Ok(None),
        }
    }
    Ok(Some(buckets))
}

/// Whether the values of `l` and `r` the pairs `keys` name are equal, as
/// rows in one bucket may differ.
fn equal(keys: &[(usize, usize)], l: &Row, r: &Row) -> Result<bool> {
    for &(lc, rc) in keys {
        if compare(column(l, lc)?, column(r, rc)?) != Some(Ordering::Equal) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Gives `each` left row `l` concatenated with each row of `right` whose
/// keys equal its own, in order, as its bucket is read.
fn each_match(right: &Built, l: &Row, mut each: impl FnMut(Row) -> Result<()>) -> Result<()> {
    let Some(b) = key(l, right.keys.iter().map(|k| k.0))? else { return Ok(()) };
    for r in right.bucket_rows(&b)? {
        let r = r?;
        if equal(&right.keys, l, &r)? {
            each(l.iter().chain(r.iter()).cloned().collect())?;
        }
    }
    Ok(())
}
