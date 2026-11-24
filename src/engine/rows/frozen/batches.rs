//! \[DESIGN\] A column store table's rows by batch (`TableStorage::scan_batches`,
//! RULES.md D31), in no particular order: first its frozen rows, a vector
//! at a time, each column's values straight from its chunks, no row made;
//! then the tree's rows, as the reader sees them. The chunks the scan's
//! conditions cannot match are skipped; the rows of the rest, and the
//! tree's, are given for the filter to judge, as it judges every row
//! given. A vector of integers kept as runs says so (`Ints::with_runs`),
//! so the filter and the set functions work once a run.
//!
//! Of a frozen run, the conjuncts the scan can judge (`columns/judged.rs`,
//! RULES.md D360) are judged first, of their own columns alone, and the
//! other columns decoded only at the rows they keep (`batches/run.rs`); the
//! rows kept of a group's runs are gathered until they make half a batch,
//! so batches stay large. A column asked for late (RULES.md D361) is not
//! decoded at all, but given as the places of its rows in the group and
//! the group to decode them of (`frozen/late.rs`).
//!
//! As a group is taken up, the pages of the chunks its runs are read from
//! are asked to be read ahead, a window at a time (`columns/ahead.rs`);
//! each chunk a run reads is opened once for the group's runs, and let go
//! as the runs pass it or the next group is taken up
//! (`columns/prepared.rs`).
//!
//! A group's rows the tree has entries for are read from them as the group
//! is taken up, the tree read once over its range of row ids, in whatever
//! order its rows are (COLUMNSTORE.ORDER), and that range then left out of
//! the tree's rows read last: a freeze meanwhile may take out an entry of
//! a row frozen before (`freeze.rs`), whose group's row is the same, so
//! each row is read once, of the one or the other.

use std::collections::VecDeque;
use std::ops::Bound;
use std::sync::Arc;

use super::late::FrozenLate;
use super::merge::VECTOR_ROWS;
use super::{Directory, Entry};
use crate::engine::columns::ahead::GroupAhead;
use crate::engine::columns::group::Group;
use crate::engine::columns::judged::Judgment;
use crate::engine::columns::prepared::Prepared;
use crate::engine::columns::scan::read_ordered;
use crate::engine::rows::gathered::Gathering;
use crate::engine::rows::mvcc::Reader;
use crate::engine::rows::scan::Ranged;
use crate::engine::rows::{RowTable, key, row_id};
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::storage::{BATCH_ROWS, Batch, Conjunction, LateSource, Ramp, RowId};

mod pending;
mod run;

