//! \[DESIGN\] Rows grouped by their dictionaries' codes
//! (RULES.md D1161). A column store gives a character string column's
//! rows as codes of a dictionary its batches share (`storage::Coded`).
//! Grouping by such columns alone, each row's codes, a null's a place of
//! its own, are its place in a memo of the groups its codes have gone to.
//! A row whose place has a group goes to it, no key hashed, decoded or
//! compared. The first row of a place is hashed and its group found or
//! made by its key as any row's is, so two codes of texts that are equal
//! as keys, as texts differing in their trailing spaces, find one group,
//! and a group's text is copied once, as it is made. The memo holds while
//! the batches come of the same dictionaries, and is made anew when one
//! differs. A grouping by a column that is not coded, or of more places
//! than `MOST_PLACES`, is grouped as before.

use std::sync::Arc;

use crate::exec::table::{GroupTable, Key, Seed, hash_rows};
use crate::kernels::Kernels;
use crate::storage::sel::nth;
use crate::storage::{Batch, Column, Texts};

/// \[DESIGN\] The most places a memo holds, a group's number each: 65,536
/// of four bytes, 256 KiB, an eighth of the 2 MiB a performance core of the
/// benchmark machine caches next to it, so the memo stays there beside the
/// batch's vectors. A dictionary of a chunk holds at most the chunk's
/// distinct values, so a grouping by one or two columns of few values, the
/// case this is for, takes a small part of it.
const MOST_PLACES: usize = 1 << 16;

/// No group yet, in a memo's place.
const NONE: u32 = u32::MAX;

/// \[DESIGN\] The groups the codes of a morsel's rows have gone to, of the
/// dictionaries `entries`, a column's codes times its `strides` each.
#[derive(Default)]
pub(super) struct Memo {
    entries: Vec<Arc<Texts>>,
    strides: Vec<usize>,
    groups: Vec<u32>,
    /// Of the batch being grouped, each row's place, made a column at a
    /// time (RULES.md D1120).
    places: Vec<usize>,
}

/// The coded texts of `column`, if its rows are codes of a dictionary.
fn coded(column: Option<&Column>) -> Option<&Texts> {
    match column? {
        Column::Texts(texts) => texts.coded.is_some().then_some(texts),
        Column::Late(late) => coded(late.made()),
        Column::Values(_) | Column::Ints(_) | Column::Floats(_) | Column::Datetimes(_) | Column::Bools(_) | Column::Nulls(_) => None,
    }
}

impl Memo {
    /// Readies it for the rows `rows`, or all, of `batch`, grouped by its
    /// columns `at`, each row placed: whether every column is coded and
    /// their places are few enough.
    pub(super) fn ready(&mut self, batch: &Batch, at: &[usize], rows: Option<&[u32]>) -> bool {
        let ready = self.holds(batch, at);
        if ready {
            self.placed(batch, at, rows);
        }
        ready
    }

    /// Whether every column of `batch` at `at` is coded and their places
    /// are few enough. Of dictionaries other than its own, it is made anew.
    fn holds(&mut self, batch: &Batch, at: &[usize]) -> bool {
        let mut texts = Vec::with_capacity(at.len());
        for &c in at {
            let Some(t) = coded(batch.columns.get(c)) else { return false };
            texts.push(t);
        }
        let entries = texts.iter().filter_map(|t| t.coded.as_ref().map(|c| &c.entries));
        let same = self.entries.len() == at.len() && entries.clone().zip(&self.entries).all(|(a, b)| Arc::ptr_eq(a, b));
        if same {
            return true;
        }
        let (mut strides, mut places) = (Vec::with_capacity(at.len()), 1_usize);
        for e in entries.clone() {
            strides.push(places);
            // Each code of the dictionary, and a null's place after them.
            let Some(wider) = places.checked_mul(e.len().saturating_add(1)) else { return false };
            if wider > MOST_PLACES {
                return false;
            }
            places = wider;
        }
        self.entries = entries.cloned().collect();
        self.strides = strides;
        self.groups.clear();
        self.groups.resize(places, NONE);
        true
    }

