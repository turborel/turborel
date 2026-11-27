//! Unit tests of a run of morsels: who takes its morsels (RULES.md D1246),
//! how far it leads its reader (RULES.md D1262), and what a run dropped by
//! its reader lets go of (RULES.md D290, D1245).

use std::time::Duration;

use super::*;
use crate::db::Database;
use crate::exec::Db;
use crate::txn::NO_TRANSACTION;
use crate::value::{Decimal, Value};

/// A gate morsels wait at: how many wait, and whether it is open.
#[derive(Default)]
struct Gate {
    state: Mutex<(usize, bool)>,
    moved: Condvar,
}

impl Gate {
    /// Waits at the gate until it opens.
    fn pass(&self) {
        let mut state = self.state.lock().unwrap();
        state.0 = state.0.saturating_add(1);
        self.moved.notify_all();
        while !state.1 {
            state = self.moved.wait(state).unwrap();
        }
    }

    /// Waits until `n` morsels wait at the gate.
    fn waiting(&self, n: usize) {
        let mut state = self.state.lock().unwrap();
        while state.0 < n {
            state = self.moved.wait(state).unwrap();
        }
    }

    /// Opens the gate.
    fn open(&self) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).1 = true;
        self.moved.notify_all();
    }
}

/// Opens its gate as it is dropped, so a test that fails with morsels
/// waiting at the gate ends, and does not wait for them for ever.
struct Opening<'a>(&'a Gate);

impl Drop for Opening<'_> {
    fn drop(&mut self) {
        self.0.open();
    }
}

/// A run of `count` morsels of `degree`, each waiting at `gate`, with
/// no worker of a pool offered it.
fn gated(count: usize, degree: usize, gate: &Arc<Gate>) -> Run<usize> {
    let at = Arc::clone(gate);
    Run {
        work: RwLock::new(Some(Box::new(move |m| {
            at.pass();
            Ok(m)
        }))),
        count,
        degree,
        window: window(degree),
        stop: None,
        taken: AtomicUsize::new(0),
        running: AtomicUsize::new(0),
        given: AtomicUsize::new(0),
        failed: AtomicUsize::new(usize::MAX),
        ended: AtomicBool::new(false),
        done: Mutex::new(BTreeMap::new()),
        landed: Condvar::new(),
        pool: Arc::new(Pool::default()),
        led: None,
    }
}

/// Verifies: [DESIGN] RULES.md D1246 the workers of a pool run one
/// morsel fewer at once than a run's degree, and the reader's thread
/// the last. Of a run of degree 3 with two workers each in a morsel, a
/// third worker's step takes none and the run wants no worker, while
/// the reader still takes the next morsel; of degree 2, one worker in
/// a morsel is all it wants; and once a morsel ends a worker takes
/// another.
///
/// Decision: fewer workers run than the degree less one, or as many;
/// the taker is a worker, or the reader.
#[test]
fn workers_take_one_morsel_fewer_than_the_degree() {
    for degree in [3_usize, 2] {
        let gate = Arc::new(Gate::default());
        let run = gated(64, degree, &gate);
        let workers = degree.saturating_sub(1);
        std::thread::scope(|s| {
            let _opening = Opening(&gate);
            for inside in 1..=workers {
                assert!(run.wanted(), "a worker is wanted below the cap, degree {degree}");
                s.spawn(|| run.step());
                // Each waits in its morsel before the next is asked.
                gate.waiting(inside);
            }
            assert!(!run.wanted(), "no worker more is wanted, degree {degree}");
            run.step();
            assert_eq!(run.taken.load(Ordering::Acquire), workers, "a worker past the cap takes none, degree {degree}");
            assert_eq!(run.claim(), Some(workers), "the reader takes the next, degree {degree}");
        });
        assert!(run.wanted(), "a worker is wanted again once the morsels ended");
        run.step();
        assert_eq!(run.taken.load(Ordering::Acquire), workers.saturating_add(2));
    }
}

