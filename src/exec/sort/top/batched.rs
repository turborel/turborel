//! [ISO/IEC 9075-2:2023 §7.17 GR5, GR7] \[DESIGN\] RULES.md D365: a sort of
//! which only the first rows are asked for, of a stream's batches each
//! holding every column of its rows. Each batch's rows' normal keys are
//! written of its vectors (`normal.rs`), of an open layout made of the
//! first batch, and only a row whose key is below the greatest held, once
//! `most` are held, is taken, so no row is made of a batch but those given.
//! A batch is held while a row of it is, and once those held are more than
//! twice what is asked, each batch of which fewer than a quarter are held
//! is gathered to those rows alone, and past the work area every batch is.
//! Rows of equal keys of strings longer
//! than their keys hold are compared whole, and ties go to the row that
//! came first, as a whole sort's do; of WITH TIES, the peers of the
//! greatest held are held too. A batch whose keys the layout does not
//! admit, or rows held past the work area, end it. Then the rows held, in
//! the order they came, and the rest are sorted as a whole sort sorts them.

use crate::kernels::Kernels;
use std::cmp::Ordering;
use std::sync::Arc;

use super::super::Work;
use super::super::normal::{Layout, Shape, encode_batch, seen_column};
use super::super::vectors::{Held, compare_cells, row_of, sorted};
use crate::error::Result;
use crate::exec::batch::{Batches, resolved};
use crate::exec::{RowIter, broken};
use crate::logical::{Most, SortKey};
use crate::storage::{BATCH_ROWS, Batch, Threshold};

/// \[DESIGN\] The fewest rows of a batch that is gathered to those held:
/// fewer cost little to keep.
const GATHER_LEAST: usize = 64;

/// \[DESIGN\] A batch is gathered once fewer than one in this many of its rows
/// are held.
const GATHER_SHARE: usize = 4;

/// \[DESIGN\] The bytes a held row takes beside its key: its places.
const ENTRY_BYTES: usize = 32;

/// A row held: its key, its batch's place, its place there, and the place
/// it came in.
#[derive(Clone)]
struct Entry {
    key: Box<[u8]>,
    batch: usize,
    row: usize,
    seq: u64,
}

/// The rows held and their batches.
struct Top<'k> {
    keys: &'k [SortKey],
    most: Most,
    layout: Option<Layout>,
    batches: Vec<Option<Batch>>,
    /// The rows held of each batch.
    live: Vec<usize>,
    /// The first `most` rows, a heap whose greatest is first.
    heap: Vec<Entry>,
    /// Of WITH TIES, the peers of the greatest of the heap.
    peers: Vec<Entry>,
    seq: u64,
    /// The bound of the first key told the scan, raised as rows are held
    /// (RULES.md D379).
    bound: Option<Arc<Threshold>>,
    /// The kernels its keys are written on (RULES.md D354).
    kernels: Arc<dyn Kernels>,
}

/// The first `most` rows of `input`, a stream's batches each holding every
/// column of its rows, its columns not read by `keys` late if the scan gave
/// them so (RULES.md D380), sorted by `keys`, in a work area of `work`, and
/// of WITH TIES the peers of the last after it; of more, the rest after
/// them, sorted, once those held outgrow `work`. The bound of the first
/// key, `bound`, is raised as rows are held, if given (D379).
///
/// # Errors
///
/// The input's, a row of no key column's, a late column's decoding, or a
/// spill's.
pub(in crate::exec::sort) fn top_batches(
    keys: &[SortKey],
    mut input: Batches,
    (most, bound): (Most, Option<Arc<Threshold>>),
    work: &mut Work,
    kernels: Arc<dyn Kernels>,
) -> Result<RowIter> {
    let (batches, live, heap, peers) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut top = Top { keys, most, layout: None, batches, live, heap, peers, seq: 0, bound, kernels };
    while let Some(kept) = input.next() {
        let (batch, rows) = kept?;
        let rows = crate::storage::sel::places_of(rows, batch.rows);
        if let Some(k) = keys.iter().find(|k| k.column >= batch.columns.len()).filter(|_| !rows.is_empty()) {
            return Err(broken(&format!("sort column {} is not in the rows sorted", k.column)));
        }
        top.batches.push(Some(batch));
        top.live.push(0);
        if !top.take(&rows) {
            let last = top.batches.len().saturating_sub(1);
            return top.whole((last, rows), input, work);
        }
        top.settle();
        top.raise();
        if top.bytes() > work.limit() {
            // Past the work area, every batch is gathered to its rows held
            // before the whole sort is.
            top.gather(true);
            if top.bytes() > work.limit() {
                return top.whole_held(input, work);
            }
        }
    }
    let mut all = std::mem::take(&mut top.heap);
    all.append(&mut top.peers);
    all.sort_by(|a, b| top.order(a, b));
    let mut kept = top.batches;
    resolve_held(&mut kept, &all)?;
    Ok(Box::new(all.into_iter().filter_map(move |e| kept.get(e.batch)?.as_ref().map(|b| Ok(row_of(b, e.row))))))
}

