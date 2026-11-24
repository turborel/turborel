//! \[DESIGN\] Semi joins and antijoins (`physical/semi.rs`, RULES.md D540):
//! each row of the left operand, once and alone, if a row of the right
//! matches it, or of an antijoin if none does. A row matches if its keys
//! match the left row's, equal, or not distinct where a null matches a
//! null [ISO/IEC 9075-2:2023 §8.15], and the rest of the condition is true
//! of the pair; the first that matches decides, and no other is read.
//!
//! * Hashed, the right's rows are held by bucket of their keys once the
//!   first left row is read, so an empty left reads none; of no condition
//!   beyond the keys only the first row of each key is held. Past the work
//!   area they move to a table of its spill area (`Built`, D38).
//! * Built left, the left's rows are held in their order with a mark each,
//!   the right read once, marking each held row it matches; then the left's
//!   rows are given, those marked, or of an antijoin those not. Past the
//!   work area it gives way to the hashed join, the left read on from where
//!   it was.
//! * Nested, the right's rows are held, in memory or a run of the spill
//!   area (`Rescanned`), and read for each left row until one matches.
//!
//! The left is read in order, so the rows are in its order.

use super::super::work::Work;
use super::super::{RowIter, Runner, batch, broken, column, eval_cond, execute, failed};
use super::{Built, Full, Operand, Rescanned, Right, equal, key};
use crate::error::Result;
use crate::exec::Db;
use crate::hash::xxhash::Xxh3Map;
use crate::logical::{Cond, Truth};
use crate::physical::{JoinKey, Plan, SemiJoin, SemiMethod};
use crate::spill::row_bytes;
use crate::storage::Row;
use crate::txn::{TxId, checked};
use crate::value::{Bucket, key_equal};

/// The rows of the semi join or antijoin `plan`. Never inlined into
/// `execute`, whose frame every level of a plan adds to the stack (RULES.md
/// R-CF-6).
#[inline(never)]
pub(in crate::exec) fn semi(plan: Plan, db: &Db, tx: TxId) -> RowIter {
    if let Some((columns, width)) = by_batch(&plan, db) {
        return match batch::whole_batches(plan, db, tx) {
            Ok(batches) => batch::rows_of(batches, columns, width),
            Err(e) => failed(e),
        };
    }
    let Plan::SemiJoin(s) = plan else { return failed(broken("a semi join is not one")) };
    let SemiJoin { anti, method, left, right, keys, cond, .. } = *s;
    let judge = Judge { keys, cond, anti, runner: Runner::new(db, tx) };
    let work = Work::of(db, tx);
    let stop = db.tx.stop_of(tx);
    match method {
        SemiMethod::Hash => {
            let right = Some(execute(right, db, tx));
            Box::new(HashSemi { left: execute(left, db, tx), right, side: None, judge, work })
        }
        SemiMethod::BuildLeft => match built_left(judge, (left, right), work, (db, tx)) {
            Ok(rows) => Box::new(checked(rows, stop)),
            Err(e) => failed(e),
        },
        SemiMethod::Nested => {
            let mut work = work;
            match Rescanned::of(execute(right, db, tx), &mut work) {
                Ok((held, _)) => Box::new(checked(NestedSemi { left: execute(left, db, tx), right: held, judge }, stop)),
                Err(e) => failed(e),
            }
        }
    }
}

/// \[DESIGN\] (RULES.md D1071) The columns the batches of the semi join
/// `plan` hold, and its rows' width, if its rows are made of its batches
/// though an operator reads it row by row, as a sort of rows not held
/// whole does: a hashed join of streams (`batch::layout`) every table of
/// which reads by batch, while the database reads by batch
/// (EXECUTOR.BATCHES). Its operands are then read by batch, and by
/// morsels where the plan says so (`batch/joined/semi.rs`), not a row at
/// a time, and it gives its left's rows in the order the row path does.
fn by_batch(plan: &Plan, db: &Db) -> Option<(Vec<usize>, usize)> {
    if !crate::exec::batched(db) || !batch::reads_by_batch(plan, db) {
        return None;
    }
    batch::layout(plan, db)
}

/// Whether a right row matches a left row: its keys, the rest of the
/// condition, and whether the join keeps the rows none matches.
struct Judge {
    keys: Vec<JoinKey>,
    cond: Option<Cond>,
    anti: bool,
    runner: Runner,
}

impl Judge {
    /// Whether `r` matches `l`.
    fn matches(&self, l: &Row, r: &Row) -> Result<bool> {
        if !equal(&self.keys, l, r)? {
            return Ok(false);
        }
        let Some(cond) = &self.cond else { return Ok(true) };
        let pair: Row = l.iter().chain(r.iter()).cloned().collect();
        Ok(eval_cond(cond, &pair, &self.runner)? == Truth::True)
    }

    /// Whether the left row a match was or was not found for is kept.
    fn kept(&self, found: bool) -> bool {
        found != self.anti
    }

