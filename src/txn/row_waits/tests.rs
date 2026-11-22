//! Unit tests of row waits handed over to a row's next writer (RULES.md
//! D743).

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::Ending;
use crate::catalog::{MemoryCatalog, QualifiedName};
use crate::db::State;
use crate::sync::lock_builtin;
use crate::txn::{LockWait, Reading, TxId, Txn};

/// The transactions seated on `ending`, in ticket order.
fn seated(ending: &Ending) -> Vec<TxId> {
    let queue = lock_builtin(&ending.queue);
    let mut seats: Vec<(u64, TxId)> = queue.seated.iter().map(|(t, seat)| (*t, seat.waiter)).collect();
    seats.sort_unstable();
    seats.into_iter().map(|(_, waiter)| waiter).collect()
}

/// Waits until `ending` has `count` seated, for a few seconds at most.
fn until_seated(ending: &Ending, count: usize) {
    let began = std::time::Instant::now();
    while seated(ending).len() < count && began.elapsed() < Duration::from_secs(5) {
        thread::sleep(Duration::from_millis(1));
    }
}

/// A transaction of `st` that waits for locks.
fn waiting(st: &State) -> Txn {
    st.begin_waiting((LockWait::Wait, Reading::Serializable, Arc::default()))
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
    let [holder, first, second, other, third] = std::array::from_fn(|_| waiting(&st));
    let held = end_of(&holder);
    // What is seen, kept until every transaction has ended, so a failed
    // check never leaves a waiter asleep.
    let (stayed, moved, graph, others_went, still, all_went) = thread::scope(|s| {
        let a = s.spawn(|| first.wait_for_row((&table, 1), holder.id));
        until_seated(&held, 1);
        let b = s.spawn(|| second.wait_for_row((&table, 1), holder.id).map(|turn| turn.is_some()));
        until_seated(&held, 2);
        let c = s.spawn(|| other.wait_for_row((&table, 2), holder.id).map(|turn| turn.is_some()));
        until_seated(&held, 3);
        let d = s.spawn(|| third.wait_for_row((&table, 1), holder.id).map(|turn| turn.is_some()));
        until_seated(&held, 4);
        st.rollback(&holder).unwrap();
        let firsts = a.join().unwrap().unwrap().unwrap();
        super::Turn::hand_over(Some(&firsts), &first, (&table, 1));
        let (stayed, moved) = (seated(&held), seated(&end_of(&first)));
        let graph = lock_builtin(&first.shared.waits_for).get(&second.id).cloned();
        drop(firsts);
        let others_went = finishes(&c);
        let still = seated(&end_of(&first));
        st.rollback(&first).unwrap();
        let all_went = [b, c, d].into_iter().all(|waiter| waiter.join().unwrap().unwrap());
        (stayed, moved, graph, others_went, still, all_went)
    });
    for tx in [&second, &other, &third] {
        st.rollback(tx).unwrap();
    }
    assert_eq!(stayed, [other.id]);
    assert_eq!(moved, [second.id, third.id], "moved in order, asleep");
    assert_eq!(graph, Some(vec![first.id]), "waiting for the next writer");
    assert!(others_went, "the other row's waiter goes on");
    assert_eq!(still, [second.id, third.id], "the moved still wait");
    assert!(all_went);
}

/// Whether `waiter` finishes within a few seconds.
fn finishes<T>(waiter: &thread::ScopedJoinHandle<'_, T>) -> bool {
    let began = std::time::Instant::now();
    while !waiter.is_finished() && began.elapsed() < Duration::from_secs(5) {
        thread::sleep(Duration::from_millis(1));
    }
    waiter.is_finished()
}

/// Verifies: [DESIGN] RULES.md D743: a moved waiter cancelled leaves the
/// line it was moved to, and the turn passes over it there.
#[test]
fn a_moved_waiter_gives_up_where_it_was_moved() {
    let st = State::with_builtins(Box::new(MemoryCatalog::default()));
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    let [holder, first, second, third] = std::array::from_fn(|_| waiting(&st));
    let held = end_of(&holder);
    let (cancelled, left, passed) = thread::scope(|s| {
        let a = s.spawn(|| first.wait_for_row((&table, 1), holder.id));
        until_seated(&held, 1);
        // In a statement's window, which a cancel stops.
        let b = s.spawn(|| {
            let _window = second.stop.window();
            second.wait_for_row((&table, 1), holder.id).map(|turn| turn.is_some())
        });
        until_seated(&held, 2);
        st.rollback(&holder).unwrap();
        let firsts = a.join().unwrap().unwrap().unwrap();
        super::Turn::hand_over(Some(&firsts), &first, (&table, 1));
        drop(firsts);
        let moved = end_of(&first);
        let c = s.spawn(|| third.wait_for_row((&table, 1), first.id).map(|turn| turn.is_some()));
        until_seated(&moved, 2);
        second.stop.cancel();
        let cancelled = b.join().unwrap().is_err();
        let left = seated(&moved);
        st.rollback(&first).unwrap();
        (cancelled, left, c.join().unwrap().unwrap())
    });
    for tx in [&second, &third] {
        st.rollback(tx).unwrap();
    }
    assert!(cancelled);
    assert_eq!(left, [third.id]);
    assert!(passed, "the turn passes over the one that left");
}
