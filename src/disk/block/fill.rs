//! \[DESIGN\] Zeros written ahead of a block file's appends (RULES.md D25,
//! D30), for a file synced after each append, as a log is.
//!
//! A sync of a file that grew must also make its new size durable, which
//! on a journaling file system commits the journal as well as the data:
//! measured on ext4, a sync after a small append took about twice as long
//! as one after a write over bytes already there. So a file given
//! `BlockOptions::ahead` keeps that many bytes past its blocks written with
//! zeros, and an append writes over them. The zeros are refilled once
//! appends come within half of `ahead` of their end, by one appender while
//! the others go on; an append past them waits for the refill. Zeros are
//! written only past `filled`, and blocks only below it, so a refill never
//! writes over a block. Opening takes zeros from the last valid block to
//! the end of the file as this fill, not as a torn write (`tail.rs`), and
//! `trim` cuts them off a file appended to no more.

use std::sync::atomic::Ordering;
use std::sync::{PoisonError, TryLockError};

use super::BlockFile;
use crate::error::Result;
use crate::sync::lock_builtin;

/// \[DESIGN\] Zeros are refilled once fewer than `ahead` divided by this
/// remain past an append: half, so a refill writes about half of `ahead`
/// at a time, and an appender seldom finds them short and waits.
const REFILL_SHARE: u64 = 2;

impl BlockFile {
    /// Makes sure zeros lie under the block appended at `..end` before it
    /// is written, refilling them ahead when they run short: waiting for
    /// a refill if they do not reach `end`, else refilling only if no
    /// other appender is.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the zeros cannot be written: the append fails.
    pub(super) fn fill_ahead(&self, end: u64) -> Result<()> {
        let ahead = self.options.ahead;
        if ahead == 0 || self.ahead_of(end) {
            return Ok(());
        }
        let short = end > self.filled.load(Ordering::Acquire);
        let held = if short {
            lock_builtin(&self.filling)
        } else {
            match self.filling.try_lock() {
                Ok(held) => held,
                Err(TryLockError::Poisoned(held)) => PoisonError::into_inner(held),
                Err(TryLockError::WouldBlock) => return Ok(()),
            }
        };
        // Another appender may have refilled while this one waited.
        if self.ahead_of(end) {
            return Ok(());
        }
        let from = self.filled.load(Ordering::Acquire);
        let to = end.saturating_add(ahead);
        let zeros = vec![0; usize::try_from(to.saturating_sub(from)).unwrap_or(0)];
        self.file.write_all_at(&zeros, from)?;
        self.filled.store(to, Ordering::Release);
        drop(held);
        Ok(())
    }

    /// Whether the zeros reach half of `ahead` past `end`: far enough that
    /// no refill is due.
    fn ahead_of(&self, end: u64) -> bool {
        end.saturating_add(self.options.ahead.checked_div(REFILL_SHARE).unwrap_or(0)) <= self.filled.load(Ordering::Acquire)
    }

    /// Cuts the zeros past the blocks off a file appended to no more, as a
    /// segment of a log is once sealed, before it is synced: its size is
    /// then the end of its blocks, as a file grown only by appends has.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the file cannot be cut.
    pub(crate) fn trim(&self) -> Result<()> {
        let _held = lock_builtin(&self.filling);
        let end = self.end().0;
        if self.filled.load(Ordering::Acquire) <= end {
            return Ok(());
        }
        self.file.set_size(end)?;
        self.filled.store(end, Ordering::Release);
        Ok(())
    }

    /// Notes the file's bytes written through `end` and no further, once
    /// a tail past it is cut.
    pub(super) fn filled_to(&self, end: u64) {
        self.filled.store(end, Ordering::Release);
    }
}