    /// Whether the right rows `a` and `b` have the same keys, so of no
    /// condition beyond them the second need not be held.
    fn same_keys(&self, a: &Row, b: &Row) -> Result<bool> {
        for k in &self.keys {
            if !key_equal(column(a, k.right)?, column(b, k.right)?, k.nulls)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// A hashed join's right rows: by bucket in memory, or past the work
/// area in its spill area.
enum Side {
    Held(Xxh3Map<Vec<Bucket>, Vec<Row>>),
    Spilled(Built),
}

impl Side {
    /// The right `rows` of `judge`'s join, held in `work`.
    fn of(mut rows: RowIter, judge: &Judge, work: &mut Work) -> Result<Side> {
        let mut held: Xxh3Map<Vec<Bucket>, Vec<Row>> = Xxh3Map::default();
        let mut bytes = 0_usize;
        while let Some(r) = rows.next() {
            let row = r?;
            let Some(b) = key(&row, &judge.keys, Operand::Right)? else { continue };
            let bucket = held.entry(b).or_default();
            if judge.cond.is_none() && first_held(bucket, &row, judge)? {
                continue;
            }
            bytes = bytes.saturating_add(row_bytes(&row));
            bucket.push(row);
            if bytes > work.limit() {
                let all = held.into_values().flatten().map(Ok).chain(rows);
                return match Built::of(all, judge.keys.clone(), (work, Full::Table))? {
                    Right::Held(built) => Ok(Side::Spilled(built)),
                    Right::Partitioned(_) => Err(broken("a semi join's right rows were partitioned")),
                };
            }
        }
        Ok(Side::Held(held))
    }

    /// Whether a row of it matches `l`.
    fn found(&self, l: &Row, judge: &Judge) -> Result<bool> {
        let Some(b) = key(l, &judge.keys, Operand::Left)? else { return Ok(false) };
        match self {
            Side::Held(held) => {
                for r in held.get(&b).into_iter().flatten() {
                    if judge.matches(l, r)? {
                        return Ok(true);
                    }
                }
            }
            Side::Spilled(built) => {
                for r in built.bucket_rows(&b)? {
                    if judge.matches(l, &*r?)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }
}

/// Whether `bucket` holds a row of `row`'s keys already.
fn first_held(bucket: &[Row], row: &Row, judge: &Judge) -> Result<bool> {
    for held in bucket {
        if judge.same_keys(held, row)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The hashed semi join: each left row looked up among the right's,
/// held once the first left row is read.
struct HashSemi {
    left: RowIter,
    right: Option<RowIter>,
    side: Option<Side>,
    judge: Judge,
    work: Work,
}

impl HashSemi {
    /// The next left row kept, if there is one.
    fn step(&mut self) -> Result<Option<Row>> {
        while let Some(l) = self.left.next().transpose()? {
            if self.side.is_none() {
                let rows = self.right.take().ok_or_else(|| broken("a semi join's right rows were read twice"))?;
                self.side = Some(Side::of(rows, &self.judge, &mut self.work)?);
            }
            let side = self.side.as_ref().ok_or_else(|| broken("a semi join holds no right rows"))?;
            if self.judge.kept(side.found(&l, &self.judge)?) {
                return Ok(Some(l));
            }
        }
        Ok(None)
    }
}

impl Iterator for HashSemi {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        self.step().transpose()
    }
}

/// The right semi join: the left held and marked, the right read once.
fn built_left(judge: Judge, (left, right): (Plan, Plan), work: Work, (db, tx): (&Db, TxId)) -> Result<RowIter> {
    let mut rows = execute(left, db, tx);
    let (mut held, mut bytes) = (Vec::new(), 0_usize);
    let mut index: Xxh3Map<Vec<Bucket>, Vec<usize>> = Xxh3Map::default();
    while let Some(r) = rows.next() {
        let row = r?;
        bytes = bytes.saturating_add(row_bytes(&row));
        if let Some(b) = key(&row, &judge.keys, Operand::Left)? {
            index.entry(b).or_default().push(held.len());
        }
        held.push(row);
        if bytes > work.limit() {
            // Too many to hold: the hashed join of them and the rest.
            let rest: RowIter = Box::new(held.into_iter().map(Ok).chain(rows));
            return Ok(Box::new(HashSemi { left: rest, right: Some(execute(right, db, tx)), side: None, judge, work }));
        }
    }
    // Of no left row, no right row is read.
    if held.is_empty() {
        return Ok(Box::new(std::iter::empty()));
    }
    let mut marks = vec![false; held.len()];
    for r in execute(right, db, tx) {
        let r = r?;
        let Some(b) = key(&r, &judge.keys, Operand::Right)? else { continue };
        for &at in index.get(&b).into_iter().flatten() {
            let (Some(l), Some(false)) = (held.get(at), marks.get(at).copied()) else { continue };
            if judge.matches(l, &r)?
                && let Some(mark) = marks.get_mut(at)
            {
                *mark = true;
            }
        }
    }
    let anti = judge.anti;
    Ok(Box::new(held.into_iter().zip(marks).filter(move |&(_, marked)| marked != anti).map(|(row, _)| Ok(row))))
}

/// The nested-loop semi join: the right held, read for each left row.
struct NestedSemi {
    left: RowIter,
    right: Rescanned,
    judge: Judge,
}

impl NestedSemi {
    /// The next left row kept, if there is one.
    fn step(&mut self) -> Result<Option<Row>> {
        while let Some(l) = self.left.next().transpose()? {
            let mut found = false;
            for r in self.right.pass()? {
                if self.judge.matches(&l, &r?)? {
                    found = true;
                    break;
                }
            }
            if self.judge.kept(found) {
                return Ok(Some(l));
            }
        }
        Ok(None)
    }
}

impl Iterator for NestedSemi {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        self.step().transpose()
    }
}
