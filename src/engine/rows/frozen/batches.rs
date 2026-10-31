//! \[DESIGN\] A column store table's rows by batch (`TableStorage::scan_batches`,
//! RULES.md D31), in no particular order: first its frozen rows, a vector
//! at a time, each column's values straight from its chunks, no row made;
//! then the tree's rows, as the reader sees them. The chunks the scan's
//! conditions cannot match are skipped; the rows of the rest are given
//! for the filter to judge, as it judges every row given.
//!
//! A group's rows the tree has entries for are read from them as the
//! group is taken up, the tree read once over its range of row ids, in
//! whatever order its rows are (COLUMNSTORE.ORDER), and that range then
//! left out of the tree's rows read last: a freeze meanwhile may take out
//! an entry of a row frozen before (`freeze.rs`), whose group's row is the
//! same, so each row is read once, of the one or the other. Split from
//! frozen.rs (RULES.md R-SIZE-2).

use std::collections::VecDeque;
use std::ops::Bound;
use std::sync::Arc;

use super::Directory;
use super::merge::VECTOR_ROWS;
use crate::engine::columns::group::{Group, Pages, Summary};
use crate::engine::columns::scan::{floats, floats_at, ints, ints_at, read_candidates, texts, texts_at, values, values_at};
use crate::engine::rows::gathered::Gathering;
use crate::engine::rows::mvcc::Reader;
use crate::engine::rows::scan::TreeOf;
use crate::engine::rows::sealing::Sealing;
use crate::engine::rows::{RowTable, key, row_id};
use crate::engine::tree::{Direction, Scan};
use crate::error::Result;
use crate::storage::{BATCH_ROWS, Batch, Column, Conjunction, RowId};
use crate::value::DataType;
use crate::value::Value;

/// \[DESIGN\] A column store table's batches.
pub(in crate::engine::rows) struct Batched {
    table: Arc<RowTable>,
    reader: Reader,
    columns: Vec<usize>,
    condition: Option<Conjunction>,
    /// Its groups, taken before its tree is read, held to the end.
    directory: Arc<Directory>,
    next_group: usize,
    /// The group being read, its row ids, those the tree has entries for,
    /// in order, and its runs left to read.
    group: Option<(Arc<Group>, Vec<RowId>, Vec<RowId>)>,
    runs: VecDeque<(u32, u32)>,
    /// The records, unsealed, of the tree's entries in the group's range,
    /// left to give, each of its row.
    entries: Vec<(RowId, Vec<u8>)>,
    /// The tree's entries, once the groups are read.
    delta: Option<Scan<TreeOf>>,
    /// The ranges of row ids of the tree's entries read with the groups'
    /// rows, in order, left out of `delta`.
    read: Vec<(RowId, RowId)>,
}

impl Batched {
    /// The batches of `table`'s rows `reader` sees, of `columns`, only those
    /// `condition` matches if given; `directory` its groups.
    pub(in crate::engine::rows) fn new(
        table: Arc<RowTable>,
        (reader, directory): (Reader, Arc<Directory>),
        columns: Vec<usize>,
        condition: Option<Conjunction>,
    ) -> Batched {
        Batched {
            table,
            reader,
            columns,
            condition,
            directory,
            next_group: 0,
            group: None,
            runs: VecDeque::new(),
            entries: Vec::new(),
            delta: None,
            read: Vec::new(),
        }
    }

    /// The group being read.
    fn current(&self) -> Option<&Group> {
        self.group.as_ref().map(|(group, _, _)| &**group)
    }

    /// Takes up the next group, its runs a vector at most each, and the
    /// tree's entries in its range; whether there was one. A group the
    /// condition cannot match by its summary is passed over unread, the
    /// tree's entries in its range read with the rest.
    fn next_group(&mut self) -> Result<bool> {
        let Some(entry) = self.directory.groups.get(self.next_group).cloned() else { return Ok(false) };
        self.group = None;
        if !entry.summary.may_match(self.condition.as_ref()) {
            self.next_group = self.next_group.saturating_add(1);
            return Ok(true);
        }
        let shadowed = self.entries_in(&entry.summary)?;
        let pages = self.table.group_pages()?;
        let group = self.table.group_of(&entry)?;
        let ids = group.row_ids(&pages)?;
        let vector = u32::try_from(VECTOR_ROWS).unwrap_or(u32::MAX);
        for (from, to) in read_candidates(&group, &pages, self.condition.as_ref())? {
            let mut at = from;
            while at < to {
                let end = at.saturating_add(vector).min(to);
                self.runs.push_back((at, end));
                at = end;
            }
        }
        self.group = Some((group, ids, shadowed));
        self.next_group = self.next_group.saturating_add(1);
        Ok(true)
    }

    /// The record, unsealed, of the tree's entry of `id`, `newest`, as the
    /// reader sees it, if the condition matches its row.
    fn delta_record(&self, (id, newest): (RowId, Vec<u8>)) -> Result<Option<Vec<u8>>> {
        let Some(record) = self.table.visible_from(newest, self.reader)? else { return Ok(None) };
        self.table.plain_matching((id, &record), self.condition.as_ref())
    }

    /// The ids of the tree's entries in `group`'s range of row ids, in
    /// order, the records of those the reader sees kept to give, and the
    /// range noted as read.
    fn entries_in(&mut self, group: &Summary) -> Result<Vec<RowId>> {
        let (low, high) = (key(group.first), key(group.last));
        let mut shadowed = Vec::new();
        for entry in self.table.tree.scan(Bound::Included(&low[..]), Bound::Included(&high[..]), Direction::Forward) {
            let (k, newest) = entry?;
            let id = row_id(&k)?;
            shadowed.push(id);
            if let Some(record) = self.delta_record((id, newest))? {
                self.entries.push((id, record));
            }
        }
        self.read.push((group.first, group.last));
        Ok(shadowed)
    }

