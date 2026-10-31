//! The buffer pool's claims of frames busy for a moment (RULES.md D30,
//! D38): frames the eviction policy declines to propose though unpinned,
//! and frames latched by reads that hit by their hints while the pool
//! goes on working, waited for rather than refused; and the wait bounded
//! however busy the pool, so a latch of the claiming thread's own is
//! refused in the end.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use turborel::disk::buffer::{BufferPool, Clock, Eviction, FileId};
use turborel::disk::io::OsFileSystem;
use turborel::disk::pager::PageId;
use turborel::error::Code;

use crate::buffer::{REFUSAL_LIMIT, get, pager, pool, put};
use crate::common::Scratch;

/// CLOCK, declining to propose a frame while `declines` is above 0, each
/// time counting it down: as CLOCK itself does when the frames it sweeps
/// are used again before its hand comes back to them.
struct Declining {
    declines: Arc<AtomicU32>,
    clock: Clock,
}

impl Eviction for Declining {
    fn name(&self) -> &'static str {
        "declining"
    }
    fn accessed(&self, frame: usize) {
        self.clock.accessed(frame);
    }
    fn victim(&self, evictable: &dyn Fn(usize) -> bool) -> Option<usize> {
        // Asked by one claim at a time in its test.
        let left = self.declines.load(Ordering::SeqCst);
        if left > 0 {
            self.declines.store(left.saturating_sub(1), Ordering::SeqCst);
            return None;
        }
        self.clock.victim(evictable)
    }
}

/// Verifies: [DESIGN] RULES.md D30, D38, buffer pool: a page not in the
/// pool, the only frame unpinned but declined by the eviction policy a
/// few times, waits and takes the frame rather than being refused; a
/// policy that declines every time is refused (SQLCODE -918) in the end,
/// not waited for for ever.
///
/// Regression test (R-TEST-7): CLOCK, its hand moved on by other claims
/// and the frames it spared used again meanwhile, proposed none of the
/// unpinned frames; the claim took that for every frame pinned, and a
/// read was refused at once on a busy server.
#[test]
fn claims_wait_for_frames_the_policy_declines() {
    let scratch = Scratch::new("buffer-declined");
    let declines = Arc::new(AtomicU32::new(0));
    let policy = Declining { declines: Arc::clone(&declines), clock: Clock::new(1) };
    let pool = BufferPool::new(1, Box::new(policy)).unwrap();
    let file = pool.add_pager(pager(&OsFileSystem, &scratch.path("p")));
    let (a, b) = (put(&pool, file, b"a"), put(&pool, file, b"b"));
    declines.store(3, Ordering::SeqCst);
    assert_eq!(get(&pool, file, a).unwrap(), b"a", "waited for, the frame is taken");
    assert_eq!(declines.load(Ordering::SeqCst), 0, "declined three times first");
    declines.store(u32::MAX, Ordering::SeqCst);
    assert_eq!(get(&pool, file, b).err().map(|e| e.code), Some(Code::Resources), "declined for ever");
}

/// How long a thread starved of the CPU holds a page read by its hint:
/// longer than a claim waits for a pool doing nothing else, shorter than
/// the most it waits.
const STARVED_HOLD: Duration = Duration::from_millis(100);
/// How long a thread reading a page over and over, by its hint, lets it
/// go between reads.
const REREAD_GAP: Duration = Duration::from_micros(100);

/// A thread reading page `id` over and over, by its hint, until `stop`.
fn reread(pool: &Arc<BufferPool>, file: FileId, id: PageId, stop: &Arc<AtomicBool>) -> thread::JoinHandle<()> {
    let (pool, stop) = (Arc::clone(pool), Arc::clone(stop));
    thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            // Refused or not once the frame is another page's: either way
            // it reads again.
            drop(pool.read(file, id.0));
            thread::sleep(REREAD_GAP);
        }
    })
}

/// Verifies: [DESIGN] RULES.md D30, D38, buffer pool: with the only frame
/// latched by a read by its hint held longer than a claim waits for a
/// pool doing nothing else, as by a thread starved of the CPU on a busy
/// server, while other reads go on, a read of another page waits for the
/// latch to be let go and takes the frame, rather than being refused.
///
/// Regression test (R-TEST-7): a claim waited a fixed number of rounds,
/// some 20 ms, however busy the pool, and was refused (SQLCODE -918)
/// while the holder waited for the CPU.
#[test]
fn claims_wait_for_latches_held_while_the_pool_works() {
    let scratch = Scratch::new("buffer-starved-latch");
    let pool = Arc::new(pool(1));
    let file = pool.add_pager(pager(&OsFileSystem, &scratch.path("p")));
    let (a, b) = (put(&pool, file, b"a"), put(&pool, file, b"b"));
    let (sender, receiver) = mpsc::channel();
    let holder = {
        let pool = Arc::clone(&pool);
        thread::spawn(move || {
            let held = pool.read(file, b.0).unwrap();
            assert_eq!((&*held, pool.counts().misses), (&b"b"[..], 0_u64), "a hit: by its hint");
            sender.send(()).unwrap();
            thread::sleep(STARVED_HOLD);
        })
    };
    receiver.recv_timeout(REFUSAL_LIMIT).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let reader = reread(&pool, file, b, &stop);
    assert_eq!(get(&pool, file, a).unwrap(), b"a", "waited for, the frame is taken");
    stop.store(true, Ordering::SeqCst);
    holder.join().unwrap();
    reader.join().unwrap();
}

/// Verifies: [DESIGN] RULES.md D30, D38, buffer pool: a thread holding the
/// only frame latched, read by its hint, that reads another page is
/// refused (SQLCODE -918) in the end, not waiting for itself for ever,
/// though another thread reads its page by its hint over and over, so the
/// pool keeps working.
#[test]
fn claims_on_a_latch_of_their_own_are_refused_however_busy_the_pool() {
    let scratch = Scratch::new("buffer-own-latch-busy");
    let pool = Arc::new(pool(1));
    let file = pool.add_pager(pager(&OsFileSystem, &scratch.path("p")));
    let (a, b) = (put(&pool, file, b"a"), put(&pool, file, b"b"));
    let stop = Arc::new(AtomicBool::new(false));
    let reader = reread(&pool, file, b, &stop);
    let (sender, receiver) = mpsc::channel();
    let claimer = {
        let pool = Arc::clone(&pool);
        thread::spawn(move || {
            let held = pool.read(file, b.0).unwrap();
            assert_eq!(&*held, b"b");
            let hits = pool.counts().hits;
            let refused = pool.read(file, a.0).err().map(|e| e.code);
            sender.send((refused, pool.counts().hits > hits)).unwrap();
        })
    };
    let (refused, worked) = receiver.recv_timeout(REFUSAL_LIMIT).expect("refused in the end, not waiting for ever");
    assert_eq!(refused, Some(Code::Resources), "held by the claiming thread");
    assert!(worked, "the pool worked meanwhile");
    stop.store(true, Ordering::SeqCst);
    claimer.join().unwrap();
    reader.join().unwrap();
}
