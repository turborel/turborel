//! \[DESIGN\] (RULES.md D1156) A semi join or an antijoin that builds its
//! left (`marked.rs`, RULES.md D1070) as the start of a pipeline run by
//! morsels. As the pipeline is made, the left is held and the right read,
//! each of its rows marking the held rows it matches, as the join does
//! before it gives a row. The held rows are then split into morsels, each
//! a range of them in their order, and a morsel gives those of its range
//! marked, or of an antijoin those not, so the operators over the join
//! take them on the part's threads. Every mark is set before the reading
//! of the right ends, which waits for each thread that marks, and a
//! morsel is read only after that, so each held row is given by one
//! morsel alone, once, whichever thread reads it. A held row of a null key
//! is marked by no right row (`marked.rs`), so it is given by an antijoin
//! and dropped by a semi join, as the serial join does.

use std::sync::{Arc, Mutex};

use super::super::super::late::Deferred;
use super::super::super::runtime::Told;
use super::super::super::stream::{self, Batches, Kept};
use super::super::rows_of;
use super::marked::{HeldLeft, Marked, held_marked};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::work::Work;
use crate::physical::SemiJoin;
use crate::storage::{BATCH_ROWS, Batch, RowId, Rows, Split};
use crate::txn::TxId;

/// A split of held rows, and the columns of its batches and the width of
/// their rows.
pub(in crate::exec) type HeldBranch = (Arc<dyn Split>, (Vec<usize>, usize));

/// The held rows of a range given: those marked, or of an antijoin those
/// not, in their order, a batch of them at a time.
pub(super) struct Given {
    held: Arc<HeldLeft>,
    anti: bool,
    /// The held row next looked at.
    at: usize,
    /// The held row past the range.
    end: usize,
    /// The work area the left operand is held in, held while it is, if
    /// this alone holds it.
    _work: Option<Work>,
}

impl Given {
    /// The rows of `held` from `at` to `end` given, of an antijoin if
    /// `anti`, the work area `work` held while they are.
    pub(super) fn of(held: Arc<HeldLeft>, anti: bool, (at, end): (usize, usize), work: Option<Work>) -> Given {
        let end = end.min(held.batch.rows);
        Given { held, anti, at, end, _work: work }
    }
}

impl Iterator for Given {
    type Item = Result<Kept>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut picks = Vec::with_capacity(BATCH_ROWS);
        while self.at < self.end && picks.len() < BATCH_ROWS {
            if self.held.marked(self.at) != self.anti {
                picks.push(Some(self.at));
            }
            self.at = self.at.saturating_add(1);
        }
        if picks.is_empty() {
            return None;
        }
        let columns = self.held.batch.columns.iter().map(|c| c.gathered(&picks)).collect();
        Some(Ok((Batch { columns, rows: picks.len() }, None)))
    }
}

/// The held rows of a semi join that builds its left, marked, split into
/// morsels of `rows` held rows each.
struct HeldSplit {
    held: Arc<HeldLeft>,
    anti: bool,
    rows: usize,
    /// The columns of its batches and the width of their rows.
    layout: (Vec<usize>, usize),
    /// The work area the left is held in, held while a morsel may be read.
    _work: Mutex<Work>,
}

impl HeldSplit {
    /// The range of held rows of morsel `n`.
    fn range(&self, n: usize) -> (usize, usize) {
        let at = n.saturating_mul(self.rows);
        (at, at.saturating_add(self.rows))
    }
}

impl Split for HeldSplit {
    fn morsels(&self) -> usize {
        self.held.batch.rows.div_ceil(self.rows)
    }

    fn morsel(&self, n: usize) -> Rows {
        let (at, end) = self.range(n);
        let given: Batches = Box::new(Given::of(Arc::clone(&self.held), self.anti, (at, end), None));
        let (columns, width) = self.layout.clone();
        // A row's id is its place among the held rows given, which no
        // step reads but to tell rows apart.
        let made = rows_of(given, columns, width).zip(0_u64..).map(|(row, id): (Result<_>, RowId)| row.map(|r| (id, r)));
        Box::new(made)
    }

    fn morsel_batches(&self, n: usize, _late: &[usize]) -> Option<crate::storage::Batches> {
        // A batch given holds the rows kept alone, so none is selected.
        let given = Given::of(Arc::clone(&self.held), self.anti, self.range(n), None);
        Some(Box::new(given.map(|kept| kept.map(|(batch, _)| batch))))
    }
}

/// The held rows of the semi join `s`, one that builds its left, its left
/// told `told`, held and marked for `tx`'s statement and split into
/// morsels of `rows` held rows, and the columns of their batches and the
/// width of their rows; none if the left gives way to the work area.
///
/// # Errors
///
/// As `held_marked`.
pub(in crate::exec) fn held_split(s: &SemiJoin, told: &[Told], (db, tx): (&Db, TxId), rows: usize) -> Result<Option<HeldBranch>> {
    let Some(layout) = stream::layout(&s.left, db) else { return Ok(None) };
    let deferred = Deferred::default().with_told(told.to_vec());
    let Marked::Held(held, anti, work) = held_marked(s.clone(), (&layout.0, &deferred), (db, tx))? else { return Ok(None) };
    let split = HeldSplit { held, anti, rows: rows.max(1), layout: layout.clone(), _work: Mutex::new(work) };
    Ok(Some((Arc::new(split), layout)))
}
