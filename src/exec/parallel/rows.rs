//! \[DESIGN\] (RULES.md D1150) A morsel of a pipeline read row by row, its
//! rows taken through the steps one at a time as they are asked for, as
//! Leis et al., "Morsel-Driven Parallelism" (2014), push a morsel's tuples
//! through its pipeline fragment. A probe's matches of a row are held as
//! the numbers of their built rows, each joined with the row as it is
//! read, and a row waits at its probe while its matches go on, so the
//! morsel holds one row of each probe and its matches' numbers, however
//! many rows the joins make of it. Before, a morsel's rows were gathered
//! whole before the first was read, which, of a join whose keys each
//! match thousands of rows, was millions of rows outside any grant.
//!
//! The steps are those of the plan, applied as its operators apply them,
//! so a morsel gives the rows, in the order, and raises the error, the
//! plan's operators would of those of its table. A counted step's rows
//! are added once the morsel is read to its end (RULES.md D378).

use std::sync::Arc;

use super::pipeline::{Pipeline, SemiOf, Step};
use super::probe::Table;
use crate::error::Result;
use crate::exec::{RowIter, Runner, broken, eval, eval_cond};
use crate::logical::Truth;
use crate::storage::{Row, Rows};

/// A row waiting at a probe: the probe's table and place among the steps,
/// the row, and the numbers of the built rows it matches not yet joined.
type Waiting = (Arc<Table>, usize, Row, std::vec::IntoIter<usize>);

/// The rows of a morsel, taken through its branch's steps as they are
/// asked for.
struct MorselRows {
    pipeline: Arc<Pipeline>,
    branch: usize,
    read: Rows,
    runner: Runner,
    waiting: Vec<Waiting>,
    /// Of a branch of a counted step, the rows that reach each step.
    tally: Option<Vec<u64>>,
    ended: bool,
}

/// The rows of morsel `n` of `pipeline`, read row by row, in order, its
/// expressions evaluated by `runner`, each made as it is asked for.
///
/// # Errors
///
/// SQLCODE -910 if `n` is past its last morsel.
pub(super) fn morsel_rows(pipeline: &Arc<Pipeline>, n: usize, runner: Runner) -> Result<RowIter> {
    let mut at = n;
    for (branch, of) in pipeline.branches.iter().enumerate() {
        let morsels = of.split.morsels();
        if at < morsels {
            let tally = of.steps.iter().any(|s| matches!(s, Step::Count(_))).then(|| vec![0_u64; of.steps.len()]);
            let read = of.split.morsel(at);
            let pipeline = Arc::clone(pipeline);
            return Ok(Box::new(MorselRows { pipeline, branch, read, runner, waiting: Vec::new(), tally, ended: false }));
        }
        at = at.saturating_sub(morsels);
    }
    Err(broken("a morsel past a pipeline's last"))
}

impl MorselRows {
    /// Takes `row` through the steps from the one at `at`: the row it
    /// gives past the last, if one; of a probe its row matches, none, the
    /// row left waiting at it.
    fn through(&mut self, mut at: usize, mut row: Row) -> Result<Option<Row>> {
        let pipeline = Arc::clone(&self.pipeline);
        let steps = pipeline.branches.get(self.branch).map_or(&[][..], |b| b.steps.as_slice());
        while let Some(step) = steps.get(at) {
            match step {
                Step::Filter(cond) => {
                    if eval_cond(cond, &row, &self.runner)? != Truth::True {
                        return Ok(None);
                    }
                }
                Step::Project(exprs) => row = exprs.iter().map(|e| eval(e, &row, &self.runner)).collect::<Result<Row>>()?,
                Step::Probe(table) => {
                    let matched = table.shared.matching(table.keys(), &row)?;
                    if !matched.is_empty() {
                        self.waiting.push((Arc::clone(table), at, row, matched.into_iter()));
                    }
                    return Ok(None);
                }
                Step::Outer(table) => {
                    let matched = table.shared.matching(table.keys(), &row)?;
                    if !matched.is_empty() {
                        self.waiting.push((Arc::clone(table), at, row, matched.into_iter()));
                        return Ok(None);
                    }
                    row = unmatched(table, row);
                }
                Step::Semi(table, semi) => {
                    if found(table, semi, &row, &self.runner)? == semi.anti {
                        return Ok(None);
                    }
                }
                Step::Count(_) => {
                    if let Some(seen) = self.tally.as_mut().and_then(|t| t.get_mut(at)) {
                        *seen = seen.saturating_add(1);
                    }
                }
                Step::Runtime(..) => {}
            }
            at = at.saturating_add(1);
        }
        Ok(Some(row))
    }

    /// The next row to take through the steps, and the place of the step
    /// it starts at: the next match of the row waiting last, joined, or
    /// else the morsel's next row; none past its last.
    fn taken(&mut self) -> Option<Result<(usize, Row)>> {
        while let Some((table, at, row, matched)) = self.waiting.last_mut() {
            if let Some(p) = matched.next() {
                let mut joined = row.clone();
                joined.extend(table.shared.row_of(p));
                return Some(Ok((at.saturating_add(1), joined)));
            }
            self.waiting.pop();
        }
        self.read.next().map(|read| read.map(|(_, row)| (0, row)))
    }

    /// Adds the rows its counted steps tallied to their counts, once.
    fn counted(&mut self) {
        let (Some(tally), Some(branch)) = (self.tally.take(), self.pipeline.branches.get(self.branch)) else { return };
        for (step, &rows) in branch.steps.iter().zip(&tally) {
            if let Step::Count(count) = step {
                count.stepped(rows);
            }
        }
    }
}

impl Iterator for MorselRows {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        while !self.ended {
            let made = match self.taken() {
                Some(Ok((at, row))) => self.through(at, row),
                Some(Err(e)) => Err(e),
                None => {
                    self.ended = true;
                    self.counted();
                    return None;
                }
            };
            match made {
                Ok(Some(row)) => return Some(Ok(row)),
                Ok(None) => {}
                Err(e) => {
                    self.ended = true;
                    return Some(Err(e));
                }
            }
        }
        None
    }
}

/// `row`, a left row no row of `table`, a LEFT join's built operand,
/// matches, with nulls for the operand's columns [ISO/IEC 9075:1992 §7.5
/// GR5b].
fn unmatched(table: &Table, mut row: Row) -> Row {
    row.resize(row.len().saturating_add(table.shared.width()), crate::value::Value::Null);
    row
}

/// Whether a row of `table`, a semi join's built operand, matches `row`
/// as `semi` asks: its keys, and the rest of the condition true of the
/// pair [X3.135-1986 §5.17 GR2].
fn found(table: &Table, semi: &SemiOf, row: &Row, runner: &Runner) -> Result<bool> {
    let mut any = false;
    table.each_match(row, |r| {
        if !any {
            any = match &semi.cond {
                Some(cond) => eval_cond(cond, &row.iter().chain(r.iter()).cloned().collect::<Row>(), runner)? == Truth::True,
                None => true,
            };
        }
        Ok(())
    })?;
    Ok(any)
}
