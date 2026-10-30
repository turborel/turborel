//! \[DESIGN\] A column store table's rows in the order of the columns its
//! groups are sorted by (`TableStorage::scan_ordered`, RULES.md D31), so an
//! ORDER BY of them needs no sort: each group's rows read in their order,
//! a vector at a time, the runs a condition may match only, those the tree
//! has an entry for left out; the tree's rows the reader sees sorted as
//! ORDER BY sorts them; and all merged, the least next of each, by their
//! keys. A group not sorted by them, one frozen before COLUMNSTORE.ORDER
//! named them and not yet written again, is read whole and sorted. The
//! directory is taken before the tree is read, as a merged scan takes it
//! (`merge.rs`), so a row frozen meanwhile is read once. Split from
//! frozen.rs (RULES.md R-SIZE-2).

use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::Arc;

use super::Directory;
use super::merge::VECTOR_ROWS;
use crate::engine::columns::group::Group;
use crate::engine::columns::scan::{read_candidates, rows};
use crate::engine::rows::mvcc::Reader;
use crate::engine::rows::{RowTable, row_id};
use crate::error::Result;
use crate::storage::{Conjunction, Row, RowId};
use crate::value::{Value, sort_cmp};

/// Where rows come from, each in the order of the keys.
enum Source {
    /// Rows read and sorted.
    Rows(VecDeque<(RowId, Row)>),
    /// A sorted group, its row ids, its runs left to read, and the rows of
    /// the run read last left to give.
    Group { group: Arc<Group>, ids: Vec<RowId>, runs: VecDeque<(u32, u32)>, ready: VecDeque<(RowId, Row)> },
}

/// The next row of a source, by its keys.
struct Head {
    key: Vec<Value>,
    id: RowId,
    row: Row,
    source: usize,
}

/// How two rows' keys compare, as ORDER BY sorts them, then their ids.
fn by_keys(a: (&[Value], RowId), b: (&[Value], RowId)) -> Ordering {
    a.0.iter().zip(b.0).map(|(x, y)| sort_cmp(x, y)).find(|o| o.is_ne()).unwrap_or_else(|| a.1.cmp(&b.1))
}

impl PartialEq for Head {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Head {}

impl PartialOrd for Head {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Head {
    /// Reversed, so the heap gives the least first.
    fn cmp(&self, other: &Self) -> Ordering {
        by_keys((&other.key, other.id), (&self.key, self.id))
    }
}

/// \[DESIGN\] A column store table's rows in the order of its sort columns.
pub(in crate::engine::rows) struct Ordered {
    table: Arc<RowTable>,
    reader: Reader,
    directory: Arc<Directory>,
    /// The columns decoded: those asked for, the keys and the condition's.
    columns: Vec<usize>,
    keys: Vec<usize>,
    condition: Option<Conjunction>,
    /// The ids of the tree's entries, in order: their groups' rows left out.
    shadowed: Vec<RowId>,
    sources: Vec<Source>,
    heads: BinaryHeap<Head>,
    started: bool,
}

impl Ordered {
    /// The rows of `table` `reader` sees, `columns` decoded, only those
    /// `condition` matches if given, in the order of `keys`; `directory`
    /// its groups, taken before its tree is read.
    pub(in crate::engine::rows) fn new(
        table: Arc<RowTable>,
        (reader, directory): (Reader, Arc<Directory>),
        (columns, keys): (Vec<usize>, Vec<usize>),
        condition: Option<Conjunction>,
    ) -> Ordered {
        Ordered {
            table,
            reader,
            directory,
            columns,
            keys,
            condition,
            shadowed: Vec::new(),
            sources: Vec::new(),
            heads: BinaryHeap::new(),
            started: false,
        }
    }

    fn key_of(&self, row: &Row) -> Vec<Value> {
        self.keys.iter().map(|&k| row.get(k).cloned().unwrap_or(Value::Null)).collect()
    }

    fn matches(&self, row: &Row) -> bool {
        self.condition.as_ref().is_none_or(|c| c.matches(row))
    }

