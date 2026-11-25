//! \[DESIGN\] An outer join of streams probed by batches (RULES.md D31,
//! [ISO/IEC 9075:1992 §7.5]): a FULL or RIGHT one, and a LEFT one the
//! simple probe does not take (`probe.rs`): one whose condition can raise
//! an error or whose operand has a condition of its own. Its right operand
//! is read first and held: on keys, as vectors while they fit the work
//! area (`held.rs`), each row's place kept by key, a row of its values
//! made only where a condition is judged of it; on none, as the row path
//! holds it (`exec/join/outer.rs`), every row a candidate of each left
//! row. Each of the left operand's batches probes
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
//! its rows gathered into batches: where no order is asked of the join, each
//! pair of partitions' rows as they are joined (`Rowwise::unordered`).

use std::sync::Arc;

use super::super::stream::{self, Batches, Kept};
use super::held::{self, HeldRight, Read as HeldRead};
use super::pairs::{Out, kept_rows};
use super::probe::{keyed, keys_match};
use super::{gathered, in_order, left_batches, operand_rows};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::eval_cond;
use crate::exec::join::{Holding, Joining, Prepared, Spec, prepared, spec_of};
use crate::exec::table::KeyHashes;
use crate::exec::work::Work;
use crate::logical::{Truth, cond_columns};
use crate::physical::{JoinKey, Plan};
use crate::storage::{Batch, Row};
use crate::txn::TxId;
use crate::value::{Bucket, Value};

/// The batches of the outer join `joining` says of `left` and `right`.
///
/// # Errors
///
/// The right operand's; the spill area's; an operand's own condition of a
/// join on no keys; a column the left's batches lack.
pub(super) fn joined(joining: Joining, (left, right): (Plan, Plan), db: &Db, tx: TxId) -> Result<Batches> {
    let (left_columns, left_width) = stream::layout(&left, db).unwrap_or_default();
    let (right_columns, _) = stream::layout(&right, db).unwrap_or_default();
    let spec = spec_of(joining, (db, tx))?;
    let work = Work::of(db, tx);
    // The right operand first: of a join on keys, held as vectors if it
    // fits the work area; else as the row path reads it.
    let rows = if spec.keys.is_empty() {
        operand_rows(right, db, tx)
    } else {
        match held::read(right, (&spec.keys, &[], Vec::new()), work.limit(), (db, tx))? {
            HeldRead::Held(held) => {
                let out = Out::over(left_columns.len(), Arc::clone(&held.batch), crate::db::kernels::chosen(db));
                let read = places(&spec, &left_columns)?;
                let vectors = Right::Vectors(Box::new(Vectors { held: *held, columns: right_columns, spec, _work: work }));
                return Ok(Box::new(Probe::new((left_batches(left, db, tx)?, vectors), read, out)));
            }
            HeldRead::Rest(rest) => rest.rows(),
        }
    };
    match prepared(spec, rows, work)? {
        Prepared::Held(held) => {
            let read = places(&held.spec, &left_columns)?;
            let out = Out::new(left_columns.len(), right_columns, crate::db::kernels::chosen(db));
            Ok(Box::new(Probe::new((left_batches(left, db, tx)?, Right::Rows(Box::new(held))), read, out)))
        }
        Prepared::Rowwise(rowwise) => {
            let mut columns = left_columns;
            columns.extend(right_columns.iter().map(|&c| c.saturating_add(left_width)));
            // In no order but the partitions', where none is asked for.
            let ordered = in_order(&left);
            let left_rows = operand_rows(left, db, tx);
            Ok(gathered(if ordered { rowwise.joined(left_rows) } else { rowwise.unordered(left_rows) }, &columns))
        }
    }
}

/// Where in a left batch the join reads its left rows' columns: of each
/// key, its left column's place with its right column; of each left column
/// its conditions read, its place with the column.
struct Read {
    keys: Vec<JoinKey>,
    columns: Vec<(usize, usize)>,
}

