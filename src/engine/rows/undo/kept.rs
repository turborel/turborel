//! \[DESIGN\] The bound on the batches of the undo store kept in memory once
//! written (RULES.md D31, D38, D746): which are kept, and their bytes. A
//! batch stays until the store is truncated past it, or until the batches
//! kept would pass their bound: then the oldest go first, as the newest are
//! what readers of open snapshots go back to most, and a version of one gone
//! is read from the log again (`undo.rs`).
//!
//! The bound is a share of the memory budget, `KEPT_SHARE`, reserved of it
//! as batches are kept and given back as they go; a reservation the budget
//! refuses makes the oldest go, as the bound does. A store with no budget
//! keeps none.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::error::Result;
use crate::memory::{MemoryBudget, Reservation};

/// \[DESIGN\] The share of the memory budget the batches kept may take:
/// one sixteenth, 64 MiB of the default budget, some seconds of the
/// versions a busy server appends, so readers of open snapshots find what
/// they go back to in memory.
const KEPT_SHARE: usize = 16;

/// What the reservation of the batches kept is listed as holding.
const KEPT_WHAT: &str = "the undo store's batches kept in memory";

/// The batches kept, by number, with their bytes.
pub(super) struct Kept {
    batches: BTreeMap<u64, usize>,
    bytes: usize,
    /// The budget, and what is reserved of it, when the store has one.
    budget: Option<(Arc<MemoryBudget>, Reservation)>,
}

impl Kept {
    /// No batch kept, as there is no budget to keep them in.
    pub(super) fn none() -> Kept {
        Kept { batches: BTreeMap::new(), bytes: 0, budget: None }
    }

    /// Batches kept up to `KEPT_SHARE` of `budget`, reserved of it.
    ///
    /// # Errors
    ///
    /// The budget's, if it refuses even an empty reservation.
    pub(super) fn in_budget(budget: &Arc<MemoryBudget>) -> Result<Kept> {
        let mut reservation = budget.reserve(0, KEPT_WHAT)?;
        // Held while the database is open, as a buffer pool is, and planned
        // at its bound, whatever the writes keep now (RULES.md D1159).
        reservation.stand_up_to(budget.limit().checked_div(KEPT_SHARE).unwrap_or(0));
        Ok(Kept { budget: Some((Arc::clone(budget), reservation)), ..Kept::none() })
    }

    /// The bytes the batches kept may hold now.
    fn bound(&self) -> usize {
        self.budget.as_ref().and_then(|(budget, _)| budget.limit().checked_div(KEPT_SHARE)).unwrap_or(0)
    }

    /// Keeps batch `number` of `size` bytes, the oldest kept going first
    /// past the bound or the budget: the numbers of those that went, and
    /// whether it is kept, which it is not if it alone passes either.
    pub(super) fn keep(&mut self, number: u64, size: usize) -> (Vec<u64>, bool) {
        let mut gone = Vec::new();
        let after = |kept: &Kept| kept.bytes.saturating_add(size);
        while after(self) > self.bound() || !self.reserved(after(self)) {
            let Some(oldest) = self.pop_oldest() else { return (gone, false) };
            gone.push(oldest);
        }
        self.bytes = after(self);
        if let Some(replaced) = self.batches.insert(number, size) {
            self.bytes = self.bytes.saturating_sub(replaced);
        }
        self.reserve_held();
        (gone, true)
    }

    /// Forgets the batches before `number`, which no reader needs.
    pub(super) fn forget_before(&mut self, number: u64) {
        let later = self.batches.split_off(&number);
        let gone = std::mem::replace(&mut self.batches, later);
        self.bytes = gone.values().fold(self.bytes, |bytes, size| bytes.saturating_sub(*size));
        self.reserve_held();
    }

    /// The bytes of the batches kept: for tests and statistics.
    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }

    /// The oldest batch kept, forgotten: its number.
    fn pop_oldest(&mut self) -> Option<u64> {
        let (number, size) = self.batches.pop_first()?;
        self.bytes = self.bytes.saturating_sub(size);
        self.reserve_held();
        Some(number)
    }

    /// Whether `bytes` are reserved of the budget, if there is one; with
    /// none, nothing need be.
    fn reserved(&mut self, bytes: usize) -> bool {
        self.budget.as_mut().is_none_or(|(_, reservation)| reservation.bytes() >= bytes || reservation.resize(bytes).is_ok())
    }

    /// Makes the reservation hold no more than what is kept: less only, so
    /// never refused.
    fn reserve_held(&mut self) {
        let bytes = self.bytes;
        if let Some((_, reservation)) = self.budget.as_mut()
            && reservation.bytes() > bytes
        {
            reservation.resize_anyway(bytes);
        }
    }
}
