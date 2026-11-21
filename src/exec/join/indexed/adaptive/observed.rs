//! Observe completed probes before paying to buffer and price the
//! remaining spatial input. Cheap probes stay lazy; a later broad probe
//! can still trigger adaptation after an initially selective prefix.

use super::super::{IndexJoin, Step};
use super::{RowIter, chosen, failed};
use crate::error::Result;
use crate::exec::Db;
use crate::storage::Row;

pub(super) struct Observed {
    join: Option<IndexJoin>,
    columns: Vec<usize>,
    state: Db,
    remainder: Option<RowIter>,
    candidates: usize,
}

impl Observed {
    pub(super) fn new(join: IndexJoin, columns: Vec<usize>, state: &Db) -> Self {
        Self { join: Some(join), columns, state: state.clone(), remainder: None, candidates: 0 }
    }

    fn promising(&mut self) -> bool {
        let Some(join) = self.join.as_ref() else { return false };
        self.candidates = self.candidates.saturating_add(join.candidates);
        // Buffering and estimating every remaining probe can itself cost
        // as much as a selective join. Require a table-sized count of
        // actual candidate reads before paying this optional setup cost.
        join.candidates != 0 && self.candidates >= join.table.storage.row_count()
    }
}

impl Iterator for Observed {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(rows) = &mut self.remainder {
                return rows.next();
            }
            match self.join.as_mut()?.advance() {
                Ok(Step::Row(row)) => return Some(Ok(row)),
                Ok(Step::End) => {
                    self.join = None;
                    return None;
                }
                Ok(Step::ProbeEnd) => {
                    let promising = self.promising();
                    if promising {
                        let join = self.join.take()?;
                        let columns = std::mem::take(&mut self.columns);
                        self.remainder = Some(chosen(join, columns, &self.state).unwrap_or_else(failed));
                    }
                }
                Err(error) => {
                    self.join = None;
                    return Some(Err(error));
                }
            }
        }
    }
}
