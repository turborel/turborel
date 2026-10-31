//! \[DESIGN\] The product of two streams [X3.135-1986 §5.20 GR2b] by
//! batches (RULES.md D31): its right operand read first and held as the row
//! path holds it (`exec/join/rescan.rs`), in memory while it fits the work
//! area, else in a run of the spill area; each of the left operand's
//! batches paired with it, its rows joined with every right row, a batch of
//! the pairs made at a time. Held in memory, each left row is paired with
//! the right's rows in turn, as the row path pairs them. In a run, the run
//! is read once for each left batch, each right row paired with every row
//! of the batch, where no order of the left's is asked for (`in_order`);
//! else once for each left row.

use std::sync::Arc;

use super::super::stream::{self, Batches, Kept};
use super::pairs::{Out, kept_rows};
use super::{in_order, left_batches, operand_rows};
use crate::db::State;
use crate::error::Result;
use crate::exec::join::Pass;
use crate::exec::join::Rescanned;
use crate::exec::work::Work;
use crate::physical::Plan;
use crate::storage::{Batch, Row};
use crate::txn::TxId;

/// The batches of the product of `left` and `right`.
///
/// # Errors
///
/// The right operand's; the spill area's.
pub(super) fn multiplied((left, right): (Plan, Plan), db: &Arc<State>, tx: TxId) -> Result<Batches> {
    let (left_columns, _) = stream::layout(&left, db).unwrap_or_default();
    let (right_columns, _) = stream::layout(&right, db).unwrap_or_default();
    let mut work = Work::of(db, tx);
    let (right, _) = Rescanned::of(operand_rows(right, db, tx), &mut work)?;
    let by_batch = matches!(right, Rescanned::Spilled(..)) && !in_order(&left);
    let out = Out::new(left_columns.len(), right_columns);
    let left = left_batches(left, db, tx)?;
    Ok(Box::new(Product { left, right, by_batch, current: None, out, ended: false, _work: work }))
}

/// A product's batches, as they are pulled.
struct Product {
    left: Batches,
    right: Rescanned,
    /// Whether a right operand in a run is read once for each left batch.
    by_batch: bool,
    current: Option<Current>,
    out: Out,
    /// Whether it is done, or an error ended it.
    ended: bool,
    /// The work area the right operand is held in, held while it is.
    _work: Work,
}

/// A left batch being paired: its rows kept, the one being paired, and
/// where in the right operand it is.
struct Current {
    batch: Batch,
    rows: Vec<usize>,
    at: usize,
    place: Place,
}

/// Where in the right operand a left batch's pairing is: the rows held in
/// memory and the next of them, or the pass of a run.
enum Place {
    Held(Arc<Vec<Row>>, usize),
    Pass(Pass),
}

impl Product {
    /// Begins pairing `batch`, of kept rows `rows`.
    fn begin(&mut self, batch: Batch, rows: Vec<usize>) -> Result<()> {
        let place = match &mut self.right {
            Rescanned::Held(held) => Place::Held(Arc::clone(held), 0),
            Rescanned::Spilled(..) => Place::Pass(self.right.pass()?),
        };
        self.current = Some(Current { batch, rows, at: 0, place });
        Ok(())
    }

    /// Takes up the next left batch, if it has a row kept; ends once the
    /// left is read.
    fn take(&mut self) -> Result<()> {
        let Some(kept) = self.left.next() else {
            self.ended = true;
            return Ok(());
        };
        let (batch, rows) = kept?;
        let rows = kept_rows(rows, batch.rows);
        if rows.is_empty() {
            return Ok(());
        }
        self.begin(batch, rows)
    }

    /// Pairs the left batch being paired until a batch is made or it is
    /// done, then readies the rest.
    fn fill(&mut self) -> Result<()> {
        let Some(current) = &mut self.current else { return Ok(()) };
        let left = (&current.batch, current.rows.as_slice(), &mut current.at);
        let done = match &mut current.place {
            Place::Held(rows, next) => held(left, next, rows, &mut self.out),
            Place::Pass(pass) => passed(left, (pass, &mut self.right), self.by_batch, &mut self.out)?,
        };
        if done {
            self.out.flush(Some(&current.batch));
            self.current = None;
        }
        Ok(())
    }
}

/// A left batch being paired: the batch, its rows kept, and the place
/// among them of the one being paired.
type Left<'c> = (&'c Batch, &'c [usize], &'c mut usize);

/// Pairs the rows of a left batch, from the one being paired, with the
/// right's held `rows`, from the one at `next`, until a batch is made;
/// whether all are.
fn held((batch, rows, at): Left<'_>, next: &mut usize, right: &[Row], out: &mut Out) -> bool {
    while !out.ready() {
        let Some(&l) = rows.get(*at) else { return true };
        if let Some(r) = right.get(*next) {
            out.push(Some(batch), Some(l), Some(r));
            *next = next.saturating_add(1);
        } else {
            *at = at.saturating_add(1);
            *next = 0;
        }
    }
    false
}

/// Pairs the rows of a left batch with those of `pass`, of the right in a
/// run: each right row with every row of the batch if `by_batch`; else each
/// left row with the whole run, `right` read again for the next; until a
/// batch is made. Whether all are.
fn passed((batch, rows, at): Left<'_>, (pass, right): (&mut Pass, &mut Rescanned), by_batch: bool, out: &mut Out) -> Result<bool> {
    while !out.ready() {
        if by_batch {
            let Some(r) = pass.next().transpose()? else { return Ok(true) };
            for &l in rows {
                out.push(Some(batch), Some(l), Some(&r));
            }
            continue;
        }
        let Some(&l) = rows.get(*at) else { return Ok(true) };
        if let Some(r) = pass.next().transpose()? {
            out.push(Some(batch), Some(l), Some(&r));
        } else {
            *at = at.saturating_add(1);
            if *at < rows.len() {
                *pass = right.pass()?;
            }
        }
    }
    Ok(false)
}

impl Iterator for Product {
    type Item = Result<Kept>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(given) = self.out.next() {
                return Some(given);
            }
            if self.ended {
                return None;
            }
            if self.current.is_none() {
                if let Err(e) = self.take() {
                    self.ended = true;
                    return Some(Err(e));
                }
                continue;
            }
            if let Err(e) = self.fill() {
                let batch = self.current.take().map(|c| c.batch);
                self.ended = self.out.settle(batch.as_ref(), Err(e));
            }
        }
    }
}
