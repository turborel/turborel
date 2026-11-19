//! \[DESIGN\] What a caller gives an algorithm besides its graph: how many
//! threads it may run on, the bytes it may hold, and what tells it to
//! stop. The budget counts the arrays an algorithm makes, each charged
//! before it is made and given back when it is dropped, so a run past
//! its bytes fails cleanly before it allocates.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::error::{Code, Result, SqlError};

/// What tells a running algorithm to stop: a check it makes between
/// pieces of its work, whose error it returns.
pub trait Cancel: Sync {
    /// Whether the run may go on.
    ///
    /// # Errors
    ///
    /// The caller's, once the run is to stop.
    fn check(&self) -> Result<()>;
}

impl<F: Fn() -> Result<()> + Sync> Cancel for F {
    fn check(&self) -> Result<()> {
        self()
    }
}

/// A run that is never told to stop.
#[derive(Debug, Clone, Copy, Default)]
pub struct Never;

impl Cancel for Never {
    fn check(&self) -> Result<()> {
        Ok(())
    }
}

/// The bytes an algorithm's run may hold at once, and those it holds.
#[derive(Debug)]
pub struct Memory {
    limit: usize,
    held: AtomicUsize,
    peak: AtomicUsize,
}

impl Memory {
    /// A budget of `limit` bytes.
    pub fn new(limit: usize) -> Memory {
        Memory { limit, held: AtomicUsize::new(0), peak: AtomicUsize::new(0) }
    }

    /// Its limit.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// The most bytes held at once so far.
    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::Acquire)
    }

    /// Charges `bytes` for `what`, given back as the charge drops.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if they would take what is held past the limit.
    pub fn charge(&self, bytes: usize, what: &str) -> Result<Charge<'_>> {
        let mut held = self.held.load(Ordering::Acquire);
        loop {
            let Some(after) = held.checked_add(bytes).filter(|&n| n <= self.limit) else {
                return Err(SqlError::new(
                    Code::Resources,
                    format!("[DESIGN] the graph algorithm needs {bytes} bytes for {what}, but {held} of its {} are held", self.limit),
                ));
            };
            match self.held.compare_exchange_weak(held, after, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {
                    self.peak.fetch_max(after, Ordering::AcqRel);
                    return Ok(Charge { memory: self, bytes });
                }
                Err(now) => held = now,
            }
        }
    }

    /// Charges `count` items of `T` for `what`.
    ///
    /// # Errors
    ///
    /// As `charge`; SQLCODE -918 too if their bytes overflow.
    pub fn items<T>(&self, count: usize, what: &str) -> Result<Charge<'_>> {
        let bytes = count.checked_mul(size_of::<T>()).ok_or_else(|| {
            SqlError::new(Code::Resources, format!("[DESIGN] the graph algorithm's {what} has more bytes than an address holds"))
        })?;
        self.charge(bytes, what)
    }
}

/// Bytes held of a `Memory`, given back as it drops.
#[derive(Debug)]
pub struct Charge<'a> {
    memory: &'a Memory,
    bytes: usize,
}

impl Charge<'_> {
    /// The bytes it holds.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for Charge<'_> {
    fn drop(&mut self) {
        self.memory.held.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// What a caller gives a run: its width, its budget and its stop.
#[derive(Debug, Clone, Copy)]
pub struct Control<'a, C> {
    /// The threads it may run on at once, the caller's own among them; 0
    /// is taken as 1.
    pub width: usize,
    /// The bytes it may hold.
    pub memory: &'a Memory,
    /// What tells it to stop.
    pub cancel: &'a C,
}

impl<'a, C: Cancel> Control<'a, C> {
    /// A run of `width` threads, `memory` and `cancel`.
    pub fn new(width: usize, memory: &'a Memory, cancel: &'a C) -> Control<'a, C> {
        Control { width, memory, cancel }
    }

    /// Its width, at least 1.
    pub(crate) fn threads(&self) -> usize {
        self.width.max(1)
    }

    /// Whether the run may go on.
    ///
    /// # Errors
    ///
    /// The cancel's.
    pub(crate) fn check(&self) -> Result<()> {
        self.cancel.check()
    }
}
