//! \[DESIGN\] An outer join of streams probed by batches (RULES.md D31,
//! [ISO/IEC 9075:1992 §7.5]): a FULL or RIGHT one, and a LEFT one the
//! simple probe does not take (`probe.rs`): one whose condition can raise
//! an error or whose operand has a condition of its own. Its right operand
//! is read first and held as the row path holds it (`exec/join/outer.rs`):
//! on keys, hashed, each row's place kept by bucket; on none, every row a
//! candidate of each left row. Each of the left operand's batches probes
//! it, a flag kept of each right row, whether it matched; once the left is
//! read, the right rows that matched none, those of a null key among them,
//! are given after nulls if the right is preserved.
//!
//! As the row path judges them: an operand's own condition of each of its
//! rows once the other is known to have one, a row it is not true of
//! matching none (RULES.md D15), the right's of each right row as the left
//! gives its first; the condition of each pair of equal keys, or of every
//! pair on no keys, of a row of the pair's values. A right operand past the
//! work area is joined as the row path joins it, partitioned or from a run,
//! its rows gathered into batches.

use std::cmp::Ordering;
use std::sync::Arc;

use super::super::stream::{self, Batches, Kept};
use super::pairs::{Out, kept_rows};
use super::{gathered, left_batches, operand_rows};
use crate::db::State;
use crate::error::Result;
use crate::exec::eval_cond;
use crate::exec::join::{Holding, Joining, Prepared, prepared};
use crate::logical::{Truth, cond_columns};
use crate::physical::Plan;
use crate::storage::{Batch, Row};
use crate::txn::TxId;
use crate::value::{Bucket, Value, bucket, compare};

/// The batches of the outer join `joining` says of `left` and `right`.
///
/// # Errors
///
/// The right operand's; the spill area's; an operand's own condition of a
/// join on no keys; a column the left's batches lack.
pub(super) fn joined(joining: Joining, (left, right): (Plan, Plan), db: &Arc<State>, tx: TxId) -> Result<Batches> {
    let (left_columns, left_width) = stream::layout(&left, db).unwrap_or_default();
    let (right_columns, _) = stream::layout(&right, db).unwrap_or_default();
    // The right operand first, as the row path reads it.
    match prepared(joining, operand_rows(right, db, tx), (db, tx))? {
        Prepared::Held(held) => {
            let read = places(&held, &left_columns)?;
            let width = held.spec.widths.0.saturating_add(held.spec.widths.1);
            let matched = vec![false; held.rows.len()];
            let out = Out::new(left_columns.len(), right_columns);
            let left = left_batches(left, db, tx)?;
            Ok(Box::new(Probe { left, held, read, scratch: vec![Value::Null; width], matched, barred: None, state: Phase::Probing, out }))
        }
        Prepared::Rowwise(rowwise) => {
            let mut columns = left_columns;
            columns.extend(right_columns.iter().map(|&c| c.saturating_add(left_width)));
            Ok(gathered(rowwise.joined(operand_rows(left, db, tx)), &columns))
        }
    }
}

/// Where in a left batch the join reads its left rows' columns: of each
/// key, its left column's place with its right column; of each left column
/// its conditions read, its place with the column.
struct Read {
    keys: Vec<(usize, usize)>,
    columns: Vec<(usize, usize)>,
}

/// The places in a batch of `left_columns` of the left columns `held`'s
/// join reads.
///
/// # Errors
///
/// A column the batch lacks (SQLCODE -910).
fn places(held: &Holding, left_columns: &[usize]) -> Result<Read> {
    let at = |c: usize| {
        left_columns.iter().position(|&h| h == c).ok_or_else(|| crate::exec::broken("a column an outer join reads its batch lacks"))
    };
    let keys = held.spec.keys.iter().map(|&(l, r)| at(l).map(|a| (a, r))).collect::<Result<Vec<_>>>()?;
    let mut read = Vec::new();
    for c in [held.spec.cond.as_ref(), held.spec.own.0.as_ref()].into_iter().flatten() {
        cond_columns(c, &mut read);
    }
    read.sort_unstable();
    read.dedup();
    let columns = read.into_iter().filter(|&c| c < held.spec.widths.0).map(|c| at(c).map(|a| (a, c))).collect::<Result<Vec<_>>>()?;
    Ok(Read { keys, columns })
}

/// Where an outer join probing by batches is.
#[derive(Clone, Copy)]
enum Phase {
    /// Before its left operand has given every batch.
    Probing,
    /// Giving its unmatched right rows, from this place.
    Tail(usize),
    /// Done, or ended by an error.
    Done,
}

/// An outer join's batches: each of its left operand's probing its right,
/// held, then its right rows that matched none.
struct Probe {
    left: Batches,
    held: Holding,
    read: Read,
    /// The row the conditions are judged of: the left row's columns they
    /// read, then the right row's.
    scratch: Row,
    /// Of each right row, whether a left row matched it.
    matched: Vec<bool>,
    /// Of each right row, once the left has given a row, whether its own
    /// condition bars it from matching; none until then, or with none.
    barred: Option<Vec<bool>>,
    state: Phase,
    out: Out,
}

