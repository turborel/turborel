//! \[DESIGN\] Work grants (RULES.md D298): the bytes an operator holds in
//! memory before it spills, given it of the memory budget as it starts.
//! MEMORY.WORK_BYTES is the most an operator is granted, not what each
//! holds. Grants share half of what the standing reservations, the
//! buffer pools, leave of the budget (D683), the other half kept for what
//! reserves beside them, spill areas' pools first, so an operator that
//! spills finds room however many grants are held. One is granted half of
//! the grants' share left free, up to that ceiling, reserved until it ends,
//! so each grant after it takes half of what is left and the grants of
//! every statement running at once together never pass their share (RULES.md
//! D38), however many sessions run them. Where half of the share left free
//! is no more than `WORK_FLOOR`, an operator holds
//! that floor unreserved, as every operator did before grants; a ceiling
//! set below the floor is held as set, unreserved, so a session that asks
//! for small work areas gets them. A grant changes when an operator
//! spills, never what it gives.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use super::{WORK_FLOOR, WorkAreas};
use crate::memory::Reservation;

/// \[DESIGN\] An operator's work grant: its bytes, and their reservation of
/// the memory budget, if they are more than the floor.
#[derive(Debug)]
pub struct Grant {
    bytes: usize,
    /// Its reservation, and the bytes of its areas' grants it counts in.
    held: Option<(Reservation, Arc<AtomicUsize>)>,
}

impl Drop for Grant {
    fn drop(&mut self) {
        if let Some((_, granted)) = &self.held {
            granted.fetch_sub(self.bytes, Ordering::AcqRel);
        }
    }
}

impl Grant {
    /// Its bytes.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Whether its bytes are reserved of the budget.
    pub fn reserved(&self) -> bool {
        self.held.is_some()
    }
}

impl WorkAreas {
    /// A grant of up to `ceiling` bytes of the budget its areas are reserved
    /// of: half of what is free, at most `ceiling`, at least the floor, or
    /// `ceiling` itself below the floor.
    pub fn grant(&self, ceiling: usize) -> Arc<Grant> {
        if self.covered {
            return Arc::new(Grant { bytes: ceiling, held: None });
        }
        Arc::new(grant_of((&self.memory, &self.granted), ceiling))
    }

    /// The bytes a grant of up to `ceiling` would be now, reserving
    /// nothing: for the planner, which costs what an operator spills past
    /// its grant (RULES.md D651, D683).
    pub(crate) fn would_grant(&self, ceiling: usize) -> usize {
        if self.covered || ceiling <= WORK_FLOOR {
            return ceiling;
        }
        wanted_of(&self.memory, self.granted.load(Ordering::Acquire), ceiling).max(WORK_FLOOR)
    }

    /// Its areas, for the operators of a part of a statement whose grant
    /// already holds their bytes: each granted its ceiling, none reserved
    /// again, so the parts of one grant never take the budget twice.
    pub(crate) fn covered(&self) -> WorkAreas {
        let place = self.place.read().unwrap_or_else(PoisonError::into_inner).clone();
        let granted = Arc::clone(&self.granted);
        WorkAreas { memory: Arc::clone(&self.memory), place: RwLock::new(place), covered: true, granted }
    }
}

/// A grant of `budget` of up to `ceiling` bytes, counted in `granted`, the
/// bytes its areas' grants hold.
fn grant_of((budget, granted): (&Arc<crate::memory::MemoryBudget>, &Arc<AtomicUsize>), ceiling: usize) -> Grant {
    if ceiling <= WORK_FLOOR {
        return Grant { bytes: ceiling, held: None };
    }
    let wanted = wanted_of(budget, granted.load(Ordering::Acquire), ceiling);
    if wanted <= WORK_FLOOR {
        return Grant { bytes: WORK_FLOOR, held: None };
    }
    match budget.reserve(wanted, "a work grant") {
        Ok(reserved) => {
            granted.fetch_add(wanted, Ordering::AcqRel);
            Grant { bytes: wanted, held: Some((reserved, Arc::clone(granted))) }
        }
        // Taken by another between the look and the reservation.
        Err(_) => Grant { bytes: WORK_FLOOR, held: None },
    }
}

