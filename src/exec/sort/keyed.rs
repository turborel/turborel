//! \[DESIGN\] RULES.md D364: rows held sorted by their normal keys
//! (`normal.rs`). What each key's values are is found over every row held,
//! the layout made of it, each row's key written, a batch's vectors at a
//! time, and the rows' places put in the keys' order (`normal/order.rs`).
//! A sort held whole in memory makes its layout as narrow as its rows
//! allow; one that spills makes it of its first run, open to later ones,
//! and shares it with every run it admits, so the merge orders rows of two
//! runs by their keys (`merge.rs`). A run it does not admit has its own
//! layout, numbered apart. Rows whose keys have no normal form are sorted
//! as before, by their first key's prefix, then their values
//! (`prefix.rs`).

use crate::kernels::Kernels;
use std::cmp::Ordering;
use std::sync::Arc;

use super::compare_rows;
use super::normal::{Found, Layout, Shape, encode_batch, encode_row, order, seen_as, seen_column, seen_rows, with};
use super::prefix::{self, joined, prefix_of_cell};
use super::vectors::compare_cells;
use crate::logical::SortKey;
use crate::storage::{Batch, Row};

/// The number of a spilled sort's layout its runs share.
const SHARED: u32 = 1;

/// The layouts of a sort's runs: the one they share, if made, and the
/// number of the next of a run's own.
#[derive(Default)]
pub(super) struct Layouts {
    shared: Option<Arc<Layout>>,
    next: u32,
}

impl Layouts {
    /// The layout of a run whose keys were found as `found` says, and its
    /// number: the shared one if it admits them, else one of its own,
    /// the first shared. None if they have no normal form.
    pub(super) fn of(&mut self, keys: &[SortKey], found: &[Option<Found>]) -> Option<(Arc<Layout>, u32)> {
        if let Some(shared) = self.shared.as_ref().filter(|l| l.admits(found)) {
            return Some((Arc::clone(shared), SHARED));
        }
        let layout = Arc::new(Layout::new(keys, found, Shape::Open)?);
        if self.shared.is_none() {
            self.shared = Some(Arc::clone(&layout));
            return Some((layout, SHARED));
        }
        self.next = self.next.max(SHARED).saturating_add(1);
        Some((layout, self.next))
    }
}

/// Rows' keys as written: their layout, its number, and the keys, one
/// after another in the rows' order before the sort.
pub(super) struct Made {
    pub(super) layout: Arc<Layout>,
    pub(super) number: u32,
    pub(super) bytes: Vec<u8>,
}

impl Made {
    /// The key of the row at `place` before the sort.
    pub(super) fn key(&self, place: u32) -> &[u8] {
        let from = usize::try_from(place).unwrap_or(usize::MAX).saturating_mul(self.layout.width);
        self.bytes.get(from..from.saturating_add(self.layout.width)).unwrap_or_default()
    }
}

/// The order of the rows of `batches` at `places`, each a batch's place and
/// a row's in it, sorted by `keys` [X3.135-1986 §8.3 GR3b], stably: the
/// places of `places` in order, and the keys made, if they were. Of
/// `layouts`, a run's of a spilled sort; of none, a sort held whole.
pub(super) fn sorted_places(
    keys: &[SortKey],
    batches: &[(Batch, Vec<usize>)],
    places: &[(u32, u32)],
    layouts: Option<&mut Layouts>,
    kernels: &dyn Kernels,
) -> (Vec<u32>, Option<Made>) {
    let compare = |a: u32, b: u32| cells_of(batches, places, a, b, keys);
    if let Some(made) = made_of(keys, batches, layouts, kernels)
        && let Some(order) = order((&made.bytes, places.len()), made.layout.width, made.layout.truncates, compare)
    {
        return (order, Some(made));
    }
    (by_prefix(keys, batches, places), None)
}

