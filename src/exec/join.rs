//! The FROM product [X3.135-1986 §5.20 GR2b], the hash join, which gives
//! the rows of a product that equalities of its columns keep
//! (`physical/join.rs`), and the outer join [ISO/IEC 9075:1992 §7.5]. A
//! hash join whose right operand outgrows its work area is partitioned
//! (`exec/join/partitioned.rs`, RULES.md D38); a product's right operand is
//! read again for each left row, from a run of the spill area once it
//! outgrows the work area (`exec/join/rescan.rs`); and an outer join's is
//! either (`exec/join/outer.rs`). Split from exec.rs (RULES.md R-SIZE-2).

mod outer;
mod partitioned;
mod rescan;

pub(super) use outer::outer;
pub use partitioned::{DEFAULT_PARTITION_DEPTH, DEFAULT_PARTITIONS};
pub(super) use rescan::Rescanned;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use super::work::Work;
use super::{RowIter, column, execute, failed};
use crate::db::State;
use crate::error::Result;
use crate::physical::Plan;
use crate::spill::{SpillTable, bucket_key, record_row, row_bytes, row_record};
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::{Bucket, bucket, compare};

/// The rows of `left` joined with those of `right`: on `keys` if given
/// (`hash_join`), else with every one (`product`). The right operand is
/// read first, and the left only if it succeeded. Never inlined into
/// `execute`, whose frame every level of a plan adds to the stack (RULES.md
/// R-CF-6).
#[inline(never)]
pub(super) fn joined(left: Plan, right: Plan, keys: Option<Vec<(usize, usize)>>, db: &Arc<State>, tx: TxId) -> RowIter {
    let mut work = Work::new(db);
    let Some(keys) = keys else {
        return match Rescanned::of(execute(right, db, tx), &mut work) {
            Ok((right, _)) => rescan::product(execute(left, db, tx), right),
            Err(e) => failed(e),
        };
    };
    match Built::of(execute(right, db, tx), keys, (&mut work, Full::Partition)) {
        Ok(Right::Held(built)) => hash_join(execute(left, db, tx), built),
        Ok(Right::Partitioned(parts)) => partitioned::joined(execute(left, db, tx), parts, work),
        Err(e) => failed(e),
    }
}

/// The right operand of a hash join, its rows with a null key left out, as
/// none matches [X3.135-1986 §5.11 GR2]: by bucket of key values, in
/// memory, or in a table of its spill area keyed by bucket if they outgrew
/// it. Either way a bucket's rows are in order.
pub(in crate::exec) struct Built {
    keys: Vec<(usize, usize)>,
    buckets: HashMap<Vec<Bucket>, Vec<Row>>,
    spilled: Option<Box<dyn SpillTable>>,
}

/// What a hash join's right operand does once it outgrows its work area:
/// it is partitioned, or, already a partition, moves to a table.
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

impl Built {
    /// The right operand `rows`, joined on `keys`, built in `work`, and
    /// once it outgrows it, done with as `full` says.
    pub(in crate::exec) fn of(
        mut rows: impl Iterator<Item = Result<Row>>,
        keys: Vec<(usize, usize)>,
        (work, full): (&mut Work, Full),
    ) -> Result<Right> {
        let mut built = Built { keys, buckets: HashMap::new(), spilled: None };
        let mut bytes = 0_usize;
        for r in rows.by_ref() {
            let row = r?;
            let Some(b) = key(&row, built.keys.iter().map(|k| k.1))? else { continue };
            bytes = bytes.saturating_add(row_bytes(&row));
            built.buckets.entry(b).or_default().push(row);
            if bytes > work.limit() {
                return match full {
                    Full::Partition => Ok(Right::Partitioned(partitioned::Partitions::of(built, rows, work)?)),
                    Full::Table => built.spilled(rows, work).map(Right::Held),
                };
            }
        }
        Ok(Right::Held(built))
    }

    /// It and the rest of its operand, `rows`, in a table of `work`'s
    /// spill area.
    fn spilled(mut self, rows: impl Iterator<Item = Result<Row>>, work: &mut Work) -> Result<Built> {
        let mut table = work.table()?;
        for (bucket, held) in self.buckets.drain() {
            let at = bucket_key(&bucket);
            for one in &held {
                table.insert(&at, &row_record(one))?;
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

    /// The rows of bucket `b`.
    pub(in crate::exec) fn bucket(&self, b: &[Bucket]) -> Result<Vec<Row>> {
        match &self.spilled {
            Some(table) => table.get(&bucket_key(b))?.iter().map(|r| record_row(r)).collect(),
            None => Ok(self.buckets.get(b).cloned().unwrap_or_default()),
        }
    }
}

/// [X3.135-1986 §5.20 GR2b, §5.21 GR2] each row of the left concatenated
/// with each row of the right whose `keys` (a left column, a right column)
/// are equal [X3.135-1986 §5.11]: a null equals nothing (GR2). The left is
/// read in order, so the joined rows are in its order.
fn hash_join(left: RowIter, right: Built) -> RowIter {
    Box::new(left.flat_map(move |l| {
        let out: Vec<Result<Row>> = match l.and_then(|l| matches(&right, &l)) {
            Ok(rows) => rows.into_iter().map(Ok).collect(),
            Err(e) => vec![Err(e)],
        };
        out
    }))
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

/// `l` concatenated with each row of `right` whose keys equal its own:
/// those in its bucket, compared, as rows in one bucket may differ.
fn matches(right: &Built, l: &Row) -> Result<Vec<Row>> {
    let Some(b) = key(l, right.keys.iter().map(|k| k.0))? else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for r in right.bucket(&b)? {
        let mut equal = true;
        for &(lc, rc) in &right.keys {
            equal &= compare(column(l, lc)?, column(&r, rc)?) == Some(Ordering::Equal);
        }
        if equal {
            out.push(l.iter().chain(&r).cloned().collect());
        }
    }
    Ok(out)
}
