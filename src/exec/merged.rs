//! \[EXTENSION\] The rows of several inputs, each in the order of its
//! keys, merged in that order (RULES.md D156): a row of each input held,
//! the least given and the next of its input read in its place. A limit
//! over it reads no more of each input than the rows it gives need.

use std::cmp::Ordering;

use super::sort::compare_rows;
use super::{RowIter, TxId, broken, execute, failed};
use crate::error::Result;
use crate::exec::Db;
use crate::logical::SortKey;
use crate::physical::Plan;
use crate::storage::Row;

/// The rows of the merge `plan`.
#[inline(never)]
pub(super) fn merged(plan: Plan, db: &Db, tx: TxId) -> RowIter {
    let Plan::MergeAppend { inputs, keys, .. } = plan else { return failed(broken("a merge is not one")) };
    let inputs: Vec<RowIter> = inputs.into_iter().map(|p| execute(p, db, tx)).collect();
    Box::new(Merged { keys, heads: Vec::new(), inputs, started: false })
}

/// The inputs, and the row each holds next, if any.
struct Merged {
    keys: Vec<SortKey>,
    heads: Vec<Option<Row>>,
    inputs: Vec<RowIter>,
    started: bool,
}

impl Merged {
    /// The next row of input `at`, held at it.
    fn advance(&mut self, at: usize) -> Result<()> {
        let next = self.inputs.get_mut(at).and_then(Iterator::next).transpose()?;
        if let Some(head) = self.heads.get_mut(at) {
            *head = next;
        }
        Ok(())
    }

    /// The input holding the least row, the first of equals.
    fn least(&self) -> Option<usize> {
        let mut best: Option<(usize, &Row)> = None;
        for (at, head) in self.heads.iter().enumerate() {
            let Some(row) = head else { continue };
            if best.is_none_or(|(_, b)| compare_rows(&self.keys, row, b) == Ordering::Less) {
                best = Some((at, row));
            }
        }
        best.map(|(at, _)| at)
    }
}

impl Iterator for Merged {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        if !self.started {
            self.started = true;
            self.heads = vec![None; self.inputs.len()];
            for at in 0..self.inputs.len() {
                if let Err(e) = self.advance(at) {
                    return Some(Err(e));
                }
            }
        }
        let at = self.least()?;
        let row = self.heads.get_mut(at).and_then(Option::take)?;
        match self.advance(at) {
            Ok(()) => Some(Ok(row)),
            Err(e) => Some(Err(e)),
        }
    }
}
