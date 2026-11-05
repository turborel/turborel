//! \[DESIGN\] A column store table's rows in the order of the columns its
//! groups are sorted by (`TableStorage::scan_ordered`, RULES.md D31), so an
//! ORDER BY of them needs no sort: each group's rows read in their order,
//! or backward, from its last run, for an ORDER BY of them all descending,
//! a vector at a time, each column decoded into a vector of its values, the
//! runs a condition may match only, those the tree has an entry for left
//! out; the tree's rows the reader sees sorted as ORDER BY sorts them; and
//! all merged, the least next of each, or the greatest backward, by their
//! keys. The merge gives rows, or batches of the columns asked for alone,
//! so a statement read by batches makes no row (`scan_ordered_batches`). A
//! group not sorted by them, one frozen before COLUMNSTORE.ORDER named them
//! and not yet written again, is read whole and sorted. The directory is
//! taken before the tree is read, as a merged scan takes it (`merge.rs`),
//! so a row frozen meanwhile is read once.

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::mem;
use std::sync::Arc;

use super::Directory;
use super::merge::VECTOR_ROWS;
use crate::engine::columns::group::Group;
use crate::engine::columns::scan::{read_candidates, values};
use crate::engine::rows::mvcc::Reader;
use crate::engine::rows::{RowTable, row_id};
use crate::error::Result;
use crate::storage::{BATCH_ROWS, Batch, Batches, Column, Conjunction, Row, RowId};
use crate::value::{Value, sort_cmp};

/// Rows of a source read, by column: each column decoded's values, the
/// rows' ids, and the places among them of the rows to give, in the order
/// given, the next at `at`.
#[derive(Default)]
struct Part {
    values: Vec<Vec<Value>>,
    ids: Vec<RowId>,
    keep: Vec<usize>,
    at: usize,
}

impl Part {
    /// The place of the next row to give, if one is left.
    fn current(&self) -> Option<usize> {
        self.keep.get(self.at).copied()
    }

    /// Row `n`'s values of the columns decoded at `places`.
    fn key(&self, places: &[usize], n: usize) -> Vec<Value> {
        places.iter().map(|&c| self.values.get(c).and_then(|v| v.get(n)).cloned().unwrap_or(Value::Null)).collect()
    }

    /// Row `n`'s value of the column decoded at `c`: a copy if it is to be
    /// given `again`, else taken out.
    fn give(&mut self, c: usize, n: usize, again: bool) -> Value {
        if again { value_of(self, c, n).cloned().unwrap_or(Value::Null) } else { self.take(c, n) }
    }

    /// Row `n`'s value of the column decoded at `c`, taken out.
    fn take(&mut self, c: usize, n: usize) -> Value {
        self.values.get_mut(c).and_then(|v| v.get_mut(n)).map_or(Value::Null, |v| mem::replace(v, Value::Null))
    }
}

/// A sorted group's runs left to read, and its rows' ids.
struct Unread {
    group: Arc<Group>,
    ids: Vec<RowId>,
    runs: VecDeque<(u32, u32)>,
}

/// Where rows come from, each in the order of the keys: the part read
/// last, and of a sorted group, its runs left.
struct Source {
    part: Part,
    runs: Option<Unread>,
}

/// How two rows' keys compare, as ORDER BY sorts them, then their ids;
/// reversed, each descending, if `backward`.
fn by_keys(a: (&[Value], RowId), b: (&[Value], RowId), backward: bool) -> Ordering {
    let ascending = a.0.iter().zip(b.0).map(|(x, y)| sort_cmp(x, y)).find(|o| o.is_ne()).unwrap_or_else(|| a.1.cmp(&b.1));
    if backward { ascending.reverse() } else { ascending }
}

/// \[DESIGN\] A column store table's rows in the order of its sort columns.
pub(in crate::engine::rows) struct Ordered {
    table: Arc<RowTable>,
    reader: Reader,
    directory: Arc<Directory>,
    /// The columns decoded, by their places, in order, each once: those
    /// asked for, the keys and the condition's.
    decoded: Vec<usize>,
    /// The columns asked for, as places among `decoded`, and whether each
    /// is asked for again after, so given as a copy.
    asked: Vec<usize>,
    again: Vec<bool>,
    /// The keys, as places among `decoded`.
    keys: Vec<usize>,
    /// Read backward: each key descending.
    backward: bool,
    condition: Option<Conjunction>,
    /// The ids of the tree's entries, in order: their groups' rows left out.
    shadowed: Vec<RowId>,
    sources: Vec<Source>,
    /// The sources with rows left, as a heap: the first, the source whose
    /// next row is first in order, compared in place (`row_cmp`).
    heads: Vec<usize>,
    started: bool,
}

