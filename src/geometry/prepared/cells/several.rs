//! Boundary cells that several rings' edges may touch, or whose one ring
//! does not decide them alone (RULES.md D85). At the cell's reference
//! point the sweep knows every ring's parity, and rings without edges in
//! the cell have the same parity at every point of it. So a polygon none
//! of whose rings touch the cell holds all of it or none: if one holds it,
//! the cell is interior. A polygon whose shell does not touch the cell and
//! does not hold it holds none of the cell, so it is left out. For each
//! other polygon a record keeps whether its shell touches the cell or holds
//! it, the first of its holes, in order, that holds the cell without
//! touching it, and each touching ring with its parity at the reference
//! point. A point is then located from those rings' edges near it, and the
//! polygons combined as exact location combines them (`path.rs`).
//!
//! A record is a sequence of words: the number of polygons, then for each
//! its shell's word, its first holding hole's ring or `NO_HOLE`, the count
//! of its touching rings, and each of those rings' index times two plus its
//! parity, in ring order.

use super::{BOUNDARY, EXTERIOR, INTERIOR, NEAR_SEVERAL, Parity};

/// A polygon's shell touches the cell, or holds it without touching it.
pub(super) const SHELL_TOUCHES: u32 = 0;
pub(super) const SHELL_HOLDS: u32 = 1;
/// No hole of the polygon holds the cell without touching it.
pub(super) const NO_HOLE: u32 = u32::MAX;
/// The most rings a record keeps: one bit each in a word.
pub(super) const MOST_RINGS: usize = 64;
/// Holes searched for the first that holds the cell; a polygon with more
/// leaves such a cell to the exact edge tests.
const MOST_HOLES: usize = 1_024;

/// The records of a grid's cells, found by cell index once sorted.
#[derive(Default)]
pub(super) struct Records {
    cells: Vec<(usize, usize)>,
    words: Vec<u32>,
    rings: Vec<usize>,
}

impl Records {
    /// The state of cell `cell`, which the rings `rings` touch, from the
    /// parities at its reference point, recording it if its points are to
    /// be located near them.
    pub(super) fn add(&mut self, parity: &Parity<'_>, rings: impl Iterator<Item = usize>, cell: usize) -> u8 {
        self.rings.clear();
        self.rings.extend(rings);
        let start = self.words.len();
        let state = record(parity, &self.rings, &mut self.words).unwrap_or(BOUNDARY);
        if state == NEAR_SEVERAL && self.cells.try_reserve(1).is_ok() {
            self.cells.push((cell, start));
            return state;
        }
        self.words.truncate(start);
        if state == NEAR_SEVERAL { BOUNDARY } else { state }
    }

    /// Orders the records by cell, once every cell is labelled.
    pub(super) fn sort(&mut self) {
        self.cells.sort_unstable();
    }

    /// The record of cell `cell`, and the words after it.
    pub(super) fn get(&self, cell: usize) -> Option<&[u32]> {
        let at = self.cells.binary_search_by_key(&cell, |&(at, _)| at).ok()?;
        let &(_, start) = self.cells.get(at)?;
        self.words.get(start..)
    }

    /// The bytes the records hold.
    pub(super) fn bytes(&self) -> Option<usize> {
        let cells = self.cells.capacity().checked_mul(size_of::<(usize, usize)>())?;
        let words = self.words.capacity().checked_mul(size_of::<u32>())?;
        cells.checked_add(words)?.checked_add(self.rings.capacity().checked_mul(size_of::<usize>())?)
    }
}

/// Writes the record of a cell the sorted rings `rings` touch and returns
/// its state; none if it cannot be written.
fn record(parity: &Parity<'_>, rings: &[usize], words: &mut Vec<u32>) -> Option<u8> {
    if rings.len() > MOST_RINGS {
        return None;
    }
    let start = words.len();
    words.try_reserve(rings.len().checked_mul(4)?.checked_add(1)?).ok()?;
    words.push(0);
    let (mut kept, mut holding) = (0_u32, 0_usize);
    for group in rings.chunk_by(|&a, &b| parity.rings.get(a).map(|r| r.polygon) == parity.rings.get(b).map(|r| r.polygon)) {
        let polygon = parity.rings.get(*group.first()?)?.polygon;
        let (shell_odd, odd_holes) = *parity.polygons.get(polygon)?;
        holding = holding.checked_add(usize::from(shell_odd && odd_holes == 0))?;
        let touches = group.first() == parity.starts.get(polygon);
        if !touches && !shell_odd {
            continue;
        }
        let shell = if touches { SHELL_TOUCHES } else { SHELL_HOLDS };
        words.extend([shell, first_holding_hole(parity, polygon, group, odd_holes)?, u32::try_from(group.len()).ok()?]);
        for &ring in group {
            words.push(u32::try_from(ring).ok()?.checked_mul(2)?.checked_add(u32::from(*parity.odd.get(ring)?))?);
        }
        kept = kept.checked_add(1)?;
    }
    if parity.inside > holding {
        return Some(INTERIOR);
    }
    if kept == 0 {
        return Some(EXTERIOR);
    }
    *words.get_mut(start)? = kept;
    Some(NEAR_SEVERAL)
}

/// The first hole of `polygon`, in order, that holds the cell without
/// touching it; `NO_HOLE` when its odd holes all touch it, as their count
/// shows without a search.
fn first_holding_hole(parity: &Parity<'_>, polygon: usize, rings: &[usize], odd_holes: usize) -> Option<u32> {
    let odd_touching =
        rings.iter().filter(|&&ring| parity.rings.get(ring).is_some_and(|r| !r.shell) && parity.odd.get(ring) == Some(&true)).count();
    if odd_holes == odd_touching {
        return Some(NO_HOLE);
    }
    let first = parity.starts.get(polygon)?.checked_add(1)?;
    let end = *parity.starts.get(polygon.checked_add(1)?)?;
    if end.saturating_sub(first) > MOST_HOLES {
        return None;
    }
    let hole = (first..end).find(|ring| parity.odd.get(*ring) == Some(&true) && rings.binary_search(ring).is_err())?;
    u32::try_from(hole).ok()
}
