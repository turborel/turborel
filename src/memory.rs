//! \[DESIGN\] The memory budget of a database (RULES.md D38): a limit, in
//! bytes, on the memory its caches and work areas hold, which each draws on
//! by reserving a share. A buffer pool reserves its frames' pages as it is
//! made, and holds them until it is dropped. A reservation that would pass
//! the limit is refused (SQLCODE -918). Lowering the limit takes nothing
//! back: it limits the reservations made after.
//!
//! A plug-in is given the budget as it is registered (`Host`), so its caches
//! draw on the same limit as the built-in ones. Each reservation says what
//! holds it, and the budget lists those held (`reservations`), as
//! INFORMATION_SCHEMA.MEMORY_RESERVATIONS shows them. The list is kept in
//! shards chosen by each reservation's number, so sessions reserving at
//! once do not wait on one lock; a reservation's size is its own atomic,
//! read by the list without a lock.
//!
//! The limit is the setting MEMORY.BUDGET_BYTES's (`settings.rs`). The budget is
//! one for the database, shared by every snapshot of its definitions, and
//! told the setting's value whenever a change of it is published.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::disk::encoding::Encodings;
use crate::error::{Code, Result, SqlError};
use crate::files::plugins::PluginFiles;
use crate::spill::WorkAreas;
use crate::sync::lock_builtin;

/// \[DESIGN\] The default MEMORY.BUDGET_BYTES: 1 GiB, eight of the default
/// buffer pools (`db::POOL_BYTES`), as a database may hold several at once.
pub const DEFAULT_BUDGET: usize = 1 << 30;

/// \[DESIGN\] The shards of a budget's list of reservations (RULES.md D38).
/// Consecutive reservations fall in different shards, so sessions
/// reserving at once rarely meet on a lock; more than the threads a
/// machine runs gains nothing.
const SHARDS: usize = 64;

/// One reservation's entry in its budget's list: what holds it, and its
/// bytes, which the reservation updates without taking the shard's lock.
#[derive(Debug)]
struct Held {
    what: Box<str>,
    bytes: AtomicUsize,
    /// Whether it stands while the database is open (`Reservation::stand`).
    standing: AtomicBool,
}

/// A shard of the list: entries by their reservation's number.
type Shard = Mutex<BTreeMap<u64, Arc<Held>>>;

/// \[DESIGN\] A database's memory budget: its limit, and what is reserved
/// of it. It may be shared between threads.
#[derive(Debug)]
pub struct MemoryBudget {
    limit: AtomicUsize,
    reserved: AtomicUsize,
    /// The bytes of its standing reservations, held while the database is
    /// open, as its buffer pools (RULES.md D683).
    standing: AtomicUsize,
    /// The bytes its standing reservations are planned at: each at the
    /// bound it stands up to, else at its bytes (RULES.md D1159).
    planned: AtomicUsize,
    /// What holds each reservation, and its bytes, in the shard its
    /// number picks.
    held: [Shard; SHARDS],
    /// The number of the next reservation.
    next: AtomicU64,
}

impl MemoryBudget {
    /// A budget of `limit` bytes, none of it reserved.
    pub fn new(limit: usize) -> Arc<MemoryBudget> {
        Arc::new(MemoryBudget {
            limit: AtomicUsize::new(limit),
            reserved: AtomicUsize::new(0),
            standing: AtomicUsize::new(0),
            planned: AtomicUsize::new(0),
            held: std::array::from_fn(|_| Mutex::new(BTreeMap::new())),
            next: AtomicU64::new(0),
        })
    }

    /// What holds each reservation of it, and its bytes, in the order they
    /// were made: the shards merged by reservation number.
    pub fn reservations(&self) -> Vec<(String, usize)> {
        let mut listed: Vec<(u64, String, usize)> = Vec::new();
        for shard in &self.held {
            let entries = lock_builtin(shard);
            listed.extend(entries.iter().map(|(&id, held)| (id, held.what.to_string(), held.bytes.load(Ordering::Relaxed))));
        }
        listed.sort_unstable_by_key(|&(id, _, _)| id);
        listed.into_iter().map(|(_, what, bytes)| (what, bytes)).collect()
    }

    /// The shard reservation `id`'s entry is kept in.
    fn shard(&self, id: u64) -> Option<&Shard> {
        let count = u64::try_from(self.held.len()).ok()?;
        let at = usize::try_from(id.checked_rem(count)?).ok()?;
        self.held.get(at)
    }

    /// Its limit, in bytes.
    pub fn limit(&self) -> usize {
        self.limit.load(Ordering::Acquire)
    }

    /// The bytes reserved of it.
    pub fn reserved(&self) -> usize {
        self.reserved.load(Ordering::Acquire)
    }

    /// The bytes of its standing reservations (`Reservation::stand`).
    pub fn standing(&self) -> usize {
        self.standing.load(Ordering::Acquire)
    }

