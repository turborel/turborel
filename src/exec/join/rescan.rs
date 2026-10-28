//! \[DESIGN\] A join's right operand read again for each row of its left
//! (RULES.md D38): a product's [X3.135-1986 §5.20 GR2b], and an outer
//! join's with no equalities of columns to hash on. It is held in memory
//! while it fits the work area, else in a run of the spill area, read again
//! from its first record each time; either way in the order it came. A
//! product reads it once for each left row, so what it reads is as many
//! rows as it gives. Split from exec/join.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::super::RowIter;
use super::super::work::Work;
use crate::error::Result;
use crate::spill::{SpillRun, record_row, row_bytes, row_record};
use crate::storage::Row;

/// The rows of a right operand, each pass of them.
pub(in crate::exec) type Pass = Box<dyn Iterator<Item = Result<Row>> + Send>;

/// A right operand read again for each left row.
pub(in crate::exec) enum Rescanned {
    /// Its rows, in memory.
    Held(Arc<Vec<Row>>),
    /// Its rows, in a run of the spill area.
    Spilled(Box<dyn SpillRun>),
}

impl Rescanned {
    /// The rows `rows`, in memory until they outgrow `work`, then in a run
    /// of its spill area; and how many there are.
    ///
    /// # Errors
    ///
    /// The rows', and the spill area's.
    pub(in crate::exec) fn of(rows: RowIter, work: &mut Work) -> Result<(Rescanned, usize)> {
        let mut gathering = Gathering::default();
        for r in rows {
            gathering.push(r?, work)?;
        }
        Ok(gathering.done())
    }

    /// The bytes it holds in memory, near enough: none once spilled.
    pub(in crate::exec) fn held_bytes(&self) -> usize {
        match self {
            Rescanned::Held(rows) => rows.iter().map(row_bytes).sum(),
            Rescanned::Spilled(_) => 0,
        }
    }

    /// Its rows, from the first, in order.
    ///
    /// # Errors
    ///
    /// The spill area's, reading the run.
    pub(in crate::exec) fn pass(&mut self) -> Result<Pass> {
        match self {
            Rescanned::Held(rows) => {
                let rows = Arc::clone(rows);
                Ok(Box::new((0..rows.len()).filter_map(move |n| rows.get(n).cloned().map(Ok))))
            }
            Rescanned::Spilled(run) => Ok(Box::new(run.records()?.map(|r| r.and_then(|record| record_row(&record))))),
        }
    }
}

/// Rows being gathered into a `Rescanned`: in memory until they outgrow
/// their work area, then in a run of its spill area.
#[derive(Default)]
struct Gathering {
    held: Vec<Row>,
    bytes: usize,
    count: usize,
    run: Option<Box<dyn SpillRun>>,
}

impl Gathering {
    /// Adds `row`, the rows held moved to a run of `work`'s spill area once
    /// they outgrow it.
    ///
    /// # Errors
    ///
    /// The spill area's.
    fn push(&mut self, row: Row, work: &mut Work) -> Result<()> {
        self.count = self.count.saturating_add(1);
        if let Some(run) = &mut self.run {
            return run.push(&row_record(&row));
        }
        self.bytes = self.bytes.saturating_add(row_bytes(&row));
        self.held.push(row);
        if self.bytes > work.limit() {
            let mut spilled = work.run()?;
            for one in self.held.drain(..) {
                spilled.push(&row_record(&one))?;
            }
            self.run = Some(spilled);
        }
        Ok(())
    }

    /// The rows gathered, and how many.
    fn done(self) -> (Rescanned, usize) {
        let rescanned = match self.run {
            Some(run) => Rescanned::Spilled(run),
            None => Rescanned::Held(Arc::new(self.held)),
        };
        (rescanned, self.count)
    }
}

/// [X3.135-1986 §5.20 GR2b] every row of `left` concatenated with every row
/// of `right`, in `left`'s order, then `right`'s.
pub(super) fn product(left: RowIter, right: Rescanned) -> RowIter {
    Box::new(Product { left, right, current: None })
}

/// A product, as it is pulled: the left row being paired, and the pass of
/// the right it is paired with.
struct Product {
    left: RowIter,
    right: Rescanned,
    current: Option<(Row, Pass)>,
}

impl Iterator for Product {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        loop {
            if let Some((l, pass)) = &mut self.current {
                match pass.next() {
                    Some(r) => return Some(r.map(|r| l.iter().chain(&r).cloned().collect())),
                    None => self.current = None,
                }
            }
            let l = match self.left.next()? {
                Ok(l) => l,
                Err(e) => return Some(Err(e)),
            };
            match self.right.pass() {
                Ok(pass) => self.current = Some((l, pass)),
                Err(e) => return Some(Err(e)),
            }
        }
    }
}
