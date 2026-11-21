//! [ISO/IEC 9075-2:2023 §7.7, §9.41] Row pattern recognition run. The rows
//! of the input come in order of their row pattern partitioning and
//! ordering (the planner sorts them) [GR1 to GR3], and are read one
//! partition at a time. Each partition's rows are held, up to the work
//! area and spilled past it (RULES.md D38), read by place as a definition
//! or a measure navigates to them; its retained matches are then found and
//! their rows given as they are asked for (`pattern/matches.rs`), before
//! the next partition is read. What a definition or a measure reads of a
//! match is in `pattern/scene.rs`, shared with a window's row pattern
//! recognition (`window/matched.rs`).
//!
//! Each row is taken only while the statement is not cancelled (RULES.md
//! D44).

mod aggregate;
mod matches;
mod measures;
pub(super) mod scene;

use std::cmp::Ordering;

use self::matches::Part;
use self::scene::Reading;
use super::work::Work;
use super::{HeldRows, RowIter, Runner, broken, column, execute, failed};
use crate::error::Result;
use crate::exec::Db;
use crate::logical::{Recognize, SortKey};
use crate::pattern::Program;
use crate::physical::Plan;
use crate::spill::ListSpace;
use crate::storage::Row;
use crate::txn::{TxId, checked};
use crate::value::Value;

/// The rows of the row pattern recognition `plan`.
pub(super) fn recognized(plan: Plan, db: &Db, tx: TxId) -> RowIter {
    let Plan::Recognize { recognize, .. } = plan else { return failed(broken("a row pattern recognition is not one")) };
    let (r, input) = recognize.split();
    let rows = execute(input, db, tx);
    let work = Work::of(db, tx);
    let space = ListSpace { limit: work.limit(), ..db.list_space() };
    match Recognizer::new(r, rows, (Runner::new(db, tx), work), space) {
        Ok(given) => Box::new(checked(given, db.tx.stop_of(tx))),
        Err(e) => failed(e),
    }
}

/// Row pattern recognition over rows in order, a partition at a time.
struct Recognizer {
    r: Recognize<()>,
    program: Program,
    rows: RowIter,
    runner: Runner,
    work: Work,
    /// The space a partition's rows, and a set function's values, are held
    /// in.
    space: ListSpace,
    /// The first row of the next partition, once read.
    next: Option<Row>,
    /// The partition whose rows are being given.
    part: Option<Part>,
    /// Whether the input is read to its end, or failed.
    done: bool,
}

impl Recognizer {
    /// `r` over `rows`, its subqueries run by `runner`, a DISTINCT set
    /// function's values kept in `work`, rows held in `space`.
    ///
    /// # Errors
    ///
    /// \[DESIGN\] A pattern beyond the matcher's limits.
    fn new(r: Recognize<()>, rows: RowIter, (runner, work): (Runner, Work), space: ListSpace) -> Result<Recognizer> {
        let program = Program::new(&r.matching.pattern)?;
        Ok(Recognizer { r, program, rows, runner, work, space, next: None, part: None, done: false })
    }

    /// The next row of the partition being given, none past its last.
    fn step(&mut self) -> Result<Option<Row>> {
        let Recognizer { r, program, runner, work, space, part, .. } = self;
        let Some(part) = part.as_mut() else { return Ok(None) };
        let reading = Reading { matching: &r.matching, runner, work, space };
        part.next(r, program, &reading)
    }

    /// [GR1, GR2] The rows of the next partition, none past the last; the
    /// first row of the partition after it kept.
    fn read(&mut self) -> Result<Option<Part>> {
        let first = if let Some(row) = self.next.take() {
            row
        } else {
            let read = self.rows.next();
            let Some(row) = read else { return Ok(None) };
            row?
        };
        let parted = self.r.partition.iter().map(|&c| column(&first, c).cloned()).collect::<Result<Vec<_>>>()?;
        let mut held = HeldRows::new(self.space.clone());
        held.push(first)?;
        for row in self.rows.by_ref() {
            let row = row?;
            let same = alike(&self.r.partition, &parted, &row)?;
            if !same {
                self.next = Some(row);
                break;
            }
            held.push(row)?;
        }
        Ok(Some(Part::new(held)))
    }
}

impl Iterator for Recognizer {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        loop {
            if self.part.is_some() {
                match self.step() {
                    Ok(Some(row)) => return Some(Ok(row)),
                    Ok(None) => self.part = None,
                    Err(e) => {
                        (self.part, self.done) = (None, true);
                        return Some(Err(e));
                    }
                }
            }
            if self.done {
                return None;
            }
            match self.read() {
                Ok(Some(part)) => self.part = Some(part),
                Ok(None) => self.done = true,
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
    }
}

/// [ISO/IEC 9075-2:2023 §7.7 GR1] Whether `row`'s values of the columns
/// `partition` are not distinct from `values`, so that it is of the same
/// partition.
///
/// # Errors
///
/// \[DESIGN\] A partitioning column past the row's last.
fn alike(partition: &[usize], values: &[Value], row: &Row) -> Result<bool> {
    for (&c, v) in partition.iter().zip(values) {
        let x = column(row, c)?;
        if SortKey::ascending(c).order().cmp(v, x) != Ordering::Equal {
            return Ok(false);
        }
    }
    Ok(true)
}

// Unit tests: matches, skips, rows per match and the values read of them
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
