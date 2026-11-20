//! The kth computed distance, in a fixed-capacity, budgeted heap.
//! Rows stay in the ordinary SQL sorter; this heap holds only measures.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::db::State;
use crate::error::Result;
use crate::exec::{broken, work::Work};
use crate::memory::Reservation;

const RESERVATION: &str = "nearest distance threshold";

#[derive(Clone, Copy)]
struct Distance(f64);

impl Ord for Distance {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl PartialOrd for Distance {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Distance {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Distance {}

pub(super) struct Threshold {
    heap: BinaryHeap<Distance>,
    first: usize,
    /// Whether its values are negated scores, at most 0 (D87).
    negated: bool,
    _held: Reservation,
}

impl Threshold {
    pub(super) fn new(first: usize, db: &State) -> Option<Self> {
        let bytes = first.checked_mul(size_of::<Distance>())?;
        if first == 0 || bytes > Work::ceiling_of(db) {
            return None;
        }
        let mut held = db.memory.reserve(bytes, RESERVATION).ok()?;
        let mut heap = BinaryHeap::new();
        heap.try_reserve_exact(first).ok()?;
        let allocated = heap.capacity().checked_mul(size_of::<Distance>())?;
        if allocated > Work::ceiling_of(db) {
            return None;
        }
        held.resize(allocated).ok()?;
        Some(Self { heap, first, negated: false, _held: held })
    }

    /// It, of negated scores, each at most 0, as a descending read gives.
    pub(super) fn negated(self, negated: bool) -> Self {
        Self { negated, ..self }
    }

    pub(super) fn full(&self) -> bool {
        self.heap.len() == self.first
    }

    pub(super) fn add(&mut self, distance: f64) -> Result<()> {
        self.valid(distance)?;
        if !self.full() {
            self.heap.push(Distance(distance));
        } else if let Some(mut greatest) = self.heap.peek_mut()
            && distance < greatest.0
        {
            *greatest = Distance(distance);
        }
        Ok(())
    }

    pub(super) fn beyond(&self, bound: f64) -> Result<bool> {
        self.valid(bound)?;
        Ok(self.full() && self.heap.peek().is_some_and(|greatest| bound > greatest.0))
    }

    /// Whether `distance` is finite, and nonnegative, or of a negated
    /// score at most 0.
    fn valid(&self, distance: f64) -> Result<()> {
        let signed = if self.negated { -distance } else { distance };
        if !distance.is_finite() || signed < 0.0_f64 {
            return Err(broken("a nearest distance or bound is not finite and nonnegative"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::error::Code;

    /// Verifies: [DESIGN] D38, D85: the threshold counts qualifying
    /// distances only, retains ties, updates its kth value without heap
    /// growth, and releases its reservation. Invalid plugin measures fail.
    #[test]
    fn nearest_geometry_threshold_keeps_ties_and_releases_memory() {
        let db = Database::new();
        let state = db.snapshot().unwrap();
        let before = state.memory.reserved();
        let mut top = Threshold::new(3, &state).unwrap();
        assert_eq!(state.memory.reserved(), before.saturating_add(3 * size_of::<Distance>()));
        for value in [10.0_f64, 8.0_f64] {
            top.add(value).unwrap();
            assert!(!top.full() && !top.beyond(100.0).unwrap());
        }
        top.add(6.0).unwrap();
        assert!(top.full() && top.beyond(10.0_f64.next_up()).unwrap());
        assert!(!top.beyond(10.0).unwrap());
        for value in [20.0_f64, 8.0_f64, 7.0_f64, 0.0_f64] {
            top.add(value).unwrap();
        }
        assert!(!top.beyond(7.0).unwrap());
        assert!(top.beyond(7.0_f64.next_up()).unwrap());
        assert_eq!(top.heap.capacity(), 3);
        for bad in [-1.0_f64, f64::NAN, f64::INFINITY] {
            assert_eq!(top.add(bad).unwrap_err().code, Code::Api);
            assert_eq!(top.beyond(bad).unwrap_err().code, Code::Api);
        }
        drop(top);
        assert_eq!(state.memory.reserved(), before);
    }

    /// Verifies: [DESIGN] D38: zero/overflowing counts, an insufficient
    /// operator work area, and an exhausted global budget independently
    /// decline the threshold before any candidate can be emitted.
    #[test]
    fn nearest_geometry_threshold_declines_unaffordable_counts() {
        let db = Database::new();
        let state = db.snapshot().unwrap();
        assert!(Threshold::new(0, &state).is_none());
        assert!(Threshold::new(usize::MAX, &state).is_none());
        let too_many = Work::ceiling_of(&state).div_euclid(size_of::<Distance>()).saturating_add(1);
        assert!(Threshold::new(too_many, &state).is_none());
        state.memory.set_limit(state.memory.reserved());
        assert!(Threshold::new(1, &state).is_none());
    }
}