/// The places in a batch of `left_columns` of the left columns the join
/// `spec` says reads.
///
/// # Errors
///
/// A column the batch lacks (SQLCODE -910).
fn places(spec: &Spec, left_columns: &[usize]) -> Result<Read> {
    let at = |c: usize| {
        left_columns.iter().position(|&h| h == c).ok_or_else(|| crate::exec::broken("a column an outer join reads its batch lacks"))
    };
    let keys = spec.keys.iter().map(|&k| at(k.left).map(|a| JoinKey { left: a, ..k })).collect::<Result<Vec<_>>>()?;
    let mut read = Vec::new();
    for c in [spec.cond.as_ref(), spec.own.0.as_ref()].into_iter().flatten() {
        cond_columns(c, &mut read);
    }
    read.sort_unstable();
    read.dedup();
    let columns = read.into_iter().filter(|&c| c < spec.widths.0).map(|c| at(c).map(|a| (a, c))).collect::<Result<Vec<_>>>()?;
    Ok(Read { keys, columns })
}

/// A right operand on keys held as vectors (`held.rs`): the columns of its
/// rows its vectors hold, and what the join matches on and keeps.
struct Vectors {
    held: HeldRight,
    columns: Vec<usize>,
    spec: Spec,
    /// The work area it is held in, held while it is.
    _work: Work,
}

/// An outer join's right operand, held: its rows, as the row path holds
/// them, or of a join on keys, as vectors.
enum Right {
    Rows(Box<Holding>),
    Vectors(Box<Vectors>),
}

impl Right {
    /// What the join matches on and keeps.
    fn spec(&self) -> &Spec {
        match self {
            Right::Rows(held) => &held.spec,
            Right::Vectors(v) => &v.spec,
        }
    }

    /// How many rows it holds.
    fn len(&self) -> usize {
        match self {
            Right::Rows(held) => held.rows.len(),
            Right::Vectors(v) => v.held.batch.rows,
        }
    }

    /// Row `n` of its values, of the right operand's width.
    fn row(&self, n: usize) -> Option<Row> {
        match self {
            Right::Rows(held) => held.rows.get(n).cloned(),
            Right::Vectors(v) => (n < v.held.batch.rows).then(|| {
                let mut row = vec![Value::Null; v.spec.widths.1];
                for (at, &c) in v.columns.iter().enumerate() {
                    if let Some(slot) = row.get_mut(c) {
                        *slot = v.held.batch.cell(at, n).to_value();
                    }
                }
                row
            }),
        }
    }

    /// Whether row `n`'s keys match those of row `r` of `batch`, its key
    /// columns at `left`, of values `values` (`key_equal`).
    ///
    /// # Errors
    ///
    /// A column the row lacks, or a value nested deeper than a type may
    /// (SQLCODE -910).
    fn equal(&self, n: usize, (batch, left, r): (&Batch, &[usize], usize), (keys, values): (&[JoinKey], &[Value])) -> Result<bool> {
        match self {
            Right::Rows(held) => match held.rows.get(n) {
                Some(right) => keys_match(keys, values, right),
                None => Ok(false),
            },
            Right::Vectors(v) => v.held.matches((batch, left, r), n),
        }
    }

    /// The places of the rows the left row of key buckets `key`, or of
    /// hash `hash`, may match: of its bucket or hash, none of a miss, or on
    /// no keys, every row's.
    fn candidates(&self, key: &[Bucket], (hash, miss): (u64, bool)) -> Option<Arc<[usize]>> {
        match self {
            Right::Rows(held) => held.buckets.as_ref().map(|b| b.get(key).cloned().unwrap_or_else(|| Arc::from([]))),
            Right::Vectors(v) => Some(if miss { Arc::from([]) } else { v.held.candidates(hash).collect() }),
        }
    }
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
    right: Right,
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
    /// The left batch probing now, the places of its rows kept, and how
    /// many of them probed.
    at: Option<(Batch, Vec<usize>, usize)>,
}

impl Probe {
    /// The probe of `right` by the batches of `left`, reading their columns
    /// at `read`, its rows put in batches by `out`.
    fn new((left, right): (Batches, Right), read: Read, out: Out) -> Probe {
        let (widths, rows) = (right.spec().widths, right.len());
        let scratch = vec![Value::Null; widths.0.saturating_add(widths.1)];
        Probe { left, right, read, scratch, matched: vec![false; rows], barred: None, state: Phase::Probing, out, at: None }
    }