/// \[DESIGN\] A column store table's batches.
pub(in crate::engine::rows) struct Batched {
    table: Arc<RowTable>,
    reader: Reader,
    columns: Vec<usize>,
    /// The condition, which skips chunks; the filter judges every row given.
    condition: Option<Conjunction>,
    /// The conjuncts of the condition judged of a run before its other
    /// columns are decoded (RULES.md D360).
    judgment: Judgment,
    /// The columns given late (RULES.md D361), and what the group being
    /// read decodes them of, if any is.
    late: Vec<usize>,
    source: Option<Arc<dyn LateSource>>,
    /// The rows kept of the group being read, not yet given, and the
    /// columns judging decoded of them (RULES.md D366).
    pending: pending::Pending,
    /// Its groups, taken before its tree is read, held to the end.
    directory: Arc<Directory>,
    next_group: usize,
    /// The group being read, its row ids if the tree has entries for any,
    /// those it has entries for, in order, and its runs left to read.
    group: Option<(Arc<Group>, Vec<RowId>, Vec<RowId>)>,
    runs: VecDeque<(u32, u32)>,
    /// The records, unsealed, of the tree's entries in the group's range,
    /// left to give, each of its row.
    entries: Vec<(RowId, Vec<u8>)>,
    /// The tree's entries, of the zones the condition may match, once the
    /// groups are read.
    delta: Option<Ranged>,
    /// The ranges of row ids of the tree's entries read with the groups'
    /// rows, in order, left out of `delta`.
    read: Vec<(RowId, RowId)>,
    /// The pages of the group being read it asks to be read ahead.
    ahead: GroupAhead,
    /// The chunks of the group being read its runs have opened.
    prepared: Prepared,
    /// How many of the tree's rows each batch of them holds.
    ramp: Ramp,
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
        let judgment = Judgment::of(condition.as_ref(), table.format.types());
        Batched {
            table,
            reader,
            columns,
            condition,
            judgment,
            late: Vec::new(),
            source: None,
            pending: pending::Pending::default(),
            directory,
            next_group: 0,
            group: None,
            runs: VecDeque::new(),
            entries: Vec::new(),
            delta: None,
            read: Vec::new(),
            ahead: GroupAhead::default(),
            prepared: Prepared::default(),
            ramp: Ramp::default(),
        }
    }

    /// As `new`, of a morsel of a split scan alone (RULES.md D290, D354),
    /// whose `directory` holds only the groups within its row ids, its
    /// tree's rows those of `zones`, the ranges of the morsel's ids the
    /// condition may match, as the split judged its table's zones once
    /// for all its morsels (RULES.md D1052), each batch of them whole.
    pub(in crate::engine::rows) fn within(
        table: Arc<RowTable>,
        (reader, directory): (Reader, Arc<Directory>),
        (columns, condition): (Vec<usize>, Option<Conjunction>),
        zones: Vec<(RowId, Option<RowId>)>,
    ) -> Batched {
        let delta = Some(RowTable::ranged(&table, zones));
        Batched { delta, ramp: Ramp::whole(), ..Batched::new(table, (reader, directory), columns, condition) }
    }

    /// As it is, `late`, those of its columns asked for late, given so
    /// (RULES.md D361).
    pub(in crate::engine::rows) fn with_late(mut self, late: &[usize]) -> Batched {
        self.late = late.iter().copied().filter(|c| self.columns.contains(c)).collect();
        self
    }

    /// Takes up the next group, its runs a vector at most each, and the
    /// tree's entries in its range; whether there was one. A group the
    /// condition cannot match by its summary is passed over unread, the
    /// tree's entries in its range read with the rest.
    fn next_group(&mut self) -> Result<bool> {
        let Some(entry) = self.directory.groups.get(self.next_group).cloned() else { return Ok(false) };
        self.group = None;
        self.prepared.clear();
        if !entry.summary.may_match(self.condition.as_ref()) {
            self.next_group = self.next_group.saturating_add(1);
            return Ok(true);
        }
        let shadowed = self.entries_in(&entry)?;
        let pages = self.table.group_pages()?;
        let group = self.table.group_of(&entry)?;
        // Its row ids read only to tell the rows the tree shadows.
        let ids = if shadowed.is_empty() { Vec::new() } else { group.row_ids(&pages)? };
        let vector = u32::try_from(VECTOR_ROWS).unwrap_or(u32::MAX);
        let (candidates, order) = read_ordered(&group, &pages, self.condition.as_ref())?;
        self.judgment.order_by(&order);
        // A late column's pages are read only as it is decoded.
        let eager: Vec<usize> = self.columns.iter().copied().filter(|c| !self.late.contains(c)).collect();
        self.ahead.take_up(&group, &eager, &candidates);
        for (from, to) in candidates {
            let mut at = from;
            while at < to {
                let end = at.saturating_add(vector).min(to);
                self.runs.push_back((at, end));
                at = end;
            }
        }
        self.source = (!self.late.is_empty()).then(|| {
            let source: Arc<dyn LateSource> = Arc::new(FrozenLate::new(Arc::clone(&self.table), Arc::clone(&entry), Arc::clone(&group)));
            source
        });
        self.group = Some((group, ids, shadowed));
        self.next_group = self.next_group.saturating_add(1);
        Ok(true)
    }

    /// The record, unsealed, of the tree's entry of `id`, `newest`, as the
    /// reader sees it, if it sees the row. Not judged here: the filter
    /// judges every row given over its vectors, and a record is decoded
    /// whole whatever is judged of it (`gathered.rs`), so judging it here
    /// would save no decoding, as judging a frozen run does (`run.rs`).
    fn delta_record(&self, (id, newest): (RowId, Vec<u8>)) -> Result<Option<Vec<u8>>> {
        let Some(record) = self.table.visible_from(newest, self.reader)? else { return Ok(None) };
        self.table.plain(id, &record).map(Some)
    }

    /// The ids of the tree's entries in `group`'s range of row ids, in
    /// order, the records of those the reader sees kept to give, and the
    /// range noted as read. The tree is read only over the ids whose
    /// shadow bits are set, not at all if none is (`shadow_bits.rs`).
    fn entries_in(&mut self, group: &Entry) -> Result<Vec<RowId>> {
        let mut shadowed = Vec::new();
        for (first, last) in self.table.shadow_ranges(group) {
            let (low, high) = (key(first), key(last));
            for entry in self.table.tree.scan(Bound::Included(&low[..]), Bound::Included(&high[..]), Direction::Forward) {
                let (k, newest) = entry?;
                let id = row_id(&k)?;
                shadowed.push(id);
                if let Some(record) = self.delta_record((id, newest))? {
                    self.entries.push((id, record));
                }
            }
        }
        self.read.push((group.summary.first, group.summary.last));
        Ok(shadowed)
    }

    /// The positions from `from` to `to` of the group being read whose rows
    /// the tree has no entry for: none if it has an entry for none of the
    /// group's, every position then given, no list made of them.
    fn unshadowed(&self, (from, to): (u32, u32)) -> Option<Vec<u32>> {
        let Some((_, ids, shadowed)) = &self.group else { return Some(Vec::new()) };
        if shadowed.is_empty() {
            return None;
        }
        let at = |p: u32| usize::try_from(p).ok().and_then(|p| ids.get(p)).copied();
        Some((from..to).filter(|&p| at(p).is_some_and(|id| shadowed.binary_search(&id).is_err())).collect())
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

    /// Whether the tree's entry of `id` was read with the groups' rows.
    fn was_read(&self, id: RowId) -> bool {
        let after = self.read.partition_point(|&(first, _)| first <= id);
        after.checked_sub(1).and_then(|at| self.read.get(at)).is_some_and(|&(_, last)| id <= last)
    }

    /// The next batch of the tree's rows the reader sees, if any is left.
    fn delta_batch(&mut self) -> Result<Option<Batch>> {
        // Only the zones of the tree's rows the condition may match
        // (`rows/zones.rs`).
        let mut delta =
            self.delta.take().unwrap_or_else(|| RowTable::ranged(&self.table, self.table.zones.ranges(self.condition.as_ref())));
        let mut gathering = Gathering::new(self.table.format.types(), &self.columns);
        let mut failed = None;
        let rows = self.ramp.next_rows();
        while gathering.rows() < rows {
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
                self.ahead.step(&self.table.group_pages()?);
                match self.run_batch(run)? {
                    Some(batch) => return Ok(Some(batch)),
                    None => continue,
                }
            }
            // The rows kept of the group's last runs, before the next group.
            if let Some(batch) = self.kept_batch()? {
                return Ok(Some(batch));
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
