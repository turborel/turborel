//! \[DESIGN\] A range join's right operand past the work area (RULES.md
//! D38, D580): written whole to a run of the spill area, then read back
//! once, a part at a time, each part as many rows as the work area holds,
//! sorted and searched as a right operand that fits is. The left operand
//! is gathered too, in memory or in a run, and read again for each part.
//! Of a LEFT join, whether each left row matched in some part is kept, a
//! flag for each, and once the last part is searched the left is read a
//! last time for the rows no part matched.

use super::super::super::work::Work;
use super::super::rescan::{Pass, Rescanned};
use super::SORTED_BYTES;
use crate::error::Result;
use crate::exec::RowIter;
use crate::spill::row_bytes;
use crate::storage::Row;

/// The right operand's parts still to search, and the left read for each.
pub(super) struct Parts {
    /// The right's run, kept while it is read.
    _right: Rescanned,
    right: Pass,
    /// The first row of the next part, read past the part before.
    next: Option<Row>,
    left: Rescanned,
    /// Of a LEFT join, whether each left row has matched in a part.
    matched: Option<Vec<bool>>,
    /// The left row being read, from 0, of the pass being read.
    at: usize,
    /// Whether the left is being read a last time, for its rows no part
    /// matched.
    pub(super) finishing: bool,
}

impl Parts {
    /// The parts of the rows `held` and then `rest`, and `left`, of a LEFT
    /// join if `outer`, gathered in `work`; and the first pass of the left.
    ///
    /// # Errors
    ///
    /// The operands', and the spill area's.
    pub(super) fn of((held, rest): (Vec<Row>, RowIter), left: RowIter, outer: bool, work: &mut Work) -> Result<(Parts, Pass)> {
        let (mut right, _) = Rescanned::of(held.into_iter().map(Ok).chain(rest), work)?;
        let (mut lefts, count) = Rescanned::of(left, work)?;
        let (first, reading) = (lefts.pass()?, right.pass()?);
        let matched = outer.then(|| vec![false; count]);
        Ok((Parts { _right: right, right: reading, next: None, left: lefts, matched, at: 0, finishing: false }, first))
    }

    /// The rows of the next part, as many as `work` holds but at least one;
    /// none once every part is read.
    ///
    /// # Errors
    ///
    /// The spill area's, reading the run.
    pub(super) fn part(&mut self, work: &Work) -> Result<Option<Vec<Row>>> {
        let (mut rows, mut bytes) = (Vec::new(), 0_usize);
        if let Some(row) = self.next.take() {
            bytes = row_bytes(&row).saturating_add(SORTED_BYTES);
            rows.push(row);
        }
        while let Some(row) = self.right.next().transpose()? {
            let more = row_bytes(&row).saturating_add(SORTED_BYTES);
            if !rows.is_empty() && bytes.saturating_add(more) > work.limit() {
                self.next = Some(row);
                break;
            }
            bytes = bytes.saturating_add(more);
            rows.push(row);
        }
        Ok((!rows.is_empty()).then_some(rows))
    }

    /// The left read again, from its first row.
    ///
    /// # Errors
    ///
    /// The spill area's, reading the run.
    pub(super) fn again(&mut self) -> Result<Pass> {
        self.at = 0;
        self.left.pass()
    }

    /// Notes of the left row being read whether it `matched`, and moves to
    /// the next.
    pub(super) fn read(&mut self, matched: bool) {
        if let Some(flag) = self.matched.as_mut().and_then(|m| m.get_mut(self.at)) {
            *flag |= matched;
        }
        self.at = self.at.saturating_add(1);
    }

    /// Whether the left row being read matched in no part, of a LEFT join.
    pub(super) fn unmatched(&self) -> bool {
        self.matched.as_ref().and_then(|m| m.get(self.at)) == Some(&false)
    }
}
