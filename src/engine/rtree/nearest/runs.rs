//! \[DESIGN\] A frontier kept as runs (RULES.md D72). Items arrive a node
//! at a time; each node's items become a run, a binary heap built in place
//! in one arena that every run shares, and a heap of the runs' greatest
//! items orders the runs. A node then costs one linear heap build and one
//! push, rather than a logarithmic push for each of its items, and items a
//! search never reaches are never sifted. Taken one at a time, the items
//! come out as one heap of all of them would give them, ties included.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use super::exhausted;
use crate::error::{Code, Result, SqlError};
use crate::memory::Reservation;

/// A run's heap is binary: the item at `i` has children at `2i + 1` and
/// `2i + 2`.
const ARITY: usize = 2;
/// Avoid an allocation at each of the first few runs.
const INITIAL_CAPACITY: usize = 64;
/// Geometric growth bounds allocation work as the frontier expands.
const GROWTH: usize = 2;

/// A run's place in the arena: its heap is the items from `start` to `end`.
#[derive(Clone, Copy)]
struct Run {
    start: usize,
    end: usize,
}

/// A run's greatest item, by which the runs are ordered.
#[derive(Clone, Copy)]
struct Head<T> {
    item: T,
    run: usize,
}

impl<T: Ord> Ord for Head<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.item.cmp(&other.item)
    }
}

impl<T: Ord> PartialOrd for Head<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Ord> PartialEq for Head<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: Ord> Eq for Head<T> {}

/// Items taken greatest first, as a `BinaryHeap` of them all gives them.
pub(super) struct Runs<T> {
    arena: Vec<T>,
    places: Vec<Run>,
    heads: BinaryHeap<Head<T>>,
}

impl<T: Ord + Copy> Runs<T> {
    pub(super) fn new() -> Self {
        Self { arena: Vec::new(), places: Vec::new(), heads: BinaryHeap::new() }
    }

    /// Adds the first `count` of `items` as one run, charging `held` for
    /// any room it needs before allocating it (RULES.md D38). An empty run
    /// adds nothing.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the budget refuses the room; nothing is added.
    pub(super) fn add(&mut self, count: usize, items: impl Iterator<Item = T>, held: &mut Reservation) -> Result<()> {
        self.reserve(count, held)?;
        let start = self.arena.len();
        self.arena.extend(items.take(count));
        let end = self.arena.len();
        let run = self.arena.get_mut(start..end).ok_or_else(broken)?;
        heapify(run)?;
        let Some(&item) = run.first() else { return Ok(()) };
        self.places.push(Run { start, end });
        self.heads.push(Head { item, run: self.places.len().saturating_sub(1) });
        Ok(())
    }

    /// The greatest item, taken from its run, if any is left.
    ///
    /// # Errors
    ///
    /// An internal error if a run's place is not in the arena.
    pub(super) fn pop(&mut self) -> Result<Option<T>> {
        let Some(head) = self.heads.pop() else { return Ok(None) };
        let Run { start, end } = *self.places.get(head.run).ok_or_else(broken)?;
        let items = self.arena.get_mut(start..end).ok_or_else(broken)?;
        let last = items.len().checked_sub(1).ok_or_else(broken)?;
        let taken = *items.first().ok_or_else(broken)?;
        exchange(items, 0, last)?;
        let rest = items.get_mut(..last).ok_or_else(broken)?;
        sift_down(rest, 0)?;
        // The heads never outnumber the runs, whose room is reserved.
        if let Some(&item) = rest.first() {
            self.heads.push(Head { item, run: head.run });
        }
        let run = self.places.get_mut(head.run).ok_or_else(broken)?;
        run.end = start.checked_add(last).ok_or_else(broken)?;
        Ok(Some(taken))
    }

    /// Forgets every item, keeping the room already reserved.
    pub(super) fn clear(&mut self) {
        self.arena.clear();
        self.places.clear();
        self.heads.clear();
    }

    /// Room for `count` more items in one more run: the arena, the runs
    /// and their heads grown geometrically, charged to `held` first.
    fn reserve(&mut self, count: usize, held: &mut Reservation) -> Result<()> {
        let arena = grown(self.arena.len(), self.arena.capacity(), count)?;
        let runs = grown(self.places.len(), self.places.capacity(), 1)?;
        let heads = grown(self.heads.len(), self.heads.capacity(), 1)?.max(runs);
        if (arena, runs, heads) == (self.arena.capacity(), self.places.capacity(), self.heads.capacity()) {
            return Ok(());
        }
        held.resize(bytes::<T>(arena, runs, heads).ok_or_else(exhausted)?)?;
        self.arena.try_reserve_exact(arena.saturating_sub(self.arena.len())).map_err(|_| exhausted())?;
        self.places.try_reserve_exact(runs.saturating_sub(self.places.len())).map_err(|_| exhausted())?;
        self.heads.try_reserve_exact(heads.saturating_sub(self.heads.len())).map_err(|_| exhausted())?;
        Ok(())
    }
}

/// The capacity holding `extra` more than `len`: the present one if it
/// does, else at least twice it.
fn grown(len: usize, capacity: usize, extra: usize) -> Result<usize> {
    let needed = len.checked_add(extra).ok_or_else(exhausted)?;
    if needed <= capacity {
        return Ok(capacity);
    }
    Ok(capacity.checked_mul(GROWTH).ok_or_else(exhausted)?.max(needed).max(INITIAL_CAPACITY))
}

/// The bytes of an arena, runs and heads of these capacities.
fn bytes<T>(arena: usize, runs: usize, heads: usize) -> Option<usize> {
    let items = arena.checked_mul(size_of::<T>())?;
    let places = runs.checked_mul(size_of::<Run>())?;
    items.checked_add(places)?.checked_add(heads.checked_mul(size_of::<Head<T>>())?)
}

/// Makes `items` a heap, greatest first, in linear time.
fn heapify<T: Ord + Copy>(items: &mut [T]) -> Result<()> {
    let mut parents = items.len().checked_div(ARITY).unwrap_or(0);
    while let Some(at) = parents.checked_sub(1) {
        sift_down(items, at)?;
        parents = at;
    }
    Ok(())
}

/// Moves the item at `at` down until no child is greater.
fn sift_down<T: Ord + Copy>(items: &mut [T], mut at: usize) -> Result<()> {
    loop {
        let Some(left) = at.checked_mul(ARITY).and_then(|n| n.checked_add(1)) else { return Ok(()) };
        let Some(&first) = items.get(left) else { return Ok(()) };
        let right = left.checked_add(1).ok_or_else(broken)?;
        let child = if items.get(right).is_some_and(|other| *other > first) { right } else { left };
        let (Some(&parent), Some(&greater)) = (items.get(at), items.get(child)) else { return Err(broken()) };
        if greater <= parent {
            return Ok(());
        }
        exchange(items, at, child)?;
        at = child;
    }
}

/// Exchanges the items at `a` and `b`.
fn exchange<T: Copy>(items: &mut [T], a: usize, b: usize) -> Result<()> {
    let (Some(&first), Some(&second)) = (items.get(a), items.get(b)) else { return Err(broken()) };
    *items.get_mut(a).ok_or_else(broken)? = second;
    *items.get_mut(b).ok_or_else(broken)? = first;
    Ok(())
}

fn broken() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a nearest-neighbour frontier run is not in its arena")
}

#[cfg(test)]
mod tests;
