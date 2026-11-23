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

/// How many times `until_seated` looks, a millisecond apart, before it
/// fails: ten seconds. A waiter seats once its thread runs and takes three
/// locks, which a loaded machine delays by its scheduling periods, tens of
/// milliseconds at most; so a waiter not seated by then never will be, as
/// when its wait was refused at once, and the test fails instead of
/// spinning forever.
const SEAT_LOOKS: u32 = 10_000;

/// Waits until `ending` has `count` seated, failing once `SEAT_LOOKS`
/// looks found fewer.
fn until_seated(ending: &Ending, count: usize) {
    seated_within(ending, count, SEAT_LOOKS);
}

/// Waits until `ending` has `count` seated, failing once `looks` looks
/// found fewer.
fn seated_within(ending: &Ending, count: usize, looks: u32) {
    for _ in 0..looks {
        if seated(ending).len() >= count {
            return;
        }
        thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("{count} waiters never seated: {:?} are", seated(ending));
}

/// Verifies: [DESIGN] R-TEST-5, R-TEST-7: a wait for waiters to seat that
/// never comes fails, rather than spinning forever. Regression test: two
/// runs hung for hours when the waiters' transactions were no-wait, so
/// each wait was refused at once and never seated.
#[test]
#[should_panic(expected = "never seated")]
fn a_wait_for_seats_that_never_come_fails() {
    seated_within(&Ending::default(), 1, 1);
}

/// A transaction of `st` that waits for locks.
fn waiting(st: &State) -> Txn {
    st.begin_waiting((LockWait::Wait, Reading::Serializable, Arc::default()), &mut None)
}

/// The end of open transaction `tx`.
fn end_of(tx: &Txn) -> Arc<Ending> {
    tx.origin.shared.ends.of(tx.id).unwrap()
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
        let graph = lock_builtin(&first.origin.shared.waits_for).get(&second.id).cloned();
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
        lock_builtin(&first.origin.shared.waits_for).insert(first.id, vec![cyclic.id]);
        super::Turn::hand_over(Some(&firsts), &first, (&table, 1));
        lock_builtin(&first.origin.shared.waits_for).remove(&first.id);
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

/// Verifies: [DESIGN] RULES.md D456, D851: while a row's writer is open, a
/// newcomer finds the row claimed for its first waiter and queues behind
/// it; once the writer ends, the claim holds back no one, so a newcomer
/// running writes the row while the first waiter, which has its turn, has
/// not written yet.
#[test]
fn a_newcomer_takes_a_row_its_first_waiter_has_not_written() {
    let st = State::with_builtins(Box::new(MemoryCatalog::default()));
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    let [holder, first, newcomer] = std::array::from_fn(|_| waiting(&st));
    let held = end_of(&holder);
    let (open, own, ended) = thread::scope(|s| {
        let a = s.spawn(|| first.wait_for_row((&table, 1), holder.id));
        until_seated(&held, 1);
        let open = newcomer.claim_of((&table, 1)).map(|c| (c.owner, c.first, c.firm));
        let own = first.claim_of((&table, 1)).is_none();
        st.rollback(&holder).unwrap();
        let firsts = a.join().unwrap().unwrap().unwrap();
        let ended = newcomer.claim_of((&table, 1)).is_none();
        drop(firsts);
        (open, own, ended)
    });
    for tx in [&first, &newcomer] {
        st.rollback(tx).unwrap();
    }
    assert_eq!(open, Some((holder.id, first.id, false)), "claimed for the first waiter while its writer is open");
    assert!(own, "the first waiter is not held by its own claim");
    assert!(ended, "once the writer ends, the newcomer goes on");
}

/// Verifies: [DESIGN] RULES.md D851: a first waiter overtaken at its turn
/// queues first in the overtaker's line, the waiter behind it moved there
/// unwoken after it, and claims the row firmly: a later newcomer queues
/// behind it even once the overtaker has ended, and the overtaken goes
/// first.
#[test]
fn a_waiter_overtaken_once_is_never_overtaken_again() {
    let st = State::with_builtins(Box::new(MemoryCatalog::default()));
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    let [holder, first, second, overtaker, late] = std::array::from_fn(|_| waiting(&st));
    let held = end_of(&holder);
    let (line, firm, held_back, went) = thread::scope(|s| {
        let a = s.spawn(|| first.wait_for_row((&table, 1), holder.id));
        until_seated(&held, 1);
        let b = s.spawn(|| second.wait_for_row((&table, 1), holder.id).map(|turn| turn.is_some()));
        until_seated(&held, 2);
        st.rollback(&holder).unwrap();
        let firsts = a.join().unwrap().unwrap().unwrap();
        let ahead = end_of(&overtaker);
        let c = s.spawn(|| first.overtaken(firsts, (&table, 1), overtaker.id));
        until_seated(&ahead, 2);
        let line = seated(&ahead);
        let firm = late.claim_of((&table, 1)).map(|claim| (claim.first, claim.firm));
        st.rollback(&overtaker).unwrap();
        let held_back = late.claim_of((&table, 1)).map(|claim| claim.first);
        let went = c.join().unwrap().unwrap().is_some();
        st.rollback(&first).unwrap();
        (line, firm, held_back, went && b.join().unwrap().unwrap())
    });
    for tx in [&second, &late] {
        st.rollback(tx).unwrap();
    }
    assert_eq!(line, [first.id, second.id], "first in the overtaker's line, the next behind it");
    assert_eq!(firm, Some((first.id, true)), "claimed firmly");
    assert_eq!(held_back, Some(first.id), "held back once the overtaker ended");
    assert!(went);
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
        // The claims themselves: once the writer ended, a newcomer passes
        // an unfirm claim by (RULES.md D851).
        let claims = &newcomer.origin.shared.ends.claims;
        let passed = claims.of((&table, 1), newcomer.id, |_| true).map(|c| c.first);
        drop(b.join().unwrap().unwrap());
        (passed, claims.of((&table, 1), newcomer.id, |_| true).is_none())
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
    let place = super::Place { ending: Arc::clone(ending), ticket, turn: false, lost: false };
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
    claims.claim((&table, 1), super::Claim { owner: 10, line: Arc::clone(&line), first: 20, firm: false });
    claims.claim((&table, 1), super::Claim { owner: 11, line: Arc::clone(&line), first: 21, firm: false });
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
    claims.claim((&table, 1), super::Claim { owner: 10, line: Arc::clone(&line), first: 20, firm: false });
    let first = |of: &super::Claims| of.of((&table, 1), 30, |_| true).map(|claim| claim.first);
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
