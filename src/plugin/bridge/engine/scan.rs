//! \[EXTENSION\] A scan of a plug-in engine's table (RULES.md D224): the
//! program's rows, a page of BATCH a SCAN, in order of id, merged with
//! the overlay of the transaction reading, also in order of id.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use super::Shared;
use crate::error::Result;
use crate::plugin::payload::ENGINE_SCAN;
use crate::plugin::payload::engine::{identified, put_scan};
use crate::storage::{Row, RowId};

use super::super::answer;

/// A scan.
pub(super) struct Scan {
    shared: Arc<Shared>,
    /// The overlay's rows, from the least id not yet given.
    overlay: std::iter::Peekable<std::collections::btree_map::IntoIter<RowId, Option<Row>>>,
    /// The page read, not yet given.
    page: VecDeque<(RowId, Row)>,
    /// The greatest id read of the program's.
    after: RowId,
    /// Whether the program has more rows.
    more: bool,
    /// Whether an error ended it.
    failed: bool,
}

impl Scan {
    /// A scan of `shared`'s rows and `overlay`.
    pub(super) fn new(shared: Arc<Shared>, overlay: BTreeMap<RowId, Option<Row>>) -> Scan {
        Scan { shared, overlay: overlay.into_iter().peekable(), page: VecDeque::new(), after: 0, more: true, failed: false }
    }

    /// The next page read, if the page is empty and the program has more.
    fn fill(&mut self) -> Result<()> {
        if !self.page.is_empty() || !self.more {
            return Ok(());
        }
        let mut payload = Vec::new();
        put_scan(&mut payload, &self.shared.locator, self.after, self.shared.batch);
        let (rows, more) = answer(&self.shared.link.plain(ENGINE_SCAN, payload)?, |r| Ok((identified(r)?, r.u8()? != 0)))?;
        for (id, row) in rows {
            if id <= self.after {
                return Err(crate::plugin::errors::violation(&format!("its SCAN gave row {id}, not after {}", self.after)));
            }
            self.after = id;
            self.page.push_back((id, self.shared.stored(row)?));
        }
        self.more = more && !self.page.is_empty();
        Ok(())
    }

    /// The next row, merged.
    fn step(&mut self) -> Result<Option<(RowId, Row)>> {
        loop {
            self.fill()?;
            let program = self.page.front().map(|(id, _)| *id);
            let own = self.overlay.peek().map(|(id, _)| *id);
            let take_own = match (program, own) {
                (None, None) => return Ok(None),
                (Some(p), Some(o)) => o <= p,
                (None, Some(_)) => true,
                (Some(_), None) => false,
            };
            if take_own {
                let Some((id, row)) = self.overlay.next() else { return Ok(None) };
                if program == Some(id) {
                    self.page.pop_front();
                }
                if let Some(row) = row {
                    return Ok(Some((id, row)));
                }
                continue;
            }
            return Ok(self.page.pop_front());
        }
    }
}

impl Iterator for Scan {
    type Item = Result<(RowId, Row)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        let got = self.step();
        self.failed = got.is_err();
        got.transpose()
    }
}
