//! Unit tests of row waits handed over to a row's next writer (RULES.md
//! D743).

use std::sync::Arc;
use std::thread;

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

/// Waits until `ending` has `count` seated.
fn until_seated(ending: &Ending, count: usize) {
    while seated(ending).len() < count {
        thread::yield_now();
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
    // check leaves no waiter asleep for long.
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
        // The other row's waiter goes on now, unless it was moved too.
        let mut c = Some(c);
        let others_went = (stayed == [other.id]).then(|| c.take().is_some_and(|c| c.join().unwrap().unwrap()));
        let still = seated(&end_of(&first));
        st.rollback(&first).unwrap();
        let all_went = [Some(b), c, Some(d)].into_iter().flatten().all(|waiter| waiter.join().unwrap().unwrap());
        (stayed, moved, graph, others_went, still, all_went)
    });
    for tx in [&second, &other, &third] {
        st.rollback(tx).unwrap();
    }
    assert_eq!(stayed, [other.id]);
    assert_eq!(moved, [second.id, third.id], "moved in order, asleep");
    assert_eq!(graph, Some(vec![first.id]), "waiting for the next writer");
    assert_eq!(others_went, Some(true), "the other row's waiter goes on");
    assert_eq!(still, [second.id, third.id], "the moved still wait");
    assert!(all_went);
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

/// Verifies: [DESIGN] RULES.md D743, R-TEST-8: a seat moves only if its
/// row's id, its table and the absence of a deadlock all hold. Condition
/// pairs against the moved seat (same id, same table, no deadlock): the
/// same table's other row stays (id), another table's same row stays
/// (table), and the same row's waiter that the next writer already waits
/// for stays (deadlock).
#[test]
fn a_seat_moves_only_for_the_same_row_free_of_deadlock() {
    let st = State::with_builtins(Box::new(MemoryCatalog::default()));
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    let elsewhere = QualifiedName { auth: "A".into(), ident: "U".into() };
    let [holder, first, same, other_id, other_table, cyclic] = std::array::from_fn(|_| waiting(&st));
    let held = end_of(&holder);
    let (moved, stayed, all_went) = thread::scope(|s| {
        let a = s.spawn(|| first.wait_for_row((&table, 1), holder.id));
        until_seated(&held, 1);
        let mut waiters = Vec::new();
        for (n, (tx, row)) in [(&same, (&table, 1)), (&other_id, (&table, 2)), (&other_table, (&elsewhere, 1)), (&cyclic, (&table, 1))]
            .into_iter()
            .enumerate()
        {
            waiters.push(s.spawn(move || tx.wait_for_row(row, holder.id).map(|turn| turn.is_some())));
            until_seated(&held, n + 2);
        }
        st.rollback(&holder).unwrap();
        let firsts = a.join().unwrap().unwrap().unwrap();
        // The next writer waiting for `cyclic`, as if for a row it holds.
        lock_builtin(&first.shared.waits_for).insert(first.id, vec![cyclic.id]);
        super::Turn::hand_over(Some(&firsts), &first, (&table, 1));
        lock_builtin(&first.shared.waits_for).remove(&first.id);
        let (moved, stayed) = (seated(&end_of(&first)), seated(&held));
        drop(firsts);
        st.rollback(&first).unwrap();
        let all_went = waiters.into_iter().all(|waiter| waiter.join().unwrap().unwrap());
        (moved, stayed, all_went)
    });
    for tx in [&same, &other_id, &other_table, &cyclic] {
        st.rollback(tx).unwrap();
    }
    assert_eq!(moved, [same.id]);
    assert_eq!(stayed, [other_id.id, other_table.id, cyclic.id]);
    assert!(all_went);
}