/// Decodes the late columns of each of `batches` of the rows of `entries`
/// held of it alone (RULES.md D380).
///
/// # Errors
///
/// As decoding a late column fails.
fn resolve_held(batches: &mut [Option<Batch>], entries: &[Entry]) -> Result<()> {
    let mut rows: Vec<Vec<u32>> = vec![Vec::new(); batches.len()];
    for e in entries {
        if let Some(of) = rows.get_mut(e.batch) {
            of.push(u32::try_from(e.row).unwrap_or(u32::MAX));
        }
    }
    for (batch, mut of) in batches.iter_mut().zip(rows) {
        if let Some(batch) = batch.as_mut().filter(|b| b.has_late()) {
            of.sort_unstable();
            batch.resolve(Some(&of))?;
        }
    }
    Ok(())
}

impl Top<'_> {
    /// Takes those of the last batch's rows at `rows` among the first that
    /// are; false if its keys cannot be written.
    fn take(&mut self, rows: &[usize]) -> bool {
        let at = self.batches.len().saturating_sub(1);
        let Some(Some(batch)) = self.batches.get(at) else { return false };
        let found: Vec<_> = self.keys.iter().map(|k| seen_column(batch, k.column, rows)).collect();
        if self.layout.is_none() {
            self.layout = Layout::new(self.keys, &found, Shape::Open);
        }
        let Some(layout) = self.layout.as_ref().filter(|l| l.admits(&found)) else { return false };
        let (width, truncates) = (layout.width, layout.truncates);
        let mut keys = vec![0_u8; rows.len().saturating_mul(width)];
        if encode_batch((layout, &*self.kernels), batch, rows, &mut keys).is_none() {
            return false;
        }
        for (n, &row) in rows.iter().enumerate() {
            let key = keys.get(n.saturating_mul(width)..n.saturating_add(1).saturating_mul(width)).unwrap_or_default();
            let seq = self.seq;
            self.seq = self.seq.saturating_add(1);
            // A row whose key is above the greatest held's, or equal and
            // not to be compared whole nor held as a peer, is not taken:
            // so most rows of a long input are passed over here.
            let full = self.heap.len() >= self.most.rows;
            if let Some(greatest) = self.heap.first().filter(|_| full) {
                let by_key = key.cmp(&greatest.key);
                if by_key.is_gt() || (by_key.is_eq() && !truncates && !self.most.ties) {
                    continue;
                }
            }
            self.add(Entry { key: key.into(), batch: at, row, seq });
        }
        true
    }

    /// The order of `a` and `b`, of the batches held: by key, by their
    /// values if keys of strings longer than they hold tie, then by the
    /// order they came.
    fn order(&self, a: &Entry, b: &Entry) -> Ordering {
        self.ordered(a, b).then(a.seq.cmp(&b.seq))
    }

    /// The order of `a` and `b` by key and values.
    fn ordered(&self, a: &Entry, b: &Entry) -> Ordering {
        let by_key = a.key.cmp(&b.key);
        if by_key.is_ne() || !self.layout.as_ref().is_some_and(|l| l.truncates) {
            return by_key;
        }
        let batch = |e: &Entry| self.batches.get(e.batch).and_then(Option::as_ref);
        batch(a).zip(batch(b)).map_or(Ordering::Equal, |(x, y)| compare_cells(self.keys, (x, a.row), (y, b.row)))
    }

    /// Takes `entry` among the first if it is, giving up the greatest for
    /// it; else, of WITH TIES, among the peers if it is one.
    fn add(&mut self, entry: Entry) {
        let mut heap = std::mem::take(&mut self.heap);
        if heap.len() < self.most.rows {
            self.count(entry.batch);
            heap.push(entry);
            let last = heap.len().saturating_sub(1);
            sift_up(&mut heap, last, &|a, b| self.order(a, b));
            self.heap = heap;
            return;
        }
        let placed = heap.first().map(|greatest| self.ordered(&entry, greatest));
        match (placed, self.most.ties) {
            (Some(Ordering::Less), _) => {
                self.count(entry.batch);
                let Some(first) = heap.first_mut() else { return };
                let given_up = std::mem::replace(first, entry);
                sift_down(&mut heap, 0, &|a, b| self.order(a, b));
                self.heap = heap;
                self.after(given_up);
                return;
            }
            (Some(Ordering::Equal), true) => {
                self.count(entry.batch);
                self.peers.push(entry);
            }
            (None | Some(Ordering::Equal | Ordering::Greater), _) => {}
        }
        self.heap = heap;
    }

    /// Raises the bound told the scan to the first key of the greatest
    /// held, once as many rows as are asked are held. A row whose first key
    /// is past it is past the greatest whatever its other keys.
    fn raise(&self) {
        let Some(bound) = self.bound.as_ref().filter(|_| self.heap.len() >= self.most.rows) else { return };
        let (Some(greatest), Some(key)) = (self.heap.first(), self.keys.first()) else { return };
        if let Some(Some(batch)) = self.batches.get(greatest.batch) {
            bound.raise(&batch.cell(key.column, greatest.row).to_value());
        }
    }

    /// Counts a row more held of `batch`.
    fn count(&mut self, batch: usize) {
        if let Some(n) = self.live.get_mut(batch) {
            *n = n.saturating_add(1);
        }
    }

    /// `given_up`, once the greatest held: a peer of the greatest now, of
    /// WITH TIES, else let go, with the peers, if the greatest is no more
    /// their peer.
    fn after(&mut self, given_up: Entry) {
        let peer = self.most.ties && self.heap.first().is_some_and(|g| self.ordered(&given_up, g).is_eq());
        if peer {
            self.peers.push(given_up);
            return;
        }
        let gone: Vec<usize> = std::iter::once(given_up.batch).chain(self.peers.drain(..).map(|p| p.batch)).collect();
        for batch in gone {
            if let Some(n) = self.live.get_mut(batch) {
                *n = n.saturating_sub(1);
            }
        }
    }

    /// Lets go of the batches no row held is of, and gathers those of
    /// which few are, if the rows of the batches held are many.
    fn settle(&mut self) {
        for (batch, live) in self.batches.iter_mut().zip(&self.live) {
            if *live == 0 {
                *batch = None;
            }
        }
        let held = self.batches.iter().flatten().fold(0_usize, |n, b| n.saturating_add(b.rows));
        if held > self.most.rows.saturating_mul(2).saturating_add(BATCH_ROWS) {
            self.gather(false);
        }
    }

    /// Gathers each batch of which fewer than one in `GATHER_SHARE` rows
    /// are held, or of `all` each of which some are not, to those rows, in
    /// their order, each row held moved to its place among them.
    fn gather(&mut self, all: bool) {
        for at in 0..self.batches.len() {
            let live = self.live.get(at).copied().unwrap_or(0);
            let Some(Some(batch)) = self.batches.get(at) else { continue };
            let few = batch.rows < GATHER_LEAST || live.saturating_mul(GATHER_SHARE) >= batch.rows;
            if live >= batch.rows || (few && !all) {
                continue;
            }
            let mut rows: Vec<usize> = self.heap.iter().chain(&self.peers).filter(|e| e.batch == at).map(|e| e.row).collect();
            rows.sort_unstable();
            let picks: Vec<Option<usize>> = rows.iter().copied().map(Some).collect();
            let gathered = Batch { columns: batch.columns.iter().map(|c| c.gathered(&picks)).collect(), rows: rows.len() };
            for e in self.heap.iter_mut().chain(self.peers.iter_mut()).filter(|e| e.batch == at) {
                e.row = rows.binary_search(&e.row).unwrap_or(0);
            }
            if let Some(slot) = self.batches.get_mut(at) {
                *slot = Some(gathered);
            }
        }
    }

    /// The bytes the batches and rows held take.
    fn bytes(&self) -> usize {
        let width = self.layout.as_ref().map_or(0, |l| l.width).saturating_add(ENTRY_BYTES);
        let rows = self.heap.len().saturating_add(self.peers.len()).saturating_mul(width);
        self.batches.iter().flatten().fold(rows, |n, b| n.saturating_add(b.bytes()))
    }

    /// The rows held, in the order they came, then the rows at `rows` of
    /// the last batch, `last`, whose keys could not be written, then the
    /// rest of `input`, sorted as a whole sort sorts them.
    fn whole(mut self, (last, rows): (usize, Vec<usize>), input: Batches, work: &mut Work) -> Result<RowIter> {
        let mut held = self.held_in_order(last)?;
        if let Some(Some(mut batch)) = self.batches.get_mut(last).map(Option::take) {
            resolve_at(&mut batch, &rows)?;
            held.add(batch, rows);
        }
        sorted(self.keys, resolved(input), held, work)
    }

    /// The rows held, in the order they came, then the rest of `input`,
    /// sorted as a whole sort sorts them.
    fn whole_held(mut self, input: Batches, work: &mut Work) -> Result<RowIter> {
        let held = self.held_in_order(self.batches.len())?;
        sorted(self.keys, resolved(input), held, work)
    }

    /// The rows held of the batches before `end`, in the order they came,
    /// their late columns decoded of them.
    ///
    /// # Errors
    ///
    /// As decoding a late column fails.
    fn held_in_order(&mut self, end: usize) -> Result<Held> {
        let mut entries: Vec<Entry> = self.heap.drain(..).chain(self.peers.drain(..)).filter(|e| e.batch < end).collect();
        entries.sort_by_key(|e| e.seq);
        let mut held = Held::on(Arc::clone(&self.kernels));
        for (at, batch) in self.batches.iter_mut().enumerate().take(end) {
            let rows: Vec<usize> = entries.iter().filter(|e| e.batch == at).map(|e| e.row).collect();
            if let Some(mut batch) = batch.take().filter(|_| !rows.is_empty()) {
                resolve_at(&mut batch, &rows)?;
                held.add(batch, rows);
            }
        }
        Ok(held)
    }
}