impl Ordered {
    /// The rows of `table` `reader` sees, of the columns `asked`, only
    /// those `condition` matches if given, in the order of the columns
    /// `keys`, or backward; `directory` its groups, taken before its tree
    /// is read.
    pub(in crate::engine::rows) fn new(
        table: Arc<RowTable>,
        (reader, directory): (Reader, Arc<Directory>),
        (asked, (keys, backward)): (&[usize], (&[usize], bool)),
        condition: Option<Conjunction>,
    ) -> Ordered {
        let mut decoded: Vec<usize> = asked.iter().chain(keys).copied().chain(condition.iter().flat_map(Conjunction::columns)).collect();
        decoded.sort_unstable();
        decoded.dedup();
        let place = |c: &usize| decoded.binary_search(c).unwrap_or_default();
        let (asked, keys): (Vec<usize>, Vec<usize>) = (asked.iter().map(place).collect(), keys.iter().map(place).collect());
        let again = (0..asked.len())
            .map(|i| asked.get(i.saturating_add(1)..).is_some_and(|rest| asked.get(i).is_some_and(|c| rest.contains(c))))
            .collect();
        let (shadowed, sources, heads) = (Vec::new(), Vec::new(), Vec::new());
        Ordered { table, reader, directory, decoded, asked, again, keys, backward, condition, shadowed, sources, heads, started: false }
    }