    /// \[DESIGN\] (RULES.md D1120) Makes the place of each row of `rows`,
    /// or all, of `batch`'s columns `at`, ready for them: a column at a
    /// time, each its codes read in one loop, a null's place past its
    /// dictionary's codes; past the memo's places of a code past them.
    fn placed(&mut self, batch: &Batch, at: &[usize], rows: Option<&[u32]>) {
        let count = rows.map_or(batch.rows, <[u32]>::len);
        self.places.clear();
        self.places.resize(count, 0);
        for ((&c, &stride), entries) in at.iter().zip(&self.strides).zip(&self.entries) {
            let codes = coded(batch.columns.get(c)).and_then(|texts| Some((texts, texts.coded.as_ref()?.codes.as_slice())));
            let Some((texts, codes)) = codes else {
                // No place of a column not coded, as none is below the
                // memo's.
                self.places.fill(usize::MAX);
                return;
            };
            // A code past the codes read has no place.
            let code = |r: usize| {
                let entry = codes.get(r).map_or(usize::MAX, |&e| usize::try_from(e).unwrap_or(usize::MAX));
                if texts.is_null(r) { entries.len() } else { entry }
            };
            for (n, place) in self.places.iter_mut().enumerate() {
                *place = place.saturating_add(code(nth(rows, n)).saturating_mul(stride));
            }
        }
    }

    /// Of row `n` of those placed, its place and the group its codes went
    /// to, if one did.
    pub(super) fn found(&self, n: usize) -> (Option<usize>, Option<u32>) {
        let place = self.places.get(n).copied().filter(|&p| p < self.groups.len());
        (place, place.and_then(|p| self.groups.get(p)).copied().filter(|&g| g != NONE))
    }

    /// Notes that the codes of `place` go to group `g`.
    pub(super) fn note(&mut self, place: Option<usize>, g: u32) {
        if let Some(slot) = place.and_then(|p| self.groups.get_mut(p)) {
            *slot = g;
        }
    }

    /// Forgets every group noted, as the table's groups are moved.
    pub(super) fn forget(&mut self) {
        self.groups.fill(NONE);
    }
}

/// The hash of the key of row `r` of `batch`'s columns `at`, of `seed`,
/// made in `hashes`.
pub(super) fn hash_of(batch: &Batch, (at, r): (&[usize], usize), (seed, kernels): (&Seed, &dyn Kernels), hashes: &mut Vec<u64>) -> u64 {
    let one = [u32::try_from(r).unwrap_or(u32::MAX)];
    hash_rows(batch, (at, Some(&one)), (seed, kernels), hashes);
    hashes.first().copied().unwrap_or(0)
}

/// Groups the rows `rows`, or all, of `batch` by its columns `at` into
/// `table` by their codes, as `Memo` says, each its first row numbered
/// from `first`, hashed of `seed`: false, nothing done, if they are not
/// all coded or their places are too many.
pub(super) fn grouped(
    (table, memo): (&mut GroupTable, &mut Memo),
    (batch, at, rows): (&Batch, &[usize], Option<&[u32]>),
    (first, seed, kernels): (u64, &Seed, &dyn Kernels),
    hashes: &mut Vec<u64>,
) -> bool {
    if !memo.ready(batch, at, rows) {
        return false;
    }
    for n in 0..rows.map_or(batch.rows, <[u32]>::len) {
        let r = nth(rows, n);
        let (place, known) = memo.found(n);
        let g = known.unwrap_or_else(|| {
            let hash = hash_of(batch, (at, r), (seed, kernels), hashes);
            let g = table.found_or_made(hash, Key::Row(batch, at, r), first.saturating_add(u64::try_from(n).unwrap_or(u64::MAX)));
            memo.note(place, g);
            g
        });
        table.take(g, (n, r));
    }
    true
}

#[cfg(test)]
mod tests;
