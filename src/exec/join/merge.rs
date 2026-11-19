//! \[DESIGN\] The merge join (`physical/enumerate/methods.rs`, RULES.md
//! D303): the rows of a product its equality of a column of each operand
//! keeps [X3.135-1986 §5.20 GR2b, §5.21 GR2], or of a LEFT outer join
//! [ISO/IEC 9075:1992 §7.5], of operands each in ascending order of its
//! key. Each left row is paired with the run of right rows whose key
//! equals its, a run read once and held while left rows of its key come,
//! in memory until it outgrows the work area, then in a run of its spill
//! area (`exec/join/rescan.rs`, RULES.md D38); the rest of the condition
//! is judged of each pair, and of a LEFT join a left row none is kept of
//! is kept with nulls. A null key matches nothing [X3.135-1986 §5.11 GR2]
//! wherever its operand's order puts it. The planner merges only keys
//! whose order is the order their equality compares by, so equal keys are
//! adjacent (`physical/enumerate/modeled.rs`). The left's order is the
//! result's.

use std::cmp::Ordering;
use std::iter::Peekable;
use std::sync::Arc;

use super::super::work::Work;
use super::super::{RowIter, Runner, broken, column, eval_cond, execute, failed};
use super::rescan::{Pass, Rescanned};
use crate::db::State;
use crate::error::Result;
use crate::logical::{Cond, SortKey, Truth};
use crate::physical::{JoinKey, Plan};
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::{SortOrder, Value, key_equal};

/// The rows of the merge join `plan`. Never inlined into `execute`, whose
/// frame every level of a plan adds to the stack (RULES.md R-CF-6).
#[inline(never)]
pub(in crate::exec) fn merged(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::MergeJoin { outer, left, right, key, cond, widths, .. } = plan else { return failed(broken("a merge join is not one")) };
    Box::new(MergeJoin {
        left: execute(*left, db, tx),
        right: execute(*right, db, tx).peekable(),
        key,
        cond,
        nulls: outer.then(|| vec![Value::Null; widths.1]),
        runner: Runner::new(db, tx),
        work: Work::of(db, tx),
        run: None,
        current: None,
    })
}

/// A merge join being read.
struct MergeJoin {
    left: RowIter,
    right: Peekable<RowIter>,
    key: JoinKey,
    cond: Option<Cond>,
    /// Of a LEFT join, the nulls a left row matching none is kept with.
    nulls: Option<Vec<Value>>,
    runner: Runner,
    work: Work,
    /// The run of right rows of the last key met, and that key.
    run: Option<(Value, Rescanned)>,
    /// The left row being paired, the pass of its run, and whether a pair
    /// of it was kept.
    current: Option<(Row, Pass, bool)>,
}

/// The order both operands are in: ascending, nulls placed as implied.
fn order() -> SortOrder {
    SortKey::ascending(0).order()
}

impl MergeJoin {
    /// The next row of the join, if any.
    fn step(&mut self) -> Result<Option<Row>> {
        loop {
            if let Some(row) = self.paired()? {
                return Ok(Some(row));
            }
            if let Some((left, _, matched)) = self.current.take() {
                if let (false, Some(nulls)) = (matched, &self.nulls) {
                    return Ok(Some(left.into_iter().chain(nulls.iter().cloned()).collect()));
                }
                continue;
            }
            let Some(left) = self.left.next().transpose()? else { return Ok(None) };
            let value = column(&left, self.key.left)?.clone();
            let pass = if value.is_null() { None } else { self.run_of(&value)?.map(Rescanned::pass).transpose()? };
            match pass {
                Some(pass) => self.current = Some((left, pass, false)),
                None => self.current = Some((left, Box::new(std::iter::empty()), false)),
            }
        }
    }

    /// The next pair of the left row being paired its condition keeps.
    fn paired(&mut self) -> Result<Option<Row>> {
        let Some((left, pass, matched)) = &mut self.current else { return Ok(None) };
        for r in pass.by_ref() {
            let pair: Row = left.iter().cloned().chain(r?).collect();
            let kept = match &self.cond {
                Some(c) => eval_cond(c, &pair, &self.runner)? == Truth::True,
                None => true,
            };
            if kept {
                *matched = true;
                return Ok(Some(pair));
            }
        }
        Ok(None)
    }

    /// The run of right rows whose key equals `value`, read past the rows
    /// of lesser or null keys, if there is one.
    fn run_of(&mut self, value: &Value) -> Result<Option<&mut Rescanned>> {
        let same = match &self.run {
            Some((key, _)) => key_equal(key, value, false)?,
            None => false,
        };
        if !same {
            self.run = None;
            self.skip_below(value)?;
            if self.head_equals(value)? {
                let (right, work, at) = (&mut self.right, &mut self.work, self.key.right);
                let taken = std::iter::from_fn(|| {
                    let head = right.peek()?;
                    let equal = match head {
                        Ok(row) => column(row, at).and_then(|v| key_equal(v, value, false)),
                        Err(_) => Ok(true),
                    };
                    match equal {
                        Ok(true) => right.next(),
                        Ok(false) => None,
                        Err(e) => Some(Err(e)),
                    }
                });
                let (rows, _) = Rescanned::of(taken, work)?;
                self.run = Some((value.clone(), rows));
            }
        }
        Ok(self.run.as_mut().map(|(_, rows)| rows))
    }

    /// Reads past the right rows whose key is null or less than `value`.
    fn skip_below(&mut self, value: &Value) -> Result<()> {
        while let Some(head) = self.right.peek() {
            let below = match head {
                Ok(row) => {
                    let key = column(row, self.key.right)?;
                    key.is_null() || order().cmp(key, value) == Ordering::Less
                }
                Err(_) => return self.right.next().map_or(Ok(()), |r| r.map(|_| ())),
            };
            if !below {
                break;
            }
            self.right.next();
        }
        Ok(())
    }

    /// Whether the next right row's key equals `value`.
    fn head_equals(&mut self, value: &Value) -> Result<bool> {
        match self.right.peek() {
            Some(Ok(row)) => key_equal(column(row, self.key.right)?, value, false),
            Some(Err(_)) => self.right.next().map_or(Ok(false), |r| r.map(|_| false)),
            None => Ok(false),
        }
    }
}

impl Iterator for MergeJoin {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        self.step().transpose()
    }
}