    /// The bytes its standing reservations are planned at
    /// (`Reservation::stand_up_to`): what the planner takes them as, the
    /// same however much a growing one holds now (RULES.md D1159).
    pub fn planned(&self) -> usize {
        self.planned.load(Ordering::Acquire)
    }

    /// Makes `limit` its limit, for the reservations made after.
    pub fn set_limit(&self, limit: usize) {
        self.limit.store(limit, Ordering::Release);
    }

    /// Reserves `bytes` of it for `what`, until the reservation is dropped.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if they would take what is reserved past its limit.
    pub fn reserve(self: &Arc<Self>, bytes: usize, what: &str) -> Result<Reservation> {
        self.take(bytes, what)?;
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let held = Arc::new(Held { what: what.into(), bytes: AtomicUsize::new(bytes), standing: AtomicBool::new(false) });
        if let Some(shard) = self.shard(id) {
            lock_builtin(shard).insert(id, Arc::clone(&held));
        }
        Ok(Reservation { budget: Arc::clone(self), bytes, bound: None, held, id })
    }

    /// Counts `bytes` more reserved, for `what`, if they fit its limit, or
    /// past it while this thread undoes work (`Undoing`).
    fn take(&self, bytes: usize, what: &str) -> Result<()> {
        let (limit, undoing) = (self.limit(), Undoing::active());
        let mut reserved = self.reserved.load(Ordering::Acquire);
        loop {
            let Some(after) = reserved.checked_add(bytes).filter(|&n| n <= limit || undoing) else {
                return Err(SqlError::new(
                    Code::Resources,
                    format!("[DESIGN] {what} needs {bytes} bytes, but {reserved} of the memory budget's {limit} are reserved"),
                ));
            };
            match self.reserved.compare_exchange_weak(reserved, after, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Ok(()),
                Err(now) => reserved = now,
            }
        }
    }
}

std::thread_local! {
    /// How many undos this thread is in (`Undoing`).
    static UNDOING: Cell<usize> = const { Cell::new(0) };
}

/// \[DESIGN\] RULES.md D38: while it is held, its thread undoes work, a
/// failed statement's changes or a transaction's at ROLLBACK, and what that
/// thread reserves of a budget is never refused, though it pass the limit
/// until the undo gives back what it held, as pages written back are
/// (`Reservation::resize_anyway`): undoing is always possible, where a
/// refusal would damage a table (D19). A buffer pool whose every frame is
/// held makes it wait for one rather than refuse it (`disk/buffer.rs`).
/// It stays with its thread, and undos may nest.
pub(crate) struct Undoing {
    /// Neither sent nor shared: it marks its own thread.
    _thread: PhantomData<*const ()>,
}

impl Undoing {
    /// Marks this thread undoing until it is dropped.
    pub(crate) fn begin() -> Undoing {
        UNDOING.with(|n| n.set(n.get().saturating_add(1)));
        Undoing { _thread: PhantomData }
    }

    /// Whether this thread is undoing.
    pub(crate) fn active() -> bool {
        UNDOING.with(|n| n.get() > 0)
    }
}

impl Drop for Undoing {
    fn drop(&mut self) {
        UNDOING.with(|n| n.set(n.get().saturating_sub(1)));
    }
}

/// \[DESIGN\] What a database provides the plug-ins registered with it
/// (`StorageEngine::attach`, `IndexMethod::attach`): its memory budget,
/// which their caches and work areas reserve of, its spill areas, the
/// encodings registered with it, and a directory for each of their own. More may be added, so it is made only by
/// the database.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Host {
    /// The database's memory budget.
    pub memory: Arc<MemoryBudget>,
    /// Where the database's spill areas are made (`spill.rs`), for work
    /// that outgrows memory.
    pub work: Arc<WorkAreas>,
    /// The encodings registered (`Database::register_encoding`): those a
    /// table's options name, its data is sealed with (RULES.md D30, D41).
    pub encodings: Arc<Encodings>,
    /// Where it keeps files of its own (RULES.md D56); its scratch files
    /// are those of `work` (`WorkAreas::scratch`).
    pub files: Arc<PluginFiles>,
}

impl Host {
    /// What a database whose budget is `memory`, whose spill areas are
    /// `work`, whose encodings are `encodings` and whose plug-ins' files
    /// are kept in `files` provides.
    pub(crate) fn new(memory: Arc<MemoryBudget>, work: Arc<WorkAreas>, encodings: Arc<Encodings>, files: Arc<PluginFiles>) -> Host {
        Host { memory, work, encodings, files }
    }
}