/// Verifies: [DESIGN] RULES.md D1262 a run whose reader asked a lead
/// takes morsels past its window, as many a thread as the lead says,
/// while the bytes of the results landed and not yet given are fewer
/// than there is room for, and no morsel past the window once they are
/// not; a result's bytes are held from its landing until the reader is
/// given it; and a run that asked none keeps to its window.
///
/// Decision: a lead is asked, or not; the bytes held leave room, or
/// not.
#[test]
fn a_run_leads_its_reader_while_what_it_holds_has_room() {
    let run_of = |room: usize| {
        let lead = Lead { wide: 16, size: |m: &usize| m.saturating_add(100), room: Box::new(move || room) };
        Run { led: Some(Led { lead, held: AtomicUsize::new(0) }), ..gated(200, 2, &Arc::new(Gate::default())) }
    };
    let taken = |run: &Run<usize>| std::iter::from_fn(|| run.claim()).count();
    assert_eq!(taken(&gated(200, 2, &Arc::new(Gate::default()))), window(2), "no lead asked");
    let roomy = run_of(1000);
    assert_eq!(taken(&roomy), 32, "sixteen a thread while there is room");
    let none = run_of(0);
    assert_eq!(taken(&none), window(2), "no room, the window alone");
    // Morsels 0 to 9 land, each of its number and a hundred bytes.
    let filling = run_of(1000);
    for m in 0..10_usize {
        filling.taken.store(m.saturating_add(1), Ordering::Release);
        done_of(&filling).insert(m, Ok(m));
        filling.led.as_ref().unwrap().held.fetch_add(filling.bytes_of(&Ok(m)), Ordering::AcqRel);
    }
    assert_eq!(filling.led.as_ref().unwrap().held.load(Ordering::Acquire), 1045);
    assert_eq!(filling.claim(), None, "ten are taken, past the window, and what landed leaves no room");
    assert_eq!(filling.next().map(Result::unwrap), Some(0));
    assert_eq!(filling.led.as_ref().unwrap().held.load(Ordering::Acquire), 945, "given, its bytes are held no longer");
    assert_eq!(filling.claim(), Some(10), "room again");
}

/// Verifies: [DESIGN] RULES.md D1246, D290 a run of degree 1 wants no
/// worker and is offered to none: its reader takes every morsel on
/// its own thread, in order.
#[test]
fn a_run_of_one_thread_is_its_readers_alone() {
    let db = Database::new();
    let state = Db::from(db.snapshot().unwrap());
    let reader = std::thread::current().id();
    let work = Box::new(move |m: usize| Ok((m, std::thread::current().id() == reader)));
    let run = Morsels::start(&state, NO_TRANSACTION, (40, 1), work);
    assert!(!run.run.wanted(), "no worker is wanted");
    let taken: Vec<(usize, bool)> = run.collect::<Result<_>>().unwrap();
    assert_eq!(taken, (0..40).map(|m| (m, true)).collect::<Vec<_>>());
}

/// Verifies: [DESIGN] (RULES.md D290, D1245) Regression test (R-TEST-7):
/// a run dropped by its reader after its first result left its work, and
/// what the work held, to the pool's task until a worker next woke, so a
/// statement returned with its workers still holding its lists. Dropped
/// now, it waits for the morsels running and lets go of its work and of
/// the results not read before the drop returns: each morsel and each
/// result holds the token, and only the test's own is left.
#[test]
fn a_run_dropped_lets_go_of_its_work_and_its_results_at_once() {
    let db = Database::new();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let (st, _) = db.session("A").optimized("SELECT K FROM T").unwrap();
    let token = Arc::new(());
    for _ in 0_usize..20 {
        let held = Arc::clone(&token);
        let work: Work<Arc<()>> = Box::new(move |_m: usize| {
            let running = Arc::clone(&held);
            std::thread::sleep(Duration::from_millis(1));
            Ok(running)
        });
        let mut run = Morsels::start(&st, NO_TRANSACTION, (1_000, 4), work);
        assert!(run.next().is_some_and(|first| first.is_ok()));
        drop(run);
        assert_eq!(Arc::strong_count(&token), 1, "nothing of the run left held");
    }
}

/// Verifies: [DESIGN] RULES.md D1262, D1245 a run that leads its reader,
/// with no room for what it would hold ahead, so its workers take no
/// morsel past the window and go back to the pool, dropped by its reader
/// after its first result, returns and leaves nothing held: neither its
/// work, nor a result not read, nor a byte counted ahead, and no morsel
/// was taken past the window.
#[test]
fn a_run_with_a_lead_and_no_room_dropped_early_holds_nothing() {
    let db = Database::new();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let (st, _) = db.session("A").optimized("SELECT K FROM T").unwrap();
    let token = Arc::new(());
    for _ in 0_usize..20 {
        let held = Arc::clone(&token);
        let work: Work<Arc<()>> = Box::new(move |_m: usize| {
            let running = Arc::clone(&held);
            std::thread::sleep(Duration::from_millis(1));
            Ok(running)
        });
        let lead = Lead { wide: 16, size: |_: &Arc<()>| 1, room: Box::new(|| 0) };
        let mut run = Morsels::led(&st, NO_TRANSACTION, (1_000, 4), work, Some(lead));
        assert!(run.next().is_some_and(|first| first.is_ok()));
        let shared = Arc::clone(&run.run);
        drop(run);
        assert_eq!(Arc::strong_count(&token), 1, "nothing of the run left held");
        assert_eq!(shared.led.as_ref().unwrap().held.load(Ordering::Acquire), 0, "no byte counted ahead");
        assert!(shared.taken.load(Ordering::Acquire) <= window(4).saturating_add(1), "none past the window");
    }
}