/// Decodes `batch`'s late columns of the rows at `rows` alone, if it has
/// any (RULES.md D380).
///
/// # Errors
///
/// As decoding a late column fails.
fn resolve_at(batch: &mut Batch, rows: &[usize]) -> Result<()> {
    if !batch.has_late() {
        return Ok(());
    }
    let mut at: Vec<u32> = rows.iter().map(|&r| u32::try_from(r).unwrap_or(u32::MAX)).collect();
    at.sort_unstable();
    batch.resolve(Some(&at))
}

/// Moves `heap`'s entry at `at` up to its place in the heap whose
/// greatest is first, by `order`.
fn sift_up(heap: &mut [Entry], mut at: usize, order: &impl Fn(&Entry, &Entry) -> Ordering) {
    while at > 0 {
        let parent = at.saturating_sub(1).checked_div(2).unwrap_or(0);
        let up = heap.get(at).zip(heap.get(parent)).is_some_and(|(e, p)| order(e, p).is_gt());
        if !up {
            return;
        }
        heap.swap(at, parent);
        at = parent;
    }
}

/// Moves `heap`'s entry at `at` down to its place, by `order`.
fn sift_down(heap: &mut [Entry], mut at: usize, order: &impl Fn(&Entry, &Entry) -> Ordering) {
    loop {
        let left = at.saturating_mul(2).saturating_add(1);
        let mut greatest = at;
        for child in [left, left.saturating_add(1)] {
            if heap.get(child).zip(heap.get(greatest)).is_some_and(|(c, g)| order(c, g).is_gt()) {
                greatest = child;
            }
        }
        if greatest == at {
            return;
        }
        heap.swap(at, greatest);
        at = greatest;
    }
}