    /// Probes the right operand with the rows of `batch` at `rows` from
    /// the one at `from`, until a batch of pairs is made or they end, as
    /// the inner probe does (RULES.md D1154): how many of them are probed.
    /// Of a right held as vectors, their keys are hashed a vector at a
    /// time first, as the first is probed.
    fn probe(&mut self, batch: &Batch, rows: &[usize], from: usize) -> Result<usize> {
        let left: Vec<usize> = self.read.keys.iter().map(|k| k.left).collect();
        let probing = match &mut self.right {
            Right::Vectors(v) => {
                if from == 0 {
                    v.held.hash_probing((batch, &left), rows)?;
                }
                Some(std::mem::take(&mut v.held.probing))
            }
            Right::Rows(_) => None,
        };
        let probed = self.probe_rows((batch, &left), (rows, from), probing.as_ref());
        if let (Right::Vectors(v), Some(hashes)) = (&mut self.right, probing) {
            v.held.probing = hashes;
            let probed = probed?;
            if probed >= rows.len() {
                v.held.settle()?;
            }
            return Ok(probed);
        }
        probed
    }

    /// Probes the right operand with the rows of `batch` at `rows` from the
    /// one at `from`, until a batch of pairs is made, its key columns at
    /// `left`, their keys' hashes `probing` if hashed: how many are probed.
    fn probe_rows(&mut self, (batch, left): (&Batch, &[usize]), (rows, from): (&[usize], usize), probing: Option<&KeyHashes>) -> Result<usize> {
        let mut key: Vec<Bucket> = Vec::with_capacity(self.read.keys.len());
        let mut values: Vec<Value> = Vec::with_capacity(self.read.keys.len());
        for (n, &r) in rows.iter().enumerate().skip(from) {
            if self.out.ready() {
                return Ok(n);
            }
            self.bar()?;
            for &(at, c) in &self.read.columns {
                if let Some(slot) = self.scratch.get_mut(c) {
                    *slot = batch.cell(at, r).to_value();
                }
            }
            let hashed = probing.map(|p| (p.hashes.get(n).copied().unwrap_or(0), p.misses.get(n).copied().unwrap_or(true)));
            let keyed = if let Some((_, miss)) = hashed {
                !miss
            } else {
                values.clear();
                values.extend(self.read.keys.iter().map(|k| batch.cell(k.left, r).to_value()));
                keyed(&self.read.keys, &values, &mut key)?
            };
            // Its own condition judged of it as the right has a row, its key
            // null or not [§7.5 GR1c].
            let may = self.right.len() > 0 && self.right.spec().left_may_match(&self.scratch)?;
            let found = may && keyed && self.matches((batch, left, r), (&key, &values), hashed.unwrap_or((0, false)))?;
            // [ISO/IEC 9075:1992 §7.5 GR4, GR5b] A preserved left row no
            // right row matches, with nulls.
            if !found && self.right.spec().kind.keeps_left() {
                self.out.push_held(Some(batch), Some(r), None);
            }
        }
        Ok(rows.len())
    }

    /// Joins row `r` of `batch`, its key columns at `left`, of key buckets
    /// `key` and values `values`, or of the hash `hashed`, with each right
    /// row it matches, flagged; whether one did.
    fn matches(
        &mut self,
        (batch, left, r): (&Batch, &[usize], usize),
        (key, values): (&[Bucket], &[Value]),
        hashed: (u64, bool),
    ) -> Result<bool> {
        let places = self.right.candidates(key, hashed);
        let count = places.as_ref().map_or(self.right.len(), |p| p.len());
        let mut found = false;
        for i in 0..count {
            let n = places.as_ref().map_or(Some(i), |p| p.get(i).copied()).unwrap_or(i);
            let barred = self.barred.as_ref().and_then(|b| b.get(n)).copied().unwrap_or(false);
            if barred || n >= self.right.len() || !self.right.equal(n, (batch, left, r), (&self.read.keys, values))? || !self.holds(n)? {
                continue;
            }
            found = true;
            if let Some(m) = self.matched.get_mut(n) {
                *m = true;
            }
            self.push_right(Some((batch, r)), n);
        }
        Ok(found)
    }