impl Probe {
    /// Probes the right operand with the rows of `batch` at `rows`.
    fn probe(&mut self, batch: &Batch, rows: &[usize]) -> Result<()> {
        let mut key: Vec<Bucket> = Vec::with_capacity(self.read.keys.len());
        let mut values: Vec<Value> = Vec::with_capacity(self.read.keys.len());
        for &r in rows {
            self.bar()?;
            for &(at, c) in &self.read.columns {
                if let Some(slot) = self.scratch.get_mut(c) {
                    *slot = batch.cell(at, r).to_value();
                }
            }
            values.clear();
            values.extend(self.read.keys.iter().map(|&(at, _)| batch.cell(at, r).to_value()));
            key.clear();
            // A null equals nothing [X3.135-1986 §5.11 GR2].
            let keyed = values.iter().all(|v| bucket(v).map(|b| key.push(b)).is_some());
            // Its own condition judged of it as the right has a row, its key
            // null or not [§7.5 GR1c].
            let may = !self.held.rows.is_empty() && self.held.spec.left_may_match(&self.scratch)?;
            let found = may && keyed && self.matches((batch, r), (&key, &values))?;
            // [ISO/IEC 9075:1992 §7.5 GR4, GR5b] A preserved left row no
            // right row matches, with nulls.
            if !found && self.held.spec.kind.keeps_left() {
                self.out.push(Some(batch), Some(r), None);
            }
        }
        Ok(())
    }

    /// Joins row `r` of `batch`, of key buckets `key` and values `values`,
    /// with each right row it matches, flagged; whether one did.
    fn matches(&mut self, (batch, r): (&Batch, usize), (key, values): (&[Bucket], &[Value])) -> Result<bool> {
        let rows = Arc::clone(&self.held.rows);
        let places = self.held.buckets.as_ref().map(|b| b.get(key).cloned().unwrap_or_else(|| Arc::from([])));
        let count = places.as_ref().map_or(rows.len(), |p| p.len());
        let mut found = false;
        for i in 0..count {
            let n = places.as_ref().map_or(Some(i), |p| p.get(i).copied()).unwrap_or(i);
            let barred = self.barred.as_ref().and_then(|b| b.get(n)).copied().unwrap_or(false);
            let Some(right) = rows.get(n).filter(|_| !barred) else { continue };
            let equal =
                self.read.keys.iter().zip(values).all(|(&(_, rc), v)| right.get(rc).and_then(|w| compare(v, w)) == Some(Ordering::Equal));
            if !equal || !self.holds(right)? {
                continue;
            }
            found = true;
            if let Some(m) = self.matched.get_mut(n) {
                *m = true;
            }
            self.out.push(Some(batch), Some(r), Some(right));
        }
        Ok(found)
    }

    /// Whether the join's condition, if any, is true of the left row in
    /// the scratch row and `right` [ISO/IEC 9075:1992 §7.5 GR1c].
    fn holds(&mut self, right: &Row) -> Result<bool> {
        let Some(cond) = &self.held.spec.cond else { return Ok(true) };
        for (slot, v) in self.scratch.iter_mut().skip(self.held.spec.widths.0).zip(right) {
            slot.clone_from(v);
        }
        Ok(eval_cond(cond, &self.scratch, &self.held.spec.runner)? == Truth::True)
    }

    /// Judges the right operand's own condition of each of its rows, once,
    /// as the left gives its first row [ISO/IEC 9075:1992 §7.5 GR1c].
    fn bar(&mut self) -> Result<()> {
        let Some(own) = self.held.spec.own.1.as_ref().filter(|_| self.barred.is_none()) else { return Ok(()) };
        let mut barred = Vec::with_capacity(self.held.rows.len());
        for row in self.held.rows.iter() {
            barred.push(eval_cond(own, row, &self.held.spec.runner)? != Truth::True);
        }
        self.barred = Some(barred);
        Ok(())
    }

    /// Gives the right rows from place `from` that matched none, after
    /// nulls, until a batch is made or none is left.
    fn tail(&mut self, from: usize) {
        let rows = Arc::clone(&self.held.rows);
        let mut n = from;
        while n < rows.len() && !self.out.ready() {
            if !self.matched.get(n).copied().unwrap_or(true) {
                self.out.push(None, None, rows.get(n));
            }
            n = n.saturating_add(1);
        }
        if n < rows.len() {
            self.state = Phase::Tail(n);
        } else {
            self.out.flush(None);
            self.state = Phase::Done;
        }
    }

    /// Probes with the rows of `batch` kept, `rows`, its joined rows put in
    /// batches; the rows joined before an error, then the error.
    fn batch(&mut self, batch: &Batch, rows: Option<Vec<usize>>) {
        let done = self.probe(batch, &kept_rows(rows, batch.rows));
        if self.out.settle(Some(batch), done) {
            self.state = Phase::Done;
        }
    }
}

impl Iterator for Probe {
    type Item = Result<Kept>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(given) = self.out.next() {
                return Some(given);
            }
            match self.state {
                Phase::Done => return None,
                Phase::Tail(from) => self.tail(from),
                Phase::Probing => match self.left.next() {
                    Some(Ok((batch, rows))) => self.batch(&batch, rows),
                    Some(Err(e)) => return Some(Err(e)),
                    None => self.state = if self.held.spec.kind.keeps_right() { Phase::Tail(0) } else { Phase::Done },
                },
            }
        }
    }
}