/// Bytes reserved of a memory budget, given back when dropped.
#[derive(Debug)]
pub struct Reservation {
    budget: Arc<MemoryBudget>,
    bytes: usize,
    /// The bytes it is planned at while it stands, if fixed
    /// (`stand_up_to`); else its bytes.
    bound: Option<usize>,
    /// Its entry in the budget's list, shared with it.
    held: Arc<Held>,
    /// Its number in the budget's list.
    id: u64,
}

impl Reservation {
    /// The bytes it holds.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Makes it hold `bytes`: more reserved of its budget, or what it no
    /// longer needs given back.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the more would take what is reserved past the
    /// budget's limit; it then holds what it held.
    pub fn resize(&mut self, bytes: usize) -> Result<()> {
        if let Some(more) = bytes.checked_sub(self.bytes) {
            self.budget.take(more, &self.held.what)?;
        } else {
            self.budget.reserved.fetch_sub(self.bytes.saturating_sub(bytes), Ordering::AcqRel);
        }
        self.held(bytes);
        Ok(())
    }

    /// Makes it hold `bytes`, even past its budget's limit: for what is
    /// already held, as a page written back that was not reserved, which
    /// refusing would lose.
    pub fn resize_anyway(&mut self, bytes: usize) {
        if let Some(more) = bytes.checked_sub(self.bytes) {
            self.budget.reserved.fetch_add(more, Ordering::AcqRel);
        } else {
            self.budget.reserved.fetch_sub(self.bytes.saturating_sub(bytes), Ordering::AcqRel);
        }
        self.held(bytes);
    }

    /// \[DESIGN\] (RULES.md D683) Counts it among its budget's standing
    /// reservations, held while the database is open, as a buffer pool,
    /// not by a statement: what work grants share half of the rest of.
    pub fn stand(&mut self) {
        if !self.held.standing.swap(true, Ordering::AcqRel) {
            self.budget.standing.fetch_add(self.bytes, Ordering::AcqRel);
            self.budget.planned.fetch_add(self.bytes, Ordering::AcqRel);
        }
    }

    /// \[DESIGN\] (RULES.md D1159) Stands as `stand` does, one that grows
    /// and shrinks with the work, planned at `bound`, the most it may
    /// hold, whatever it holds now.
    pub fn stand_up_to(&mut self, bound: usize) {
        if !self.held.standing.swap(true, Ordering::AcqRel) {
            self.bound = Some(bound);
            self.budget.standing.fetch_add(self.bytes, Ordering::AcqRel);
            self.budget.planned.fetch_add(bound, Ordering::AcqRel);
        }
    }

    /// Notes that it holds `bytes`, in its entry too, without a lock: the
    /// list reads the entry's atomic.
    fn held(&mut self, bytes: usize) {
        if self.held.standing.load(Ordering::Acquire) {
            self.budget.standing.fetch_add(bytes, Ordering::AcqRel);
            self.budget.standing.fetch_sub(self.bytes, Ordering::AcqRel);
            if self.bound.is_none() {
                self.budget.planned.fetch_add(bytes, Ordering::AcqRel);
                self.budget.planned.fetch_sub(self.bytes, Ordering::AcqRel);
            }
        }
        self.bytes = bytes;
        self.held.bytes.store(bytes, Ordering::Relaxed);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(shard) = self.budget.shard(self.id) {
            lock_builtin(shard).remove(&self.id);
        }
        self.budget.reserved.fetch_sub(self.bytes, Ordering::AcqRel);
        if self.held.standing.load(Ordering::Acquire) {
            self.budget.standing.fetch_sub(self.bytes, Ordering::AcqRel);
            self.budget.planned.fetch_sub(self.bound.unwrap_or(self.bytes), Ordering::AcqRel);
        }
    }
}

