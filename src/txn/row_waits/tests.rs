//! Unit tests of row waits handed over to a row's next writer (RULES.md
//! D743).

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::Ending;
use crate::catalog::{MemoryCatalog, QualifiedName};
use crate::db::State;
use crate::sync::lock_builtin;
use crate::txn::{TxId, Txn};

/// The transactions seated on `ending`, in ticket order.
fn seated(ending: &Ending) -> Vec<TxId> {
    let queue = lock_builtin(&ending.queue);
    let mut seats: Vec<(u64, TxId)> = queue.seated.iter().map(|(t, seat)| (*t, seat.waiter)).collect();
    seats.sort_unstable();
    seats.into_iter().map(|(_, waiter)| waiter).collect()
}

/// Waits until `ending` has `count` seated.
fn until_seated(ending: &Ending, count: usize) {
    while seated(ending).len() < count {
        thread::sleep(Duration::from_millis(1));
    }
}

/// The end of open transaction `tx`.
fn end_of(tx: &Txn) -> Arc<Ending> {
    tx.shared.ends.of(tx.id).unwrap()
}

/// Verifies: [DESIGN] RULES.md D743: a waiter whose turn's write is made
/// hands the waiters behind it for the same row to its own end, unwoken and
/// in their order, waiting for it in the graph; a waiter for another row
/// stays, and its turn comes as the first's is dropped; the moved waiter's
/// comes once the first ends.
#[test]
fn a_rows_waiters_are_handed_to_its_next_writer() {
    let st = State::with_builtins(Box::new(MemoryCatalog::default()));
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    let [holder, first, second, other, third] = std::array::from_fn(|_| st.begin());
    let held = end_of(&holder);
    thread::scope(|s| {
        let a = s.spawn(|| first.wait_for_row((&table, 1), holder.id));
        until_seated(&held, 1);
        let b = s.spawn(|| second.wait_for_row((&table, 1), holder.id).map(|turn| turn.is_some()));
        until_seated(&held, 2);
        let c = s.spawn(|| other.wait_for_row((&table, 2), holder.id).map(|turn| turn.is_some()));
        until_seated(&held, 3);
        let d = s.spawn(|| third.wait_for_row((&table, 1), holder.id).map(|turn| turn.is_some()));
        until_seated(&held, 4);
        st.rollback(&holder).unwrap();
        let turn = a.join().unwrap().unwrap().unwrap();
        turn.hand_over(&first, (&table, 1));
        assert_eq!(seated(&held), [other.id]);
        assert_eq!(seated(&end_of(&first)), [second.id, third.id], "moved in order, asleep");
        assert_eq!(lock_builtin(&first.shared.waits_for).get(&second.id), Some(&vec![first.id]));
        drop(turn);
        assert!(c.join().unwrap().unwrap(), "the other row's waiter goes on");
        assert_eq!(seated(&end_of(&first)), [second.id, third.id], "the moved still wait");
        st.rollback(&first).unwrap();
        assert!(b.join().unwrap().unwrap());
        assert!(d.join().unwrap().unwrap());
    });
    for tx in [&second, &other, &third] {
        st.rollback(tx).unwrap();
    }
}

/// Verifies: [DESIGN] RULES.md D743: a moved waiter cancelled leaves the
/// line it was moved to, and the turn passes over it there.
#[test]
fn a_moved_waiter_gives_up_where_it_was_moved() {
    let st = State::with_builtins(Box::new(MemoryCatalog::default()));
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    let [holder, first, second, third] = std::array::from_fn(|_| st.begin());
    let held = end_of(&holder);
    thread::scope(|s| {
        let a = s.spawn(|| first.wait_for_row((&table, 1), holder.id));
        until_seated(&held, 1);
        let b = s.spawn(|| second.wait_for_row((&table, 1), holder.id).map(|turn| turn.is_some()));
        until_seated(&held, 2);
        st.rollback(&holder).unwrap();
        let turn = a.join().unwrap().unwrap().unwrap();
        turn.hand_over(&first, (&table, 1));
        drop(turn);
        let moved = end_of(&first);
        let c = s.spawn(|| third.wait_for_row((&table, 1), first.id).map(|turn| turn.is_some()));
        until_seated(&moved, 2);
        second.stop.cancel();
        assert!(b.join().unwrap().is_err(), "cancelled");
        assert_eq!(seated(&moved), [third.id]);
        st.rollback(&first).unwrap();
        assert!(c.join().unwrap().unwrap(), "the turn passes over the one that left");
    });
    for tx in [&second, &third] {
        st.rollback(tx).unwrap();
    }
}
