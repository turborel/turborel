//! \[DESIGN\] Mark joins (`physical/mark.rs`, RULES.md D642): each row of
//! the left operand once, in its order, with its mark, the OR of the truth
//! values the right rows that count for it give, false of none
//! [ISO/IEC 9075:1992 §8.12 GR2]. A right row counts if its keys equal the
//! left row's, or of a key on which a null matches a null are not distinct
//! [ISO/IEC 9075-2:2023 §8.15], and the rest of the condition is true of
//! the pair; it gives true of EXISTS, else the comparison's truth value.
//!
//! A comparison that is an AND of equalities of a column of each, as of
//! IN, is true where every pair of values is equal, false where one pair
//! of values neither null differs, else unknown [ISO/IEC 9075:1992 §8.2
//! GR7a, b, g, m]. So the right's rows are held by the buckets of their keys and of
//! those values when none is null, and by their keys alone when one is,
//! the null list of Neumann, Leis and Kemper (BTW 2017, §5.6): a left row
//! of no null value reads its own bucket, where a row gives true, and its
//! keys' null list, whose rows give unknown or false; a left row of a null
//! value reads every right row of its keys (`mark/side.rs`).
//!
//! The right is read once the first left row is, so an empty left reads
//! none, and the first right row to give true decides.

mod side;

use std::sync::Arc;

use super::super::work::Work;
use super::super::{RowIter, Runner, broken, column, eval_cond, execute, failed};
use super::equal;
use crate::db::State;
use crate::error::Result;
use crate::logical::{Cond, Truth};
use crate::physical::{JoinKey, MarkJoin, Plan};
use crate::storage::Row;
use crate::txn::{TxId, checked};
use crate::value::key_equal;
pub(in crate::exec) use side::Side;

/// The rows of the mark join `plan`. Never inlined into `execute`, whose
/// frame every level of a plan adds to the stack (RULES.md R-CF-6).
#[inline(never)]
pub(in crate::exec) fn mark(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::MarkJoin(m) = plan else { return failed(broken("a mark join is not one")) };
    let MarkJoin { left, right, keys, cond, marks, compared, .. } = *m;
    let marker = Marker { keys, cond, marks, compared, runner: Runner::new(db, tx) };
    let rows = MarkRows { left: execute(left, db, tx), right: Some(execute(right, db, tx)), side: None, marker, work: Work::of(db, tx) };
    Box::new(checked(rows, db.tx.stop_of(tx)))
}

/// What a mark join judges a pair by: its keys and the rest of its
/// condition, and its comparison, as marks or a condition.
pub(in crate::exec) struct Marker {
    pub(in crate::exec) keys: Vec<JoinKey>,
    pub(in crate::exec) cond: Option<Cond>,
    pub(in crate::exec) marks: Vec<JoinKey>,
    pub(in crate::exec) compared: Option<Cond>,
    pub(in crate::exec) runner: Runner,
}

impl Marker {
    /// The truth value right row `r` gives left row `l`: false if it does
    /// not count.
    ///
    /// # Errors
    ///
    /// The conditions'; a column a row lacks.
    pub(in crate::exec) fn truth(&self, l: &Row, r: &Row) -> Result<Truth> {
        if !equal(&self.keys, l, r)? {
            return Ok(Truth::False);
        }
        let mut pair: Option<Row> = None;
        if let Some(c) = &self.cond {
            let both = pair.get_or_insert_with(|| l.iter().chain(r.iter()).cloned().collect());
            if eval_cond(c, both, &self.runner)? != Truth::True {
                return Ok(Truth::False);
            }
        }
        if !self.marks.is_empty() {
            return marks_truth(&self.marks, l, r);
        }
        match &self.compared {
            None => Ok(Truth::True),
            Some(c) => eval_cond(c, pair.get_or_insert_with(|| l.iter().chain(r.iter()).cloned().collect()), &self.runner),
        }
    }

    /// Whether, of no condition beyond the keys, right rows `a` and `b`
    /// give every left row the same truth value: their keys and marks
    /// alike, so the second need not be held.
    pub(in crate::exec) fn same(&self, a: &Row, b: &Row) -> Result<bool> {
        if self.cond.is_some() || (self.compared.is_some() && self.marks.is_empty()) {
            return Ok(false);
        }
        for k in self.keys.iter().chain(&self.marks) {
            if !key_equal(column(a, k.right)?, column(b, k.right)?, k.nulls)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// [ISO/IEC 9075:1992 §8.2 GR7a, b, g, m] The AND of the equalities `marks` of the
/// values of `l` and `r`: false if a pair of values neither null differs,
/// else unknown if a value is null, else true.
fn marks_truth(marks: &[JoinKey], l: &Row, r: &Row) -> Result<Truth> {
    let mut unknown = false;
    for k in marks {
        let (a, b) = (column(l, k.left)?, column(r, k.right)?);
        if a.is_null() || b.is_null() {
            unknown = true;
        } else if !key_equal(a, b, false)? {
            return Ok(Truth::False);
        }
    }
    Ok(if unknown { Truth::Unknown } else { Truth::True })
}

/// [ISO/IEC 9075:1992 §8.12 GR2] The OR of `a` and `b`.
pub(in crate::exec) fn or(a: Truth, b: Truth) -> Truth {
    if a == Truth::True || b == Truth::True {
        Truth::True
    } else if a == Truth::Unknown || b == Truth::Unknown {
        Truth::Unknown
    } else {
        Truth::False
    }
}

/// A mark join's rows: each left row with its mark, the right held once
/// the first left row is read.
struct MarkRows {
    left: RowIter,
    right: Option<RowIter>,
    side: Option<Side>,
    marker: Marker,
    work: Work,
}

impl MarkRows {
    /// The next left row with its mark, if there is one.
    fn step(&mut self) -> Result<Option<Row>> {
        let Some(mut l) = self.left.next().transpose()? else { return Ok(None) };
        if self.side.is_none() {
            let rows = self.right.take().ok_or_else(|| broken("a mark join's right rows were read twice"))?;
            self.side = Some(Side::of(rows, &self.marker, &mut self.work)?);
        }
        let side = self.side.as_mut().ok_or_else(|| broken("a mark join holds no right rows"))?;
        let mark = side.mark(&l, &self.marker)?;
        l.push(mark.value());
        Ok(Some(l))
    }
}

impl Iterator for MarkRows {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        self.step().transpose()
    }
}