    /// Adds the joined row of left row `left`, or nulls, and right row `n`.
    fn push_right(&mut self, left: Option<(&Batch, usize)>, n: usize) {
        let (batch, r) = left.map_or((None, None), |(b, r)| (Some(b), Some(r)));
        match &self.right {
            Right::Rows(held) => self.out.push(batch, r, held.rows.get(n)),
            Right::Vectors(_) => self.out.push_held(batch, r, Some(n)),
        }
    }

    /// Whether the join's condition, if any, is true of the left row in
    /// the scratch row and right row `n` [ISO/IEC 9075:1992 §7.5 GR1c].
    fn holds(&mut self, n: usize) -> Result<bool> {
        let Probe { right, scratch, .. } = self;
        let right: &Right = right;
        let spec = right.spec();
        let Some(cond) = &spec.cond else { return Ok(true) };
        let width = spec.widths.0;
        match right {
            Right::Rows(held) => {
                for (slot, v) in scratch.iter_mut().skip(width).zip(held.rows.get(n).into_iter().flatten()) {
                    slot.clone_from(v);
                }
            }
            Right::Vectors(v) => {
                for (at, &c) in v.columns.iter().enumerate() {
                    if let Some(slot) = scratch.get_mut(width.saturating_add(c)) {
                        *slot = v.held.batch.cell(at, n).to_value();
                    }
                }
            }
        }
        Ok(eval_cond(cond, scratch, &spec.runner)? == Truth::True)
    }

    /// Judges the right operand's own condition of each of its rows, once,
    /// as the left gives its first row [ISO/IEC 9075:1992 §7.5 GR1c].
    fn bar(&mut self) -> Result<()> {
        let spec = self.right.spec();
        let Some(own) = spec.own.1.as_ref().filter(|_| self.barred.is_none()) else { return Ok(()) };
        let mut barred = Vec::with_capacity(self.right.len());
        for n in 0..self.right.len() {
            let row = self.right.row(n).unwrap_or_default();
            barred.push(eval_cond(own, &row, &spec.runner)? != Truth::True);
        }
        self.barred = Some(barred);
        Ok(())
    }

    /// Gives the right rows from place `from` that matched none, after
    /// nulls, until a batch is made or none is left.
    fn tail(&mut self, from: usize) {
        let mut n = from;
        while n < self.right.len() && !self.out.ready() {
            if !self.matched.get(n).copied().unwrap_or(true) {
                self.push_right(None, n);
            }
            n = n.saturating_add(1);
        }
        if n < self.right.len() {
            self.state = Phase::Tail(n);
        } else {
            self.out.flush(None);
            self.state = Phase::Done;
        }
    }

    /// Probes with the rows of `batch` at `rows` from the one at `from`,
    /// its joined rows put in batches, kept to probe on if a batch is made
    /// before they end; the rows joined before an error, then the error.
    fn batch(&mut self, batch: Batch, rows: Vec<usize>, from: usize) {
        let done = self.probe(&batch, &rows, from);
        if let Ok(probed) = done
            && probed < rows.len()
        {
            self.at = Some((batch, rows, probed));
        } else if self.out.settle(Some(&batch), done.map(|_| ())) {
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
            if let Some((batch, rows, from)) = self.at.take() {
                self.batch(batch, rows, from);
                continue;
            }
            match self.state {
                Phase::Done => return None,
                Phase::Tail(from) => self.tail(from),
                Phase::Probing => match self.left.next() {
                    Some(Ok((batch, rows))) => {
                        let rows = kept_rows(rows, batch.rows);
                        self.batch(batch, rows, 0);
                    }
                    Some(Err(e)) => return Some(Err(e)),
                    None => self.state = if self.right.spec().kind.keeps_right() { Phase::Tail(0) } else { Phase::Done },
                },
            }
        }
    }
}
