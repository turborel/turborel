//! \[DESIGN\] A scan of a table with frozen row groups (RULES.md D31): its
//! tree's entries and its groups' rows merged in row id order, a row's
//! entry taken before its frozen values. Each group's rows are read a
//! vector at a time, only the runs whose chunks a condition may match
//! (`columns/scan.rs`); those of a group sorted by its columns
//! (COLUMNSTORE.ORDER) all as it is taken up, then put in row id order.
//! Split from frozen.rs (RULES.md R-SIZE-2).

use std::collections::VecDeque;
use std::iter::Peekable;
use std::sync::Arc;

use super::{Directory, Entry};
use crate::engine::columns::scan::{candidates, rows};
use crate::engine::rows::mvcc::Reader;
use crate::engine::rows::scan::TreeOf;
use crate::engine::rows::{RowTable, row_id};
use crate::engine::tree::Scan;
use crate::error::Result;
use crate::storage::{Conjunction, Row, RowId};

/// \[DESIGN\] The rows a scan reads of a group at a time: a vector.
const VECTOR: u32 = 2048;
/// The same, as a count of rows.
pub(super) const VECTOR_ROWS: usize = 2048;

/// What a merged scan gives: the rows a reader sees, or only the frozen
/// rows no entry of the tree stands for.
#[derive(Debug, Clone, Copy)]
pub(in crate::engine::rows) enum Merging {
    /// Every row `Reader` sees.
    Seen(Reader),
    /// Only the frozen rows the tree has no entry for.
    FrozenOnly,
}

/// What a step of a merged scan found.
enum Step {
    /// The end of the scan.
    End,
    /// A row it gives.
    Row((RowId, Row)),
    /// An entry of the tree it does not give.
    Skip,
}

/// \[DESIGN\] A table's rows, its tree's and its groups', in row id order.
pub(in crate::engine::rows) struct Merged {
    table: Arc<RowTable>,
    merging: Merging,
    columns: Vec<usize>,
    condition: Option<Conjunction>,
    delta: Peekable<Scan<TreeOf>>,
    directory: Arc<Directory>,
    /// The next group to read.
    next_group: usize,
    /// The group being read, its row ids, and the runs of it left.
    group: Option<(Arc<Entry>, Vec<RowId>)>,
    runs: VecDeque<(u32, u32)>,
    frozen: VecDeque<(RowId, Row)>,
}

impl Merged {
    /// The rows of `table` that `merging` gives, `columns` decoded, and only
    /// those `condition` matches, if given; `directory` its groups, taken
    /// before its tree is read.
    pub(in crate::engine::rows) fn new(
        table: Arc<RowTable>,
        (merging, directory): (Merging, Arc<Directory>),
        columns: Vec<usize>,
        condition: Option<Conjunction>,
    ) -> Merged {
        let delta = RowTable::entries(&table).peekable();
        Merged {
            table,
            merging,
            columns,
            condition,
            delta,
            directory,
            next_group: 0,
            group: None,
            runs: VecDeque::new(),
            frozen: VecDeque::new(),
        }
    }

    /// The row id of the next frozen row, its rows read as they are needed.
    fn next_frozen(&mut self) -> Result<Option<RowId>> {
        while self.frozen.is_empty() {
            if let Some((from, to)) = self.runs.pop_front() {
                self.read_run(from, to)?;
                continue;
            }
            let Some(entry) = self.directory.groups.get(self.next_group).cloned() else { return Ok(None) };
            self.next_group = self.next_group.saturating_add(1);
            let ids = entry.group.row_ids(&self.table.group_pages()?)?;
            for (from, to) in candidates(&entry.group, self.condition.as_ref()) {
                let mut at = from;
                while at < to {
                    let end = at.saturating_add(VECTOR).min(to);
                    self.runs.push_back((at, end));
                    at = end;
                }
            }
            let sorted = entry.group.sorted();
            self.group = Some((entry, ids));
            if sorted {
                while let Some((from, to)) = self.runs.pop_front() {
                    self.read_run(from, to)?;
                }
                self.frozen.make_contiguous().sort_unstable_by_key(|(id, _)| *id);
            }
        }
        Ok(self.frozen.front().map(|(id, _)| *id))
    }

    /// Reads positions `from` to `to` of the group being read, keeping the
    /// rows the condition matches.
    fn read_run(&mut self, from: u32, to: u32) -> Result<()> {
        let Some((entry, ids)) = &self.group else { return Ok(()) };
        let pages = self.table.group_pages()?;
        let found = rows(&entry.group, &pages, self.table.format.types(), &self.columns, (from, to))?;
        let first = usize::try_from(from).unwrap_or(usize::MAX);
        for (n, row) in found.into_iter().enumerate() {
            let Some(&id) = ids.get(first.saturating_add(n)) else { continue };
            if self.condition.as_ref().is_none_or(|c| c.matches(&row)) {
                self.frozen.push_back((id, row));
            }
        }
        Ok(())
    }

    /// The tree's next entry as a row the scan gives, if it gives one.
    fn delta_row(&mut self, id: RowId, newest: Vec<u8>) -> Result<Option<(RowId, Row)>> {
        let Merging::Seen(reader) = self.merging else { return Ok(None) };
        let Some(record) = self.table.visible_from(newest, reader)? else { return Ok(None) };
        let row = self.table.decode(id, &record, Some(&self.columns))?;
        Ok(self.condition.as_ref().is_none_or(|c| c.matches(&row)).then_some((id, row)))
    }

    fn step(&mut self) -> Result<Step> {
        let frozen = self.next_frozen()?;
        let delta = match self.delta.peek() {
            Some(Ok((k, _))) => Some(row_id(k)?),
            Some(Err(_)) => return self.delta.next().transpose().map(|_| Step::Skip),
            None => None,
        };
        // The tree's entry first; a frozen row of its id is the same row's,
        // older.
        let Some(d) = delta.filter(|&d| frozen.is_none_or(|f| d <= f)) else {
            return Ok(self.frozen.pop_front().map_or(Step::End, Step::Row));
        };
        if frozen == Some(d) {
            self.frozen.pop_front();
        }
        let Some(Ok((_, newest))) = self.delta.next() else { return Ok(Step::Skip) };
        Ok(self.delta_row(d, newest)?.map_or(Step::Skip, Step::Row))
    }
}

impl Iterator for Merged {
    type Item = Result<(RowId, Row)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.step() {
                Ok(Step::End) => return None,
                Ok(Step::Row(row)) => return Some(Ok(row)),
                Ok(Step::Skip) => {}
                Err(e) => return Some(Err(e)),
            }
        }
    }
}