// Unit tests: reservations within the limit, and given back (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D38: reservations are granted while they
    /// fit the limit and refused past it (SQLCODE -918); a dropped one is
    /// given back; a lower limit takes nothing back, limiting those after.
    #[test]
    fn reservations_fit_the_limit() {
        let budget = MemoryBudget::new(100);
        let a = budget.reserve(60, "a").unwrap();
        assert_eq!(budget.reserve(41, "b").err().map(|e| e.code), Some(Code::Resources));
        let b = budget.reserve(40, "b").unwrap();
        assert_eq!((budget.reserved(), a.bytes(), b.bytes()), (100, 60, 40));
        assert_eq!(budget.reservations(), [("a".to_owned(), 60), ("b".to_owned(), 40)], "each with what holds it");
        drop(a);
        assert_eq!(budget.reservations(), [("b".to_owned(), 40)]);
        assert_eq!(budget.reserved(), 40);
        budget.set_limit(30);
        assert_eq!(budget.reserved(), 40, "nothing taken back");
        assert!(budget.reserve(1, "c").is_err());
        drop(b);
        assert!(budget.reserve(30, "c").is_ok());
        assert!(budget.reserve(usize::MAX, "d").is_err(), "no overflow");
    }

    /// Verifies: [DESIGN] RULES.md D38, D56: a reservation grows while it
    /// fits the limit, and is refused past it holding what it held
    /// (SQLCODE -918), unless made to grow anyway; it shrinks, giving back
    /// what it no longer needs; the budget lists it at its size.
    #[test]
    fn a_reservation_grows_and_shrinks() {
        let budget = MemoryBudget::new(100);
        let mut a = budget.reserve(10, "a").unwrap();
        a.resize(70).unwrap();
        assert_eq!((budget.reserved(), a.bytes()), (70, 70));
        assert_eq!(a.resize(101).err().map(|e| e.code), Some(Code::Resources));
        assert_eq!((budget.reserved(), a.bytes()), (70, 70), "held as it was");
        a.resize(20).unwrap();
        assert_eq!(budget.reservations(), [("a".to_owned(), 20)]);
        a.resize_anyway(120);
        assert_eq!(budget.reserved(), 120, "past the limit");
        assert!(budget.reserve(1, "b").is_err());
        a.resize_anyway(20);
        drop(a);
        assert_eq!(budget.reserved(), 0);
    }

    /// Verifies: [DESIGN] RULES.md D38: the list of reservations keeps the
    /// order they were made in though they are kept in shards, the 65th
    /// sharing the first's shard; each shows its size after it changes;
    /// one dropped leaves the list, the others keep their places.
    #[test]
    fn the_list_keeps_creation_order_across_shards() {
        let budget = MemoryBudget::new(usize::MAX);
        let count = SHARDS.saturating_add(2);
        let mut held: Vec<_> = (0..count).map(|n| budget.reserve(n, &format!("r{n}")).unwrap()).collect();
        held[1].resize(1000).unwrap();
        held.remove(2);
        let expected: Vec<_> = (0..count).filter(|&n| n != 2).map(|n| (format!("r{n}"), if n == 1 { 1000 } else { n })).collect();
        assert_eq!(budget.reservations(), expected, "in creation order, at their sizes");
        drop(held);
        assert_eq!(budget.reservations(), Vec::<(String, usize)>::new());
        assert_eq!(budget.reserved(), 0);
    }

    /// Verifies: [DESIGN] RULES.md D38: a reservation refused more bytes
    /// names what holds it in its error (SQLCODE -918), as one refused at
    /// first does.
    #[test]
    fn a_refused_growth_names_its_holder() {
        let budget = MemoryBudget::new(100);
        let mut a = budget.reserve(10, "the frontier").unwrap();
        let refused = a.resize(101).unwrap_err();
        assert_eq!(refused.code, Code::Resources);
        assert!(refused.msg.contains("the frontier needs 91 bytes"), "{}", refused.msg);
    }

    /// Verifies: [DESIGN] RULES.md D38: sessions reserving, growing and
    /// giving back at once are all counted, and leave nothing reserved or
    /// listed when done.
    #[test]
    fn sessions_reserve_at_once() {
        let budget = MemoryBudget::new(usize::MAX);
        std::thread::scope(|threads| {
            for t in 0..8_usize {
                let budget = &budget;
                threads.spawn(move || churn(budget, t));
            }
        });
        assert_eq!(budget.reserved(), 0);
        assert_eq!(budget.reservations(), Vec::<(String, usize)>::new());
    }

    /// Reserves, grows and gives back 500 times, as one session would.
    fn churn(budget: &Arc<MemoryBudget>, t: usize) {
        for n in 0..500_usize {
            let mut r = budget.reserve(n, "work").unwrap();
            r.resize(n.saturating_add(t)).unwrap();
        }
    }

    /// Verifies: [DESIGN] RULES.md D19, D38: while its thread undoes work,
    /// a reservation, or a reservation's growth, past the limit is granted,
    /// counted past it and given back as it goes; undos nest; once the
    /// undo ends, and on another thread meanwhile, the limit holds.
    #[test]
    fn what_undoes_is_never_refused() {
        let budget = MemoryBudget::new(100);
        let mut a = budget.reserve(90, "a").unwrap();
        let undoing = Undoing::begin();
        let nested = Undoing::begin();
        drop(nested);
        let b = budget.reserve(50, "b").unwrap();
        a.resize(120).unwrap();
        assert_eq!(budget.reserved(), 170, "past the limit");
        let other = Arc::clone(&budget);
        let elsewhere = std::thread::spawn(move || other.reserve(1, "c").map(drop).err().map(|e| e.code)).join().unwrap();
        assert_eq!(elsewhere, Some(Code::Resources), "another thread is refused");
        drop(undoing);
        assert!(!Undoing::active());
        assert_eq!(budget.reserve(1, "c").err().map(|e| e.code), Some(Code::Resources));
        drop((a, b));
        assert_eq!(budget.reserved(), 0);
    }
}