    /// A row of the table's width, of `values`, of the columns decoded, and
    /// nulls elsewhere.
    fn row_of(&self, values: impl Iterator<Item = Value>) -> Row {
        let mut row = vec![Value::Null; self.table.format.types().len()];
        for (&c, v) in self.decoded.iter().zip(values) {
            if let Some(slot) = row.get_mut(c) {
                *slot = v;
            }
        }
        row
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
                let row = self.table.decode(id, &record, Some(&self.decoded))?;
                if self.matches(&row) {
                    delta.push((id, row));
                }
            }
        }
        let tree = self.part_of_rows(delta);
        self.sources.push(Source { part: tree, runs: None });
        let groups = self.directory.groups.clone();
        for entry in groups.iter().filter(|e| e.summary.may_match(self.condition.as_ref())) {
            let pages = self.table.group_pages()?;
            let group = self.table.group_of(entry)?;
            let ids = group.row_ids(&pages)?;
            let runs: VecDeque<(u32, u32)> = read_candidates(&group, &pages, self.condition.as_ref())?.into_iter().collect();
            let in_order = group.order.starts_with(&self.decoded_keys());
            let mut runs = Unread { group, ids, runs };
            let part = if in_order { Part::default() } else { self.drained(&mut runs)? };
            self.sources.push(Source { part, runs: in_order.then_some(runs) });
        }
        for at in 0..self.sources.len() {
            if self.filled(at)? {
                self.heads.push(at);
                self.sift_up(self.heads.len().saturating_sub(1));
            }
        }
        Ok(())
    }

    /// The keys, by their places in the table.
    fn decoded_keys(&self) -> Vec<usize> {
        self.keys.iter().filter_map(|&k| self.decoded.get(k).copied()).collect()
    }

    /// `rows`, of the columns decoded, by column, in the order of the keys.
    fn part_of_rows(&self, rows: Vec<(RowId, Row)>) -> Part {
        let mut part = Part { values: vec![Vec::with_capacity(rows.len()); self.decoded.len()], ..Part::default() };
        for (id, row) in rows {
            part.ids.push(id);
            for (values, &c) in part.values.iter_mut().zip(&self.decoded) {
                values.push(row.get(c).cloned().unwrap_or(Value::Null));
            }
        }
        part.keep = (0..part.ids.len()).collect();
        self.sort(&mut part);
        part
    }

    /// Orders `part`'s rows kept by their keys, or backward.
    fn sort(&self, part: &mut Part) {
        let mut keep = mem::take(&mut part.keep);
        keep.sort_by(|&a, &b| {
            let id = |n: usize| part.ids.get(n).copied().unwrap_or_default();
            by_keys((&part.key(&self.keys, a), id(a)), (&part.key(&self.keys, b), id(b)), self.backward)
        });
        part.keep = keep;
    }

    /// Every row left of a group sorted otherwise, read and sorted.
    fn drained(&self, runs: &mut Unread) -> Result<Part> {
        let mut all = Part { values: vec![Vec::new(); self.decoded.len()], ..Part::default() };
        while let Some(mut part) = self.read_part(runs)? {
            for n in mem::take(&mut part.keep) {
                all.keep.push(all.ids.len());
                all.ids.push(part.ids.get(n).copied().unwrap_or_default());
                for (c, values) in all.values.iter_mut().enumerate() {
                    values.push(part.take(c, n));
                }
            }
        }
        self.sort(&mut all);
        Ok(all)
    }

    /// The next part of `runs` to read that has rows to give, each column
    /// decoded: of at most a vector of rows, those the tree has no entry
    /// for and the condition matches, in the order read; none once none
    /// is left.
    fn read_part(&self, runs: &mut Unread) -> Result<Option<Part>> {
        let pages = self.table.group_pages()?;
        let types = self.table.format.types();
        while let Some((from, to)) = next_part(&mut runs.runs, self.backward) {
            let mut part = Part::default();
            for &c in &self.decoded {
                let found = types.get(c).map(|t| values(&runs.group, &pages, (c, t), (from, to))).transpose()?;
                part.values.push(found.unwrap_or_default());
            }
            let (first, last) = (usize::try_from(from).unwrap_or(usize::MAX), usize::try_from(to).unwrap_or(usize::MAX));
            part.ids = runs.ids.get(first..last).unwrap_or_default().to_vec();
            part.keep = self.kept(&part);
            if self.backward {
                part.keep.reverse();
            }
            if !part.keep.is_empty() {
                return Ok(Some(part));
            }
        }
        Ok(None)
    }

    /// The places of `part`'s rows the tree has no entry for and the
    /// condition matches, judged of a row of its columns.
    fn kept(&self, part: &Part) -> Vec<usize> {
        let mut scratch = self.condition.as_ref().map(|_| vec![Value::Null; self.table.format.types().len()]);
        let condition_matches = |n: usize, judged: &mut Option<Row>| match judged {
            Some(row) => {
                for (values, &c) in part.values.iter().zip(&self.decoded) {
                    if let (Some(slot), Some(v)) = (row.get_mut(c), values.get(n)) {
                        slot.clone_from(v);
                    }
                }
                self.matches(row)
            }
            None => true,
        };
        (0..part.ids.len())
            .filter(|&n| part.ids.get(n).is_some_and(|id| self.shadowed.binary_search(id).is_err()) && condition_matches(n, &mut scratch))
            .collect()
    }

    /// Whether source `at` has a next row, a group's next part read if its
    /// last has none left.
    fn filled(&mut self, at: usize) -> Result<bool> {
        let Some(mut source) = self.sources.get_mut(at).map(|s| mem::replace(s, Source { part: Part::default(), runs: None })) else {
            return Ok(false);
        };
        let read = match (&mut source.runs, source.part.current()) {
            (Some(runs), None) => self.read_part(runs),
            (Some(_) | None, _) => Ok(None),
        };
        let read = read.map(|part| {
            if let Some(part) = part {
                source.part = part;
            }
        });
        let has = source.part.current().is_some();
        if let Some(slot) = self.sources.get_mut(at) {
            *slot = source;
        }
        read.map(|()| has)
    }

    /// How the next rows of sources `a` and `b` compare, in the order read.
    fn row_cmp(&self, first: usize, second: usize) -> Ordering {
        let (Some(one), Some(other)) = (self.sources.get(first), self.sources.get(second)) else { return Ordering::Equal };
        let (Some(row), Some(other_row)) = (one.part.current(), other.part.current()) else { return Ordering::Equal };
        let id = |part: &Part, at: usize| part.ids.get(at).copied().unwrap_or_default();
        let keys = self.keys.iter().map(|&k| match (value_of(&one.part, k, row), value_of(&other.part, k, other_row)) {
            (Some(value), Some(other_value)) => sort_cmp(value, other_value),
            (Some(_) | None, _) => Ordering::Equal,
        });
        let ascending = keys.into_iter().find(|o| o.is_ne()).unwrap_or_else(|| id(&one.part, row).cmp(&id(&other.part, other_row)));
        if self.backward { ascending.reverse() } else { ascending }
    }

    /// Whether the source at heap place `i` comes before the one at `j`.
    fn before(&self, i: usize, j: usize) -> bool {
        let at = |p: usize| self.heads.get(p).copied().unwrap_or_default();
        self.row_cmp(at(i), at(j)) == Ordering::Less
    }

    /// Moves the source at heap place `i` up to its place.
    fn sift_up(&mut self, mut i: usize) {
        while i > 0 {
            let parent = i.saturating_sub(1) >> 1_u32;
            if !self.before(i, parent) {
                break;
            }
            self.heads.swap(i, parent);
            i = parent;
        }
    }

    /// Moves the source at heap place `i` down to its place.
    fn sift_down(&mut self, mut i: usize) {
        loop {
            let left = i.saturating_mul(2).saturating_add(1);
            let first = [left, left.saturating_add(1)]
                .into_iter()
                .filter(|&c| c < self.heads.len())
                .fold(i, |first, c| if self.before(c, first) { c } else { first });
            if first == i {
                return;
            }
            self.heads.swap(i, first);
            i = first;
        }
    }

    /// The next row in order: its source, and its place in the source's
    /// part, whose values are taken before the source moves on
    /// (`moved_on`).
    fn next_place(&mut self) -> Result<Option<(usize, usize, RowId)>> {
        if !self.started {
            self.started = true;
            self.start()?;
        }
        let Some(&at) = self.heads.first() else { return Ok(None) };
        let place = self.sources.get(at).and_then(|s| s.part.current().map(|n| (n, s.part.ids.get(n).copied().unwrap_or_default())));
        Ok(place.map(|(n, id)| (at, n, id)))
    }

    /// Moves source `at`, the first of the heap, on past the row it gave:
    /// to its place by its next row, or out, of none.
    fn moved_on(&mut self, at: usize) -> Result<()> {
        if let Some(source) = self.sources.get_mut(at) {
            source.part.at = source.part.at.saturating_add(1);
        }
        if !self.filled(at)? {
            self.heads.swap_remove(0);
        }
        self.sift_down(0);
        Ok(())
    }

    /// The next row, of the table's width.
    fn next_row(&mut self) -> Result<Option<(RowId, Row)>> {
        let Some((at, n, id)) = self.next_place()? else { return Ok(None) };
        let taken: Vec<Value> = match self.sources.get_mut(at) {
            Some(source) => (0..self.decoded.len()).map(|c| source.part.take(c, n)).collect(),
            None => Vec::new(),
        };
        self.moved_on(at)?;
        Ok(Some((id, self.row_of(taken.into_iter()))))
    }

    /// The next batch of up to `BATCH_ROWS` rows, of the columns asked for,
    /// in order; none once none is left.
    fn next_batch(&mut self) -> Result<Option<Batch>> {
        let mut columns: Vec<Vec<Value>> = vec![Vec::with_capacity(BATCH_ROWS); self.asked.len()];
        let mut rows = 0;
        while rows < BATCH_ROWS {
            let Some((at, n, _)) = self.next_place()? else { break };
            if let Some(source) = self.sources.get_mut(at) {
                for (out, (&c, &again)) in columns.iter_mut().zip(self.asked.iter().zip(&self.again)) {
                    out.push(source.part.give(c, n, again));
                }
            }
            self.moved_on(at)?;
            rows = rows.saturating_add(1);
        }
        Ok((rows > 0).then(|| Batch { columns: columns.into_iter().map(Column::Values).collect(), rows }))
    }

    /// Its rows by batch, of the columns asked for.
    pub(in crate::engine::rows) fn batches(mut self) -> Batches {
        Box::new(std::iter::from_fn(move || self.next_batch().transpose()))
    }
}

/// Row `r`'s value of the column decoded at `k` of `part`.
fn value_of(part: &Part, k: usize, r: usize) -> Option<&Value> {
    part.values.get(k).and_then(|v| v.get(r))
}

/// The next part of `runs` to read, of at most `VECTOR_ROWS` rows: the
/// first of the first run, or the last of the last, `backward`; the rest
/// of its run left in `runs`.
pub(super) fn next_part(runs: &mut VecDeque<(u32, u32)>, backward: bool) -> Option<(u32, u32)> {
    let most = u32::try_from(VECTOR_ROWS).unwrap_or(u32::MAX);
    if backward {
        let (from, to) = runs.pop_back()?;
        let start = to.saturating_sub(most).max(from);
        if from < start {
            runs.push_back((from, start));
        }
        Some((start, to))
    } else {
        let (from, to) = runs.pop_front()?;
        let end = from.saturating_add(most).min(to);
        if end < to {
            runs.push_front((end, to));
        }
        Some((from, end))
    }
}

impl Iterator for Ordered {
    type Item = Result<(RowId, Row)>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_row().transpose()
    }
}