/// The bytes a grant of `budget` of up to `ceiling` takes, its areas'
/// grants holding `held`: half of their share left, the share half of
/// what the standing reservations leave of the budget, the other half
/// kept for what reserves beside grants, spill areas first.
fn wanted_of(budget: &crate::memory::MemoryBudget, held: usize, ceiling: usize) -> usize {
    let share = budget.limit().saturating_sub(budget.standing()) >> 1_u32;
    ceiling.min(share.saturating_sub(held) >> 1_u32)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use super::{Grant, grant_of};
    use crate::memory::MemoryBudget;
    use crate::spill::{WORK_FLOOR, WorkAreas};

    /// Verifies: [DESIGN] RULES.md D298 each grant takes half of the
    /// grants' share left free, half the budget, up to its ceiling,
    /// reserved while it is held; past the floor none is reserved; a
    /// ceiling below the floor is held as set; and however many are held,
    /// half the budget stays free for spill areas. Regression test
    /// (R-TEST-7): grants took half of all the budget left free, so a
    /// query of hundreds of operators held at once (an XMLMAP of the
    /// catalog) left no room for a spill area's pool.
    #[test]
    fn grants_halve_the_free_budget() {
        let budget = MemoryBudget::new(64 * WORK_FLOOR);
        let granted = Arc::default();
        let grant_of = |ceiling| grant_of((&budget, &granted), ceiling);
        let first = grant_of(usize::MAX);
        assert_eq!((first.bytes, budget.reserved()), (16 * WORK_FLOOR, 16 * WORK_FLOOR));
        let second = grant_of(4 * WORK_FLOOR);
        assert_eq!(second.bytes, 4 * WORK_FLOOR, "its ceiling");
        let third = grant_of(usize::MAX);
        assert_eq!(third.bytes, 6 * WORK_FLOOR);
        let small = grant_of(1024_usize);
        assert_eq!((small.bytes, small.reserved()), (1024_usize, false));
        let rest: Vec<_> = (0_u16..300).map(|_| grant_of(usize::MAX)).collect();
        assert!(budget.reserved() <= budget.limit() >> 1_u32, "half kept");
        assert!(budget.reserve(WORK_FLOOR, "a spill area").is_ok(), "a spill area finds room");
        assert_eq!(rest.last().map(|g| (g.bytes, g.reserved())), Some((WORK_FLOOR, false)), "the floor, unreserved");
        drop((first, second, third, rest));
        assert_eq!(budget.reserved(), 0, "given back");
    }

    /// Verifies: [DESIGN] RULES.md D683 grants share half of what a buffer
    /// pool, a standing reservation, left of the budget; a spill area
    /// takes none of their share; and a grant is never reserved past what
    /// is free. Regression test
    /// (R-TEST-7): grants kept half the whole budget free, so a pool of
    /// more than half of it left every operator the floor, and a hash
    /// join of a few megabytes spilled.
    #[test]
    fn grants_share_what_a_pool_leaves() {
        let budget = MemoryBudget::new(48 * WORK_FLOOR);
        let mut pool = budget.reserve(32 * WORK_FLOOR, "a buffer pool").unwrap();
        pool.stand();
        let granted = Arc::new(AtomicUsize::new(0));
        let first: Grant = grant_of((&budget, &granted), usize::MAX);
        assert_eq!((first.bytes, first.reserved()), (4 * WORK_FLOOR, true), "half of the grants' half of 16");
        let spill = budget.reserve(8 * WORK_FLOOR, "a spill area").unwrap();
        let second = grant_of((&budget, &granted), usize::MAX);
        assert_eq!(second.bytes, 2 * WORK_FLOOR, "half of what is left of the share, a spill area taking none of it");
        let third = grant_of((&budget, &granted), 2 * WORK_FLOOR);
        assert_eq!((third.bytes, third.reserved()), (WORK_FLOOR, false), "the floor, unreserved, once the share is spent");
        let held = granted.load(std::sync::atomic::Ordering::Acquire);
        assert!(held <= (budget.limit() - budget.standing()) >> 1_u32, "the grants never pass their share");
        drop(second);
        let rest = budget.reserve(budget.limit() - budget.reserved(), "a spill area").unwrap();
        let full = grant_of((&budget, &granted), usize::MAX);
        assert_eq!((full.bytes, full.reserved(), budget.reserved()), (WORK_FLOOR, false, budget.limit()), "never past what is free");
        drop((first, third, full, spill, rest));
        assert_eq!(granted.load(std::sync::atomic::Ordering::Acquire), 0, "given back");
        drop(pool);
        assert_eq!((budget.standing(), budget.reserved()), (0, 0));
    }

    /// Verifies: [DESIGN] RULES.md D298 the operators of a part of a
    /// statement are granted their ceiling of its grant, none of the budget
    /// reserved again.
    #[test]
    fn covered_grants_reserve_nothing() {
        let budget = MemoryBudget::new(64 * WORK_FLOOR);
        let areas = WorkAreas::new(Arc::clone(&budget)).covered();
        let grant = areas.grant(16 * WORK_FLOOR);
        assert_eq!((grant.bytes, grant.reserved(), budget.reserved()), (16 * WORK_FLOOR, false, 0));
    }
}
