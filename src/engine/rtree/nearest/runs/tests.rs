//! Unit tests of the frontier's runs (RULES.md R-TEST-1): the order they
//! give, and the room they charge.

use std::collections::BinaryHeap;

use super::*;
use crate::memory::MemoryBudget;

/// Items of `count` keys drawn from a fixed sequence, with many repeated
/// keys, each with an id of its own: the same every run (R-TEST-5).
fn items(round: u64, count: u64) -> Vec<(u64, u64)> {
    (0..count)
        .map(|n| (round.wrapping_mul(7_919).wrapping_add(n.wrapping_mul(104_729)) % 97, round.wrapping_mul(1_000).wrapping_add(n)))
        .collect()
}

/// Verifies: [DESIGN] RULES.md D72: taken one at a time, runs give their
/// items in the order one heap of all of them gives, repeated keys
/// included, as runs from empty to hundreds of items are added between
/// takes and after the last.
#[test]
fn runs_give_the_order_of_one_heap() {
    let budget = MemoryBudget::new(usize::MAX);
    let mut held = budget.reserve(0, "runs").unwrap();
    let mut runs = Runs::new();
    let mut heap = BinaryHeap::new();
    for round in 0..200_u64 {
        let added = items(round, round.wrapping_mul(37) % 300);
        heap.extend(added.iter().copied());
        runs.add(added.len(), added.into_iter(), &mut held).unwrap();
        for _ in 0..round % 7 {
            assert_eq!(runs.pop().unwrap(), heap.pop());
        }
    }
    while let Some(expected) = heap.pop() {
        assert_eq!(runs.pop().unwrap(), Some(expected));
    }
    assert_eq!(runs.pop().unwrap(), None);
}

/// Verifies: [DESIGN] RULES.md D38: runs charge their room to the budget
/// before allocating it; a refused run (SQLCODE -918) adds nothing, and
/// those added before are still given in order.
#[test]
fn runs_charge_their_room_first() {
    let budget = MemoryBudget::new(usize::MAX);
    let mut held = budget.reserve(0, "runs").unwrap();
    let mut runs = Runs::new();
    runs.add(3, [(1, 0), (3, 1), (2, 2)].into_iter(), &mut held).unwrap();
    assert!(held.bytes() > 0, "the first run is charged");
    budget.set_limit(budget.reserved());
    let refused = runs.add(10_000, (0..10_000_u64).map(|n| (n, n)), &mut held).unwrap_err();
    assert_eq!(refused.code, Code::Resources);
    assert_eq!(runs.pop().unwrap(), Some((3, 1)));
    assert_eq!(runs.pop().unwrap(), Some((2, 2)));
    assert_eq!(runs.pop().unwrap(), Some((1, 0)));
    assert_eq!(runs.pop().unwrap(), None);
}