/// The keys of the rows of `batches`, each at its places, of a layout of
/// `layouts`, or narrow if none, written on `kernels`; none if they have
/// no normal form.
pub(super) fn made_of(
    keys: &[SortKey],
    batches: &[(Batch, Vec<usize>)],
    layouts: Option<&mut Layouts>,
    kernels: &dyn Kernels,
) -> Option<Made> {
    let found: Vec<Option<Found>> = keys
        .iter()
        .map(|k| {
            batches.iter().try_fold(Found::default(), |f, (batch, rows)| with(Some(f), seen_as(k, seen_column(batch, k.column, rows))))
        })
        .collect();
    let (layout, number) = match layouts {
        Some(layouts) => layouts.of(keys, &found)?,
        None => (Arc::new(Layout::new(keys, &found, Shape::Tight)?), SHARED),
    };
    let count = batches.iter().fold(0_usize, |n, (_, rows)| n.saturating_add(rows.len()));
    let mut bytes = vec![0_u8; count.checked_mul(layout.width)?];
    let mut from = 0_usize;
    for (batch, rows) in batches {
        let to = from.checked_add(rows.len().checked_mul(layout.width)?)?;
        encode_batch((&layout, kernels), batch, rows, bytes.get_mut(from..to)?)?;
        from = to;
    }
    Some(Made { layout, number, bytes })
}

/// The order of rows `a` and `b` of `places`, of `batches`, under `keys`.
fn cells_of(batches: &[(Batch, Vec<usize>)], places: &[(u32, u32)], a: u32, b: u32, keys: &[SortKey]) -> Ordering {
    let cell = |p: u32| {
        let &(batch, row) = places.get(usize::try_from(p).ok()?)?;
        Some((&batches.get(usize::try_from(batch).ok()?)?.0, usize::try_from(row).ok()?))
    };
    cell(a).zip(cell(b)).map_or(Ordering::Equal, |(x, y)| compare_cells(keys, x, y))
}

/// The order of the rows at `places` of `batches` sorted as before keys
/// were normal, stably: by their first key's prefix, if its values have
/// them, ties by the keys' values; else by the values alone.
fn by_prefix(keys: &[SortKey], batches: &[(Batch, Vec<usize>)], places: &[(u32, u32)]) -> Vec<u32> {
    let (mut kind, mut whole) = (Some(prefix::Kind::Null), true);
    let mut prefixed: Vec<(u64, u32)> = Vec::with_capacity(places.len());
    for (&(batch, row), place) in places.iter().zip(0_u32..) {
        // A key by octets has no prefix here, so its rows are compared whole
        // (RULES.md D1185).
        let p = keys.first().filter(|k| !k.octets).and_then(|k| {
            let (b, _) = batches.get(usize::try_from(batch).ok()?)?;
            prefix_of_cell(b.cell(k.column, usize::try_from(row).ok()?), k.order())
        });
        kind = kind.zip(p).and_then(|(held, p)| joined(held, p.kind));
        whole &= p.is_some_and(|p| p.whole);
        prefixed.push((p.map_or(0, |p| p.bits), place));
    }
    let all = keys;
    let ties = if kind.is_none() {
        for p in &mut prefixed {
            p.0 = 0;
        }
        all
    } else if whole {
        keys.get(1..).unwrap_or_default()
    } else {
        all
    };
    prefixed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| cells_of(batches, places, a.1, b.1, ties)));
    prefixed.into_iter().map(|(_, place)| place).collect()
}

/// `rows`, sorted by `keys` [X3.135-1986 §8.3 GR3b], stably: by their
/// normal keys, made as narrow as they allow, or as before if they have
/// none (`prefix::sorted`).
pub(in crate::exec) fn sorted_rows(keys: &[SortKey], rows: Vec<Row>) -> Vec<Row> {
    let found = seen_rows(keys, &rows);
    let Some(order) = Layout::new(keys, &found, Shape::Tight).and_then(|layout| {
        let mut bytes = vec![0_u8; rows.len().checked_mul(layout.width)?];
        if layout.width > 0 {
            for (key, row) in bytes.chunks_exact_mut(layout.width).zip(&rows) {
                encode_row(&layout, row, key)?;
            }
        }
        let compare = |a: u32, b: u32| {
            let row = |p: u32| rows.get(usize::try_from(p).unwrap_or(usize::MAX));
            row(a).zip(row(b)).map_or(Ordering::Equal, |(x, y)| compare_rows(keys, x, y))
        };
        order((&bytes, rows.len()), layout.width, layout.truncates, compare)
    }) else {
        return prefix::sorted(keys, rows);
    };
    let mut slots: Vec<Option<Row>> = rows.into_iter().map(Some).collect();
    order.iter().filter_map(|&at| slots.get_mut(usize::try_from(at).ok()?).and_then(Option::take)).collect()
}
