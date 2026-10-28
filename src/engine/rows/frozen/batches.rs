//! \[DESIGN\] A COLUMNS table's rows by batch (`TableStorage::scan_batches`,
//! RULES.md D31), in no particular order: first its frozen rows, a vector
//! at a time, each column's values straight from its chunks, no row made,
//! leaving out those the tree has an entry for; then the tree's rows, as
//! the reader sees them. A condition's columns are read first, and the
//! other columns only of the rows it keeps. Split from frozen.rs (RULES.md
//! R-SIZE-2).

use std::collections::VecDeque;
use std::ops::Bound;
use std::sync::Arc;

use super::Directory;
use super::merge::VECTOR_ROWS;
use crate::engine::columns::group::Group;
use crate::engine::columns::scan::{candidates, values, values_at};
use crate::engine::rows::mvcc::Reader;
use crate::engine::rows::scan::TreeOf;
use crate::engine::rows::{RowTable, key, row_id};
use crate::engine::tree::{Direction, Scan};
use crate::error::Result;
use crate::storage::{BATCH_ROWS, Batch, Condition, RowId};
use crate::value::Value;

/// \[DESIGN\] A COLUMNS table's batches.
pub(in crate::engine::rows) struct Batched {
    table: Arc<RowTable>,
    reader: Reader,
    columns: Vec<usize>,
    condition: Option<Condition>,
    /// Its groups, taken before its tree is read, held to the end.
    directory: Arc<Directory>,
    next_group: usize,
    /// The group being read, its row ids, and its runs left to read.
    group: Option<(usize, Vec<RowId>)>,
    runs: VecDeque<(u32, u32)>,
    /// The tree's entries, once the groups are read.
    delta: Option<Scan<TreeOf>>,
}

impl Batched {
    /// The batches of `table`'s rows `reader` sees, of `columns`, only those
    /// `condition` matches if given; `directory` its groups.
    pub(in crate::engine::rows) fn new(
        table: Arc<RowTable>,
        (reader, directory): (Reader, Arc<Directory>),
        columns: Vec<usize>,
        condition: Option<Condition>,
    ) -> Batched {
        Batched { table, reader, columns, condition, directory, next_group: 0, group: None, runs: VecDeque::new(), delta: None }
    }

    /// The group being read.
    fn current(&self) -> Option<&Group> {
        self.group.as_ref().and_then(|(at, _)| self.directory.groups.get(*at)).map(|e| &e.group)
    }

    /// Takes up the next group, its runs a vector at most each; whether
    /// there was one.
    fn next_group(&mut self) -> Result<bool> {
        let Some(entry) = self.directory.groups.get(self.next_group) else { return Ok(false) };
        let ids = entry.group.row_ids(&self.table.group_pages()?)?;
        let vector = u32::try_from(VECTOR_ROWS).unwrap_or(u32::MAX);
        for (from, to) in candidates(&entry.group, self.condition.as_ref()) {
            let mut at = from;
            while at < to {
                let end = at.saturating_add(vector).min(to);
                self.runs.push_back((at, end));
                at = end;
            }
        }
        self.group = Some((self.next_group, ids));
        self.next_group = self.next_group.saturating_add(1);
        Ok(true)
    }

    /// The positions from `from` to `to` of the group being read whose rows
    /// the tree has no entry for.
    fn unshadowed(&self, (from, to): (u32, u32)) -> Result<Vec<u32>> {
        let Some((_, ids)) = &self.group else { return Ok(Vec::new()) };
        let at = |p: u32| usize::try_from(p).ok().and_then(|p| ids.get(p)).copied();
        let (Some(first), Some(last)) = (at(from), at(to.saturating_sub(1))) else { return Ok(Vec::new()) };
        let (low, high) = (key(first), key(last));
        let mut shadowed = Vec::new();
        for entry in self.table.tree.scan(Bound::Included(&low[..]), Bound::Included(&high[..]), Direction::Forward) {
            shadowed.push(row_id(&entry?.0)?);
        }
        Ok((from..to).filter(|&p| at(p).is_some_and(|id| shadowed.binary_search(&id).is_err())).collect())
    }

    /// The batch of the run `from` to `to` of the group being read, if any
    /// of its rows is given.
    fn run_batch(&self, (from, to): (u32, u32)) -> Result<Option<Batch>> {
        let Some(group) = self.current() else { return Ok(None) };
        let pages = self.table.group_pages()?;
        let types = self.table.format.types();
        let mut positions = self.unshadowed((from, to))?;
        if let Some(condition) = &self.condition {
            let keys = condition
                .columns
                .iter()
                .map(|&c| types.get(c).map_or(Ok(Vec::new()), |t| values_at(group, &pages, (c, t), &positions)))
                .collect::<Result<Vec<_>>>()?;
            let kept: Vec<u32> = (0..positions.len())
                .filter(|&n| {
                    let key: Option<Vec<&Value>> = keys.iter().map(|k| k.get(n)).collect();
                    key.is_some_and(|k| condition.matches_key(&k))
                })
                .filter_map(|n| positions.get(n).copied())
                .collect();
            positions = kept;
        }
        if positions.is_empty() {
            return Ok(None);
        }
        let whole = u32::try_from(positions.len()).ok() == Some(to.saturating_sub(from));
        let mut batch = Batch { columns: Vec::with_capacity(self.columns.len()), rows: positions.len() };
        for &c in &self.columns {
            let column = match types.get(c) {
                // A run whole is read a chunk at a time, each value once.
                Some(t) => {
                    if whole {
                        values(group, &pages, (c, t), (from, to))?
                    } else {
                        values_at(group, &pages, (c, t), &positions)?
                    }
                }
                None => vec![Value::Null; positions.len()],
            };
            batch.columns.push(column);
        }
        Ok(Some(batch))
    }

    /// The next batch of the tree's rows the reader sees, if any is left.
    fn delta_batch(&mut self) -> Result<Option<Batch>> {
        let table = Arc::clone(&self.table);
        let delta = self.delta.get_or_insert_with(|| RowTable::entries(&table));
        let mut decoded = self.columns.clone();
        decoded.extend(self.condition.iter().flat_map(|c| c.columns.iter().copied()));
        let mut batch = Batch::empty(self.columns.len());
        while batch.rows < BATCH_ROWS {
            let Some(entry) = delta.next() else { break };
            let (k, newest) = entry?;
            let id = row_id(&k)?;
            let Some(record) = table.visible_from(newest, self.reader)? else { continue };
            let row = table.decode(id, &record, Some(&decoded))?;
            if self.condition.as_ref().is_none_or(|c| c.matches(&row)) {
                batch.push(&row, &self.columns);
            }
        }
        Ok((batch.rows > 0).then_some(batch))
    }

    fn step(&mut self) -> Result<Option<Batch>> {
        loop {
            if let Some(run) = self.runs.pop_front() {
                match self.run_batch(run)? {
                    Some(batch) => return Ok(Some(batch)),
                    None => continue,
                }
            }
            if self.next_group()? {
                continue;
            }
            return self.delta_batch();
        }
    }
}

impl Iterator for Batched {
    type Item = Result<Batch>;

    fn next(&mut self) -> Option<Self::Item> {
        self.step().transpose()
    }
}