    /// The positions from `from` to `to` of the group being read whose rows
    /// the tree has no entry for.
    fn unshadowed(&self, (from, to): (u32, u32)) -> Vec<u32> {
        let Some((_, ids, shadowed)) = &self.group else { return Vec::new() };
        let at = |p: u32| usize::try_from(p).ok().and_then(|p| ids.get(p)).copied();
        (from..to).filter(|&p| at(p).is_some_and(|id| shadowed.binary_search(&id).is_err())).collect()
    }

    /// A batch of the rows of the tree's entries left to give, if any, each
    /// column decoded straight into its vector (`gathered.rs`).
    fn entries_batch(&mut self) -> Result<Option<Batch>> {
        if self.entries.is_empty() {
            return Ok(None);
        }
        let rest = self.entries.split_off(self.entries.len().saturating_sub(BATCH_ROWS));
        let mut gathering = Gathering::new(self.table.format.types(), &self.columns);
        for (id, record) in &rest {
            self.table.gather_plain((*id, record), &mut gathering)?;
        }
        Ok(Some(gathering.take()))
    }

    /// The batch of the run `from` to `to` of the group being read, if any
    /// of its rows the tree has no entry for is given.
    fn run_batch(&self, (from, to): (u32, u32)) -> Result<Option<Batch>> {
        let Some(group) = self.current() else { return Ok(None) };
        let pages = self.table.group_pages()?;
        let types = self.table.format.types();
        // A frozen row is not judged here, the filter judging every row
        // given over its vectors (`exec/batch/judge.rs`): the conditions
        // skipped the chunks they cannot match (`candidates`), and judging
        // a row here too cost more than it saved.
        let positions = self.unshadowed((from, to));
        if positions.is_empty() {
            return Ok(None);
        }
        let whole = u32::try_from(positions.len()).ok() == Some(to.saturating_sub(from));
        let mut batch = Batch { columns: Vec::with_capacity(self.columns.len()), rows: positions.len() };
        for &c in &self.columns {
            let column = match types.get(c) {
                // A run whole is read a chunk at a time, each value once, as
                // integers, approximate numbers or text if it is of them.
                Some(t) => {
                    if whole {
                        run_column(group, &pages, (c, t), (from, to))?
                    } else {
                        positions_column(group, &pages, (c, t), &positions)?
                    }
                }
                None => Column::from(vec![Value::Null; positions.len()]),
            };
            batch.columns.push(column);
        }
        Ok(Some(batch))
    }

    /// Whether the tree's entry of `id` was read with the groups' rows.
    fn was_read(&self, id: RowId) -> bool {
        let after = self.read.partition_point(|&(first, _)| first <= id);
        after.checked_sub(1).and_then(|at| self.read.get(at)).is_some_and(|&(_, last)| id <= last)
    }

    /// The next batch of the tree's rows the reader sees, if any is left.
    fn delta_batch(&mut self) -> Result<Option<Batch>> {
        let mut delta = self.delta.take().unwrap_or_else(|| RowTable::entries(&self.table));
        let mut gathering = Gathering::new(self.table.format.types(), &self.columns);
        let mut failed = None;
        while gathering.rows() < BATCH_ROWS {
            let Some(entry) = delta.next() else { break };
            let found = entry.and_then(|(k, newest)| {
                let id = row_id(&k)?;
                let record = if self.was_read(id) { None } else { self.delta_record((id, newest))? };
                record.map(|plain| self.table.gather_plain((id, &plain), &mut gathering)).transpose()
            });
            if let Err(e) = found {
                failed = Some(e);
                break;
            }
        }
        self.delta = Some(delta);
        failed.map_or(Ok((gathering.rows() > 0).then(|| gathering.take())), Err)
    }

    fn step(&mut self) -> Result<Option<Batch>> {
        loop {
            if let Some(batch) = self.entries_batch()? {
                return Ok(Some(batch));
            }
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

/// Column `c`, of type `t`, of the rows at positions `from` to `to` of
/// `group`: as integers, approximate numbers or text if it is of them,
/// else as values.
fn run_column(group: &Group, pages: &Pages<'_, Sealing>, (c, t): (usize, &DataType), (from, to): (u32, u32)) -> Result<Column> {
    if let Some(ints) = ints(group, pages, (c, t), (from, to))? {
        return Ok(Column::Ints(ints));
    }
    if let Some(floats) = floats(group, pages, (c, t), (from, to))? {
        return Ok(Column::Floats(floats));
    }
    if let Some(texts) = texts(group, pages, (c, t), (from, to))? {
        return Ok(Column::Texts(texts));
    }
    Ok(Column::from(values(group, pages, (c, t), (from, to))?))
}

/// As `run_column`, of the rows at `positions`.
fn positions_column(group: &Group, pages: &Pages<'_, Sealing>, (c, t): (usize, &DataType), positions: &[u32]) -> Result<Column> {
    if let Some(ints) = ints_at(group, pages, (c, t), positions)? {
        return Ok(Column::Ints(ints));
    }
    if let Some(floats) = floats_at(group, pages, (c, t), positions)? {
        return Ok(Column::Floats(floats));
    }
    if let Some(texts) = texts_at(group, pages, (c, t), positions)? {
        return Ok(Column::Texts(texts));
    }
    Ok(Column::from(values_at(group, pages, (c, t), positions)?))
}

impl Iterator for Batched {
    type Item = Result<Batch>;

    fn next(&mut self) -> Option<Self::Item> {
        self.step().transpose()
    }
}
