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
//! INFORMATION_SCHEMA.MEMORY_RESERVATIONS shows them.
//!
//! The limit is the setting MEMORY_BUDGET's (`settings.rs`). The budget is
//! one for the database, shared by every snapshot of its definitions, and
//! told the setting's value whenever a change of it is published.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::disk::encoding::Encodings;
use crate::error::{Code, Result, SqlError};
use crate::files::plugins::PluginFiles;
use crate::spill::WorkAreas;
use crate::sync::lock_builtin;

/// \[DESIGN\] The default MEMORY_BUDGET: 256 MiB.
pub const DEFAULT_BUDGET: usize = 256 << 20;

/// \[DESIGN\] A database's memory budget: its limit, and what is reserved
/// of it. It may be shared between threads.
#[derive(Debug)]
pub struct MemoryBudget {
    limit: AtomicUsize,
    reserved: AtomicUsize,
    /// What holds each reservation, and its bytes, by the reservation's
    /// number.
    held: Mutex<BTreeMap<u64, (String, usize)>>,
    /// The number of the next reservation.
    next: AtomicU64,
}

impl MemoryBudget {
    /// A budget of `limit` bytes, none of it reserved.
    pub fn new(limit: usize) -> Arc<MemoryBudget> {
        Arc::new(MemoryBudget {
            limit: AtomicUsize::new(limit),
            reserved: AtomicUsize::new(0),
            held: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(0),
        })
    }

    /// What holds each reservation of it, and its bytes, in the order they
    /// were made.
    pub fn reservations(&self) -> Vec<(String, usize)> {
        lock_builtin(&self.held).values().cloned().collect()
    }

    /// Its limit, in bytes.
    pub fn limit(&self) -> usize {
        self.limit.load(Ordering::Acquire)
    }

    /// The bytes reserved of it.
    pub fn reserved(&self) -> usize {
        self.reserved.load(Ordering::Acquire)
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
        lock_builtin(&self.held).insert(id, (what.to_owned(), bytes));
        Ok(Reservation { budget: Arc::clone(self), bytes, id })
    }

    /// Counts `bytes` more reserved, for `what`, if they fit its limit.
    fn take(&self, bytes: usize, what: &str) -> Result<()> {
        let limit = self.limit();
        let mut reserved = self.reserved.load(Ordering::Acquire);
        loop {
            let Some(after) = reserved.checked_add(bytes).filter(|&n| n <= limit) else {
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
            let what = lock_builtin(&self.budget.held).get(&self.id).map(|(what, _)| what.clone()).unwrap_or_default();
            self.budget.take(more, &what)?;
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

    /// Notes that it holds `bytes`.
    fn held(&mut self, bytes: usize) {
        self.bytes = bytes;
        if let Some(held) = lock_builtin(&self.budget.held).get_mut(&self.id) {
            held.1 = bytes;
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        lock_builtin(&self.budget.held).remove(&self.id);
        self.budget.reserved.fetch_sub(self.bytes, Ordering::AcqRel);
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
}
