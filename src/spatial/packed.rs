//! A temporary hierarchy of closed bounds with word-sized mask leaves.
//! Balanced spatial partitions prune groups before their four boundary
//! searches. Skip links stream candidates without a per-query allocation;
//! callers still check every geometry candidate exactly (D85).

use super::{
    Bounds,
    masks::{Masks, ROWS},
};
use crate::error::{Code, Result, SqlError};

// A balanced binary partition halves its input at each level. Even a
// usize-sized input cannot reach more than this many levels.
const MAX_DEPTH: u32 = usize::BITS;
const CHILDREN: usize = 2;
const HALF: f64 = 0.5;

struct Item {
    row: usize,
    bounds: Bounds,
}

struct Node {
    bounds: Bounds,
    after: usize,
    start: usize,
    masks: Option<Masks>,
}

pub(crate) struct Packed {
    items: Vec<Item>,
    nodes: Vec<Node>,
}

#[derive(Default)]
pub(crate) struct Cursor {
    next: usize,
    start: usize,
    pending: u64,
}

impl Packed {
    /// Includes construction scratch and every retained allocation. The
    /// caller reserves this before creating the hierarchy.
    pub(crate) fn bytes(rows: usize) -> usize {
        let nodes = slots(rows);
        size_of::<Self>()
            .saturating_add(rows.saturating_mul(size_of::<Item>()))
            .saturating_add(nodes.saturating_mul(size_of::<Node>()))
            .saturating_add(rows.saturating_mul(Masks::ENTRY_BYTES))
            .saturating_add(ROWS.saturating_mul(size_of::<Option<Bounds>>()))
    }

    pub(crate) fn new(rectangles: &[Option<Bounds>]) -> Result<Self> {
        let mut items = Vec::with_capacity(rectangles.len());
        for (row, bounds) in rectangles.iter().enumerate() {
            if let Some(bounds) = bounds {
                items.push(Item { row, bounds: *bounds });
            }
        }
        let mut nodes = Vec::with_capacity(slots(rectangles.len()));
        if !items.is_empty() {
            build(&mut items, &mut nodes, 0, 0)?;
        }
        Ok(Self { items, nodes })
    }

    pub(crate) fn next(&self, bounds: Bounds, cursor: &mut Cursor) -> Result<Option<usize>> {
        loop {
            if cursor.pending != 0 {
                let bit = usize::try_from(cursor.pending.trailing_zeros()).map_err(|_| broken())?;
                cursor.pending &= cursor.pending.saturating_sub(1);
                let item = self.items.get(cursor.start.saturating_add(bit)).ok_or_else(broken)?;
                return Ok(Some(item.row));
            }
            let Some(node) = self.nodes.get(cursor.next) else { return Ok(None) };
            cursor.next = cursor.next.saturating_add(1);
            if !node.bounds.intersects(bounds) {
                cursor.next = node.after;
            } else if let Some(masks) = &node.masks {
                cursor.pending = masks.overlapping(bounds);
                cursor.start = node.start;
            }
        }
    }
}

fn slots(rows: usize) -> usize {
    if rows == 0 {
        return 0;
    }
    let leaves = rows.div_ceil(ROWS).checked_next_power_of_two().unwrap_or(usize::MAX);
    leaves.saturating_mul(CHILDREN).saturating_sub(1)
}

fn build(items: &mut [Item], nodes: &mut Vec<Node>, start: usize, depth: u32) -> Result<()> {
    if depth >= MAX_DEPTH {
        return Err(broken());
    }
    let bounds = items.iter().map(|item| item.bounds).reduce(Bounds::union).ok_or_else(broken)?;
    let at = nodes.len();
    let masks = if items.len() <= ROWS {
        let leaves: Vec<_> = items.iter().map(|item| Some(item.bounds)).collect();
        Some(Masks::new(&leaves).ok_or_else(broken)?)
    } else {
        None
    };
    let leaf = masks.is_some();
    nodes.push(Node { bounds, after: at.saturating_add(1), start, masks });
    if !leaf {
        items.sort_unstable_by(|a, b| centre(a.bounds, depth).total_cmp(&centre(b.bounds, depth)));
        let middle = items.len().checked_div(CHILDREN).ok_or_else(broken)?;
        let (left, right) = items.split_at_mut_checked(middle).ok_or_else(broken)?;
        build(left, nodes, start, depth.saturating_add(1))?;
        build(right, nodes, start.saturating_add(middle), depth.saturating_add(1))?;
    }
    let after = nodes.len();
    nodes.get_mut(at).ok_or_else(broken)?.after = after;
    Ok(())
}

fn centre(bounds: Bounds, depth: u32) -> f64 {
    let [lx, ly] = bounds.lower();
    let [ux, uy] = bounds.upper();
    // Halve before adding so extreme finite coordinates do not overflow.
    if depth & 1 == 0 { lx * HALF + ux * HALF } else { ly * HALF + uy * HALF }
}

fn broken() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a temporary spatial hierarchy is inconsistent")
}

#[cfg(test)]
mod tests;
