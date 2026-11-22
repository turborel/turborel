//! Unit tests of a table's escrow (`txn/escrow.rs`): what runs as
//! exclusive of a row runs alone, of that row's shard only (RULES.md
//! R-TEST-1).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::*;

/// Verifies: [DESIGN] RULES.md D452: while one look and write of a row run
/// as exclusive of it, a second of the same row waits until the first is
/// done; one of a row of another shard goes on meanwhile.
#[test]
fn what_runs_exclusive_of_a_row_runs_alone() {
    let escrow = Arc::new(Escrow::default());
    let inside = Arc::new(AtomicBool::new(false));
    let (entered, first_in) = mpsc::channel();
    let (release, go_on) = mpsc::channel::<()>();
    let first = {
        let (escrow, inside) = (Arc::clone(&escrow), Arc::clone(&inside));
        thread::spawn(move || {
            escrow.exclusive(7, || {
                inside.store(true, Ordering::SeqCst);
                entered.send(()).unwrap();
                go_on.recv().unwrap();
                inside.store(false, Ordering::SeqCst);
            });
        })
    };
    first_in.recv().unwrap();
    assert!(escrow.exclusive(8, || inside.load(Ordering::SeqCst)), "another shard goes on");
    let second = {
        let (escrow, inside) = (Arc::clone(&escrow), Arc::clone(&inside));
        thread::spawn(move || escrow.exclusive(7 + 32, || inside.load(Ordering::SeqCst)))
    };
    thread::sleep(Duration::from_millis(50));
    assert!(!second.is_finished(), "the same shard waits");
    release.send(()).unwrap();
    first.join().unwrap();
    assert!(!second.join().unwrap(), "run once the first was done");
}