    /// Reads the tree's rows the reader sees, sorted, and takes up each
    /// group a condition may match, the first row of each a head.
    fn start(&mut self) -> Result<()> {
        let mut delta = Vec::new();
        for entry in RowTable::entries(&self.table) {
            let (k, newest) = entry?;
            let id = row_id(&k)?;
            self.shadowed.push(id);
            if let Some(record) = self.table.visible_from(newest, self.reader)? {
                let row = self.table.decode(id, &record, Some(&self.columns))?;
                if self.matches(&row) {
                    delta.push((id, row));
                }
            }
        }
        self.sources.push(Source::Rows(self.sorted(delta)));
        let groups = self.directory.groups.clone();
        for entry in groups.iter().filter(|e| e.summary.may_match(self.condition.as_ref())) {
            let pages = self.table.group_pages()?;
            let group = self.table.group_of(entry)?;
            let ids = group.row_ids(&pages)?;
            let runs: VecDeque<(u32, u32)> = read_candidates(&group, &pages, self.condition.as_ref())?.into_iter().collect();
            let mut source = Source::Group { group, ids, runs, ready: VecDeque::new() };
            if !self.in_order(&source) {
                source = Source::Rows(self.sorted(self.drained(source)?));
            }
            self.sources.push(source);
        }
        for at in 0..self.sources.len() {
            self.advance(at)?;
        }
        Ok(())
    }

    /// Whether `source`'s rows are in the order of the keys.
    fn in_order(&self, source: &Source) -> bool {
        match source {
            Source::Rows(_) => true,
            Source::Group { group, .. } => group.order.starts_with(&self.keys),
        }
    }

    /// `rows`, sorted by their keys.
    fn sorted(&self, mut rows: Vec<(RowId, Row)>) -> VecDeque<(RowId, Row)> {
        rows.sort_by(|(a_id, a), (b_id, b)| by_keys((&self.key_of(a), *a_id), (&self.key_of(b), *b_id)));
        rows.into()
    }

    /// Every row of `source` left, read.
    fn drained(&self, mut source: Source) -> Result<Vec<(RowId, Row)>> {
        let mut out = Vec::new();
        while let Some(row) = self.next_of(&mut source)? {
            out.push(row);
        }
        Ok(out)
    }

    /// The next row of `source`, a group's next run read as it is needed,
    /// its rows the tree has an entry for or the condition does not match
    /// left out.
    fn next_of(&self, source: &mut Source) -> Result<Option<(RowId, Row)>> {
        match source {
            Source::Rows(rows) => Ok(rows.pop_front()),
            Source::Group { group, ids, runs, ready } => {
                while ready.is_empty() {
                    let Some((from, to)) = runs.pop_front() else { return Ok(None) };
                    let end = from.saturating_add(u32::try_from(VECTOR_ROWS).unwrap_or(u32::MAX)).min(to);
                    if end < to {
                        runs.push_front((end, to));
                    }
                    ready.extend(self.run_of((group, ids), (from, end))?);
                }
                Ok(ready.pop_front())
            }
        }
    }

    /// The rows at positions `from` to `to` of `group`, whose ids are
    /// `ids`, the tree has no entry for and the condition matches.
    fn run_of(&self, (group, ids): (&Group, &[RowId]), (from, to): (u32, u32)) -> Result<Vec<(RowId, Row)>> {
        let pages = self.table.group_pages()?;
        let found = rows(group, &pages, self.table.format.types(), &self.columns, (from, to))?;
        let first = usize::try_from(from).unwrap_or(usize::MAX);
        Ok(found
            .into_iter()
            .enumerate()
            .filter_map(|(n, row)| ids.get(first.saturating_add(n)).map(|&id| (id, row)))
            .filter(|(id, row)| self.shadowed.binary_search(id).is_err() && self.matches(row))
            .collect())
    }

    /// Makes source `at`'s next row its head, if it has one.
    fn advance(&mut self, at: usize) -> Result<()> {
        let Some(mut source) = self.sources.get_mut(at).map(|s| std::mem::replace(s, Source::Rows(VecDeque::new()))) else { return Ok(()) };
        let next = self.next_of(&mut source);
        if let Some(slot) = self.sources.get_mut(at) {
            *slot = source;
        }
        if let Some((id, row)) = next? {
            self.heads.push(Head { key: self.key_of(&row), id, row, source: at });
        }
        Ok(())
    }
}

impl Iterator for Ordered {
    type Item = Result<(RowId, Row)>;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.started {
            self.started = true;
            if let Err(e) = self.start() {
                return Some(Err(e));
            }
        }
        let head = self.heads.pop()?;
        if let Err(e) = self.advance(head.source) {
            return Some(Err(e));
        }
        Some(Ok((head.id, head.row)))
    }
}
