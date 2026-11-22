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

/// Verifies: [DESIGN] RULES.md D456: once a row's writer ends, a newcomer
/// finds the row claimed by its first waiter, which has its turn but has not
/// written yet, and queues behind it rather than writing first; the first's
/// write hands the newcomer to its end, where it waits until the first
/// ends.
#[test]
fn a_newcomer_never_overtakes_a_rows_first_waiter() {
    let st = State::with_builtins(Box::new(MemoryCatalog::default()));
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    let [holder, first, newcomer] = std::array::from_fn(|_| waiting(&st));
    let held = end_of(&holder);
    let (claim, own, behind, early, went) = thread::scope(|s| {
        let a = s.spawn(|| first.wait_for_row((&table, 1), holder.id));
        until_seated(&held, 1);
        st.rollback(&holder).unwrap();
        let firsts = a.join().unwrap().unwrap().unwrap();
        let claim = newcomer.claim_of((&table, 1)).map(|c| (c.owner, c.first));
        let own = first.claim_of((&table, 1)).is_none();
        let b = s.spawn(|| newcomer.claim_of((&table, 1)).map(|c| newcomer.wait_behind((&table, 1), &c).is_ok()));
        until_seated(&held, 1);
        super::Turn::hand_over(Some(&firsts), &first, (&table, 1));
        drop(firsts);
        let behind = seated(&end_of(&first));
        let early = b.is_finished();
        st.rollback(&first).unwrap();
        (claim, own, behind, early, b.join().unwrap())
    });
    st.rollback(&newcomer).unwrap();
    assert_eq!(claim, Some((holder.id, first.id)), "claimed for the first waiter");
    assert!(own, "the first waiter is not held by its own claim");
    assert_eq!(behind, [newcomer.id], "handed to the first's end");
    assert!(!early, "the newcomer waits until the first ends");
    assert_eq!(went, Some(true));
}

/// Verifies: [DESIGN] RULES.md D456: a first waiter that gives up its turn
/// without writing passes the row's claim to the next waiter for the row,
/// and the last to go leaves the row unclaimed.
#[test]
fn a_claim_passes_to_the_next_waiter_and_ends_with_the_last() {
    let st = State::with_builtins(Box::new(MemoryCatalog::default()));
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    let [holder, first, second, newcomer] = std::array::from_fn(|_| waiting(&st));
    let held = end_of(&holder);
    let (passed, ended) = thread::scope(|s| {
        let a = s.spawn(|| first.wait_for_row((&table, 1), holder.id));
        until_seated(&held, 1);
        let b = s.spawn(|| second.wait_for_row((&table, 1), holder.id));
        until_seated(&held, 2);
        st.rollback(&holder).unwrap();
        drop(a.join().unwrap().unwrap());
        let passed = newcomer.claim_of((&table, 1)).map(|c| c.first);
        drop(b.join().unwrap().unwrap());
        (passed, newcomer.claim_of((&table, 1)).is_none())
    });
    for tx in [&first, &second, &newcomer] {
        st.rollback(tx).unwrap();
    }
    assert_eq!(passed, Some(second.id));
    assert!(ended, "no claim once the line is gone");
}

/// A seat of `waiter` for row `id` of `table` in `ending`'s line, with
/// `ticket`.
fn seat(waiter: TxId, (table, id): (&QualifiedName, u64), (ending, ticket): (&Arc<Ending>, u64)) -> Arc<super::Seat> {
    let place = super::Place { ending: Arc::clone(ending), ticket, turn: false };
    Arc::new(super::Seat { waiter, row: (table.clone(), id), place: std::sync::Mutex::new(place), signal: std::sync::Condvar::new() })
}

/// Verifies: [DESIGN] RULES.md D456: a row's claim holds back a writer
/// that is neither the line's writer nor its first waiter. MC/DC of
/// `c.owner != tx && c.first != tx`: (true, true) holds back; (false,
/// true), the line's writer, and (true, false), the first waiter, do not.
/// A claim whose first waiter has ended is dropped.
#[test]
fn a_claim_holds_back_only_others() {
    let claims = super::Claims::default();
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    let line = Arc::new(Ending::default());
    assert!(claims.of((&table, 1), 30, |_| true).is_none(), "none claimed");
    claims.claim((&table, 1), super::Claim { owner: 10, line: Arc::clone(&line), first: 20 });
    claims.claim((&table, 1), super::Claim { owner: 11, line: Arc::clone(&line), first: 21 });
    assert_eq!(claims.of((&table, 1), 30, |_| true).map(|c| (c.owner, c.first)), Some((10, 20)), "the first claim kept");
    assert!(claims.of((&table, 1), 10, |_| true).is_none(), "the line's writer");
    assert!(claims.of((&table, 1), 20, |_| true).is_none(), "the first waiter");
    assert!(claims.of((&table, 2), 30, |_| true).is_none(), "another row");
    assert!(claims.of((&table, 1), 30, |_| false).is_none(), "its first ended");
    assert!(claims.of((&table, 1), 30, |_| true).is_none(), "dropped");
}

/// Verifies: [DESIGN] RULES.md D456: a seat giving up its place passes its
/// row's claim to the next seat for the row in the line, or ends it. MC/DC
/// of `c.first == seat.waiter && Arc::ptr_eq(&c.line, line)`: (true, true)
/// passes it; (false, true), a seat not first, and (true, false), the first
/// in another line, leave it.
#[test]
fn a_claim_passes_only_from_its_first_in_its_line() {
    let claims = super::Claims::default();
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    let (line, other) = (Arc::new(Ending::default()), Arc::new(Ending::default()));
    let [a, b, c] = [(20, 1, 0), (21, 1, 1), (22, 2, 2)].map(|(w, id, t)| seat(w, (&table, id), (&line, t)));
    lock_builtin(&line.queue).seated.extend([(1, Arc::clone(&b)), (2, Arc::clone(&c))]);
    claims.claim((&table, 1), super::Claim { owner: 10, line: Arc::clone(&line), first: 20 });
    let first = |claims: &super::Claims| claims.of((&table, 1), 30, |_| true).map(|c| c.first);
    claims.pass_on(&b, (&line, &lock_builtin(&line.queue)));
    assert_eq!(first(&claims), Some(20), "not its first");
    claims.pass_on(&a, (&other, &lock_builtin(&other.queue)));
    assert_eq!(first(&claims), Some(20), "not its line");
    claims.pass_on(&a, (&line, &lock_builtin(&line.queue)));
    assert_eq!(first(&claims), Some(21), "to the next for the row");
    lock_builtin(&line.queue).seated.retain(|(t, _)| *t != 1);
    claims.pass_on(&b, (&line, &lock_builtin(&line.queue)));
    assert!(first(&claims).is_none(), "the last ends it");
}
