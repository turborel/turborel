//! \[DESIGN\] A packed load held in memory (RULES.md D139): the entries
//! gathered into a vector reserved of the memory budget while it grants
//! the room, and tiled from the root down in place, each slab and each
//! run a slice sorted on its own, so no record is encoded, written to a
//! work area or read back. Where the budget stops granting, the entries
//! gathered go to the sorted spill ahead of the rest, so a load the
//! budget cannot hold costs what one never held would.

use std::cmp::Ordering;
use std::sync::Arc;

use super::super::node::Entry;
use super::{Packer, X, Y, centre};
use crate::error::Result;
use crate::memory::{MemoryBudget, Reservation};
use crate::spatial::{Bounds, DIMENSIONS};
use crate::storage::RowId;

/// \[DESIGN\] The entries first given room: a small load reserves little,
/// and a large one grows geometrically from here.
const FIRST: usize = 1 << 10;

/// What was gathered of a load's entries: those held, whether they are
/// all of them, and the room they are in, given back when it is dropped.
pub(super) struct Gathered {
    pub(super) entries: Vec<Entry>,
    pub(super) all: bool,
    _held: Reservation,
}

/// Gathers `entries` while `budget` grants the room, each counted once,
/// until it refuses: the entries gathered are then not all of them.
///
/// # Errors
///
/// The entries' own.
pub(super) fn gather(entries: &mut dyn Iterator<Item = Result<(Bounds, RowId)>>, budget: &Arc<MemoryBudget>) -> Result<Gathered> {
    let mut held = budget.reserve(0, "an R-tree load's entries")?;
    let mut gathered: Vec<Entry> = Vec::new();
    loop {
        if gathered.len() == gathered.capacity() {
            let more = grown(&mut gathered, &mut held);
            if !more {
                return Ok(Gathered { entries: gathered, all: false, _held: held });
            }
        }
        let Some(entry) = entries.next() else { break };
        let (bounds, row) = entry?;
        gathered.push(Entry { bounds, id: row, value: 1 });
    }
    Ok(Gathered { entries: gathered, all: true, _held: held })
}

/// Grows `gathered`'s room geometrically, `held` first: false if the
/// budget refuses, or the allocation fails, and the room stays as it was.
fn grown(gathered: &mut Vec<Entry>, held: &mut Reservation) -> bool {
    let room = gathered.capacity().saturating_mul(2).max(FIRST);
    let granted = held.resize(room.saturating_mul(size_of::<Entry>()));
    if granted.is_err() {
        return false;
    }
    let made = gathered.try_reserve_exact(room.saturating_sub(gathered.len()));
    if made.is_err() {
        // Giving back what the allocation did not take cannot fail: the
        // reservation only shrinks.
        let _ = held.resize(gathered.capacity().saturating_mul(size_of::<Entry>()));
        return false;
    }
    true
}

impl Packer<'_> {
    /// The root's entry of the tree packed of `entries`, every one held,
    /// identical entries made one counted entry; none of no entries.
    pub(super) fn pack_held(&mut self, entries: &mut Vec<Entry>) -> Result<Option<Entry>> {
        entries.sort_unstable_by(|a, b| ordered(a, b, X));
        entries.dedup_by(|next, kept| {
            let same = next.bounds == kept.bounds && next.id == kept.id;
            if same {
                kept.value = kept.value.saturating_add(next.value);
            }
            same
        });
        if entries.is_empty() {
            return Ok(None);
        }
        let height = self.height(entries.len())?;
        Ok(self.tile_held(height, entries)?.pop())
    }

    /// The nodes at `level` holding `entries`, in X order, as `tile` makes
    /// them of records: slabs of them sorted by Y and cut into runs of one
    /// node's span, each run tiled a level down.
    fn tile_held(&mut self, level: u8, entries: &mut [Entry]) -> Result<Vec<Entry>> {
        let (span, per_slab) = self.slabbed(level, entries.len());
        let mut made = Vec::new();
        for slab in entries.chunks_mut(per_slab) {
            slab.sort_unstable_by(|a, b| ordered(a, b, Y));
            for run in slab.chunks_mut(span) {
                made.push(self.group_held(level, run)?);
            }
        }
        Ok(made)
    }

    /// One node at `level` of `run`, its entries sorted by X and tiled a
    /// level down.
    fn group_held(&mut self, level: u8, run: &mut [Entry]) -> Result<Entry> {
        let Some(down) = level.checked_sub(1) else { return self.node(0, run.to_vec()) };
        run.sort_unstable_by(|a, b| ordered(a, b, X));
        let children = self.tile_held(down, run)?;
        self.node(level, children)
    }
}

/// `a` against `b` as their records sort (`encode`): by their centres on
/// `first`, then on the other axis, then their corners' bits and rows.
fn ordered(a: &Entry, b: &Entry, first: usize) -> Ordering {
    let other = DIMENSIONS.saturating_sub(1).saturating_sub(first);
    centre(a.bounds, first)
        .total_cmp(&centre(b.bounds, first))
        .then_with(|| centre(a.bounds, other).total_cmp(&centre(b.bounds, other)))
        .then_with(|| corners(a.bounds).cmp(corners(b.bounds)))
        .then_with(|| a.id.cmp(&b.id))
}

/// The bits of `bounds`'s corners, lower then upper, as a record holds them.
fn corners(bounds: Bounds) -> impl Iterator<Item = u64> {
    bounds.lower().into_iter().chain(bounds.upper()).map(f64::to_bits)
}
