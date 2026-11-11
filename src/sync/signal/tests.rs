// Unit tests of `Signal`: waiters are counted while they wait, and a
// notify reaches each (RULES.md R-TEST-1, R-TEST-2).

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::*;

/// The threads a test starts waiting.
const WAITERS: usize = 3;
/// A wait that ends by its timeout, not a notify.
const SHORT: Duration = Duration::from_millis(5);
/// How long a test waits for threads to wait or wake before it fails,
/// rather than hang, as a notify lost would make it.
const PATIENCE: Duration = Duration::from_secs(10);

/// Yields until `signal` counts `n` waiters; fails past `PATIENCE`.
fn until_waiting(signal: &Signal, n: usize) {
    let start = Instant::now();
    while signal.waiting() != n {
        assert!(start.elapsed() < PATIENCE, "{} waiting, not {n}", signal.waiting());
        thread::yield_now();
    }
}

/// Verifies: [DESIGN] RULES.md D27, signals: threads waiting on a
/// condition are counted while they wait, a notify of all wakes each once
/// the condition is changed under the mutex, and none is counted after.
/// MC/DC of a notify, some thread waiting or not: the notify with three
/// waiting wakes them, the one after with none waiting changes nothing.
#[test]
fn a_notify_wakes_each_thread_counted_waiting() {
    let shared = Arc::new((Mutex::new(false), Signal::default()));
    let waiters: Vec<_> = (0..WAITERS)
        .map(|_| {
            let shared = Arc::clone(&shared);
            thread::spawn(move || {
                let (flag, signal) = &*shared;
                drop(signal.wait_while(flag.lock().unwrap(), |set| !*set).unwrap());
            })
        })
        .collect();
    let (flag, signal) = &*shared;
    until_waiting(signal, WAITERS);
    *flag.lock().unwrap() = true;
    signal.notify_all();
    until_waiting(signal, 0);
    for waiter in waiters {
        waiter.join().unwrap();
    }
    assert_eq!(signal.waiting(), 0);
    signal.notify_all();
    assert_eq!(signal.waiting(), 0, "a notify with none waiting");
}

/// Verifies: [DESIGN] RULES.md D27, signals: a condition already false is
/// not waited for, so nothing is counted; one waiter is woken by a notify
/// of one; a wait with a timeout ends uncounted.
#[test]
fn a_wait_counts_only_while_it_waits() {
    let shared = Arc::new((Mutex::new(0_u8), Signal::default()));
    let (count, signal) = &*shared;
    drop(signal.wait_while(count.lock().unwrap(), |n| *n > 0).unwrap());
    assert_eq!(signal.waiting(), 0, "no wait");
    let waiter = {
        let shared = Arc::clone(&shared);
        thread::spawn(move || {
            let (shared_count, shared_signal) = &*shared;
            let mut held = shared_count.lock().unwrap();
            while *held == 0 {
                held = shared_signal.wait(held).unwrap();
            }
        })
    };
    until_waiting(signal, 1);
    *count.lock().unwrap() = 1;
    signal.notify_one();
    until_waiting(signal, 0);
    waiter.join().unwrap();
    let (held, timed) = signal.wait_timeout(count.lock().unwrap(), SHORT).unwrap();
    drop(held);
    assert!(timed.timed_out());
    assert_eq!(signal.waiting(), 0);
}
