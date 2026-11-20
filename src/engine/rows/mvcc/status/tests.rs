use super::*;

/// Verifies: [DESIGN] RULES.md D418: a transaction is open until its
/// commit's one store, then committed at its timestamp; one rolled back is
/// forgotten and read as open; claiming again changes nothing.
#[test]
fn a_commit_is_one_store() {
    let statuses = Statuses::default();
    statuses.claim(5);
    statuses.claim(5);
    assert_eq!(statuses.committed(5), None);
    statuses.commit(5, 9, true);
    assert_eq!(statuses.committed(5), Some(9));
    statuses.claim(6);
    statuses.abort(6);
    assert_eq!(statuses.committed(6), None);
    assert_eq!(statuses.committed(7), None, "never claimed");
}

/// Verifies: [DESIGN] RULES.md D418: two transactions `SLOTS` apart share a
/// slot; the second waits on the map while the first is unstamped, or
/// stamped but not yet before the horizon, and each reads as its own; the
/// slot is claimed again once the horizon passes the first's mark, the
/// first then read as open. MC/DC of `free`: owner none, claiming, count
/// above none, mark none, mark not before the horizon, each taken alone.
#[test]
fn a_slot_is_claimed_again_only_past_the_horizon() {
    let statuses = Statuses::default();
    let (a, b, c) = (3, 3 + SLOTS, 3 + 2 * SLOTS);
    statuses.claim(a);
    statuses.seen(20);
    statuses.commit(a, 10, true);
    statuses.claim(b);
    assert_eq!(statuses.overflowing.load(Ordering::SeqCst), 1, "a is unstamped");
    statuses.commit(b, 11, true);
    assert_eq!((statuses.committed(a), statuses.committed(b)), (Some(10), Some(11)));
    statuses.stamped(a, 10);
    let slot = statuses.slot(a).unwrap();
    assert_eq!(slot.mark.load(Ordering::SeqCst), 20, "the latest snapshot, past the commit");
    statuses.collected(20);
    assert!(!statuses.free(slot, a), "not before the horizon");
    statuses.collected(21);
    assert!(statuses.free(slot, a));
    assert!(statuses.free(slot, 0));
    assert!(!statuses.free(slot, CLAIMING));
    statuses.claim(c);
    assert_eq!(statuses.committed(a), None, "a's slot is c's");
    assert_eq!(statuses.committed(c), None);
    slot.pending.store(1, Ordering::SeqCst);
    assert!(!statuses.free(slot, c), "a table to stamp");
    slot.pending.store(0, Ordering::SeqCst);
    assert!(!statuses.free(slot, c), "no mark");
    statuses.stamped(b, 11);
    statuses.sweep();
    assert_eq!(statuses.overflowing.load(Ordering::SeqCst), 0, "b is past the horizon");
}

/// Verifies: [DESIGN] RULES.md D418: a commit stamped in one table of two
/// is not free until the second is stamped too, and a table with nothing
/// to stamp counts as stamped.
#[test]
fn every_table_of_a_commit_is_stamped_first() {
    let statuses = Statuses::default();
    statuses.claim(1);
    statuses.commit(1, 4, true);
    statuses.commit(1, 4, true);
    statuses.commit(1, 4, false);
    statuses.collected(100);
    let slot = statuses.slot(1).unwrap();
    statuses.stamped(1, 4);
    assert!(!statuses.free(slot, 1));
    statuses.stamped(1, 4);
    assert!(statuses.free(slot, 1));
    statuses.stamped(1, 4);
    assert_eq!(slot.pending.load(Ordering::SeqCst), 0, "never below none");
}

/// Verifies: [DESIGN] RULES.md D418: a transaction in the map commits,
/// stamps and rolls back there as in a slot.
#[test]
fn the_map_keeps_statuses_as_slots_do() {
    let statuses = Statuses::default();
    statuses.claim(2);
    statuses.claim(2 + SLOTS);
    statuses.claim(2 + 2 * SLOTS);
    assert_eq!(statuses.overflowing.load(Ordering::SeqCst), 2);
    statuses.commit(2 + SLOTS, 8, true);
    statuses.commit(2 + SLOTS, 8, false);
    assert_eq!(statuses.committed(2 + SLOTS), Some(8));
    statuses.abort(2 + 2 * SLOTS);
    assert_eq!(statuses.overflowing.load(Ordering::SeqCst), 1);
    statuses.collected(50);
    statuses.sweep();
    assert_eq!(statuses.overflowing.load(Ordering::SeqCst), 1, "not stamped");
    statuses.stamped(2 + SLOTS, 8);
    statuses.sweep();
    assert_eq!(statuses.overflowing.load(Ordering::SeqCst), 0);
    assert_eq!(statuses.committed(2 + SLOTS), None);
}
