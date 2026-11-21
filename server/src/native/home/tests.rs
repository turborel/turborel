//! The tests of a connection's home: where its jobs run, and when it retires.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::ThreadId;

use super::*;
use crate::limit::Limit;

/// A job that tells which thread ran it.
struct Noted(Sender<ThreadId>);

impl Job for Noted {
    fn run(self) {
        self.0.send(std::thread::current().id()).unwrap();
    }
}

/// A job that panics as it runs.
struct Panics;

impl Job for Panics {
    fn run(self) {
        panic!("a job that fails");
    }
}

/// A job that tells it was dropped, run or not.
struct Dropped(Sender<()>);

impl Job for Dropped {
    fn run(self) {}
}

impl Drop for Dropped {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

/// A runtime with a blocking pool, as the server's.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(1).build().unwrap()
}

/// The places of a limit of one, and the one place taken.
fn place() -> (Arc<Limit>, Held) {
    let places = Arc::new(Limit::default());
    let held = places.try_take(1).unwrap();
    (places, held)
}

/// \[DESIGN\] How many times `until` looks before it fails: half of
/// `KEPT` in all, so a home that would retire only once idle that long is
/// not taken for one that retired at once.
const LOOKS: u32 = 5_000;

/// Waits until `done` holds, looking every millisecond: the home's thread
/// acts on its own. Fails if it never does.
fn until(done: impl Fn() -> bool) {
    let held = (0..LOOKS).any(|_| {
        std::thread::sleep(Duration::from_millis(1));
        done()
    });
    assert!(held, "it never came to hold");
}

/// Runs a job noting its thread on `home`, as the connection's task hands
/// one at a time: the thread that ran it.
fn ran_on<J: Job>(
    home: &Home<J>,
    job: impl FnOnce(Sender<ThreadId>) -> J,
    (tell, told): (&Sender<ThreadId>, &Receiver<ThreadId>),
) -> ThreadId {
    assert!(home.send(job(tell.clone())).is_ok(), "the home takes it");
    told.recv().unwrap()
}

/// Verifies: [DESIGN] RULES.md D767: every job handed to a connection's
/// home runs on the one thread, not the thread that hands it, however
/// many threads the pool has free; two homes run on two threads.
#[test]
fn a_homes_jobs_run_on_its_one_thread() {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (tell, told) = channel();
    let places = Arc::new(Limit::default());
    let first: Home<Noted> = Home::open(places.try_take(2).unwrap(), KEPT);
    let second: Home<Noted> = Home::open(places.try_take(2).unwrap(), KEPT);
    let home = ran_on(&first, Noted, (&tell, &told));
    for _ in 0_u8..16 {
        // Another thread of the pool busy and free between them, as
        // another connection's request would have it.
        runtime.block_on(runtime.spawn_blocking(|| ())).unwrap();
        assert_eq!(ran_on(&first, Noted, (&tell, &told)), home);
    }
    assert_ne!(home, std::thread::current().id());
    assert_ne!(ran_on(&second, Noted, (&tell, &told)), home, "a home each");
}

/// Verifies: [DESIGN] RULES.md D768: a home with no job for its time
/// retires, gives its place back and takes no more jobs, the job given
/// back to whoever handed it; a home closed by its connection retires at
/// once, its place given back.
#[test]
fn a_home_retires_once_idle_or_closed() {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (tell, told) = channel();
    let (places, held) = place();
    let idle: Home<Noted> = Home::open(held, Duration::from_millis(1));
    until(|| places.held() == 0);
    let Err(back) = idle.send(Noted(tell.clone())) else { panic!("retired, it takes none") };
    back.run();
    assert!(told.recv().is_ok(), "the job given back whole");
    let (kept, at) = place();
    let closed: Home<Noted> = Home::open(at, KEPT);
    ran_on(&closed, Noted, (&tell, &told));
    assert_eq!(kept.held(), 1, "held while it lives");
    drop(closed);
    until(|| kept.held() == 0);
}

/// Verifies: [DESIGN] RULES.md D768: a home whose job panics retires, so
/// the connection's next job is given back to run elsewhere rather than
/// handed to a thread that has gone; a job handed over that the thread
/// never ran, as a runtime stopping leaves it, is dropped as the home
/// retires, so what it holds is given back.
#[test]
fn a_home_that_failed_takes_no_job() {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (places, held) = place();
    let failing: Home<Panics> = Home::open(held, KEPT);
    assert!(failing.send(Panics).is_ok());
    until(|| places.held() == 0);
    assert!(failing.send(Panics).is_err(), "retired");
    let (tell, told) = channel();
    let (kept, at) = place();
    let shared = Arc::new(Shared { state: Mutex::new(State { job: None, retired: false, closed: false }), told: Condvar::new() });
    let home = Home { shared: Arc::clone(&shared) };
    let thread = Thread { shared, kept: KEPT, _place: at };
    assert!(home.send(Dropped(tell)).is_ok());
    drop(thread);
    assert!(told.recv().is_ok(), "dropped unrun");
    assert_eq!(kept.held(), 0);
    assert!(home.send(Dropped(channel().0)).is_err(), "retired");
}
