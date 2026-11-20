//! \[DESIGN\] Work grants (RULES.md D298): the bytes an operator holds in
//! memory before it spills, given it of the memory budget as it starts.
//! MEMORY.WORK_BYTES is the most an operator is granted, not what each
//! holds. One is granted half the budget left free, up to that ceiling,
//! reserved until it ends, so each grant after it takes half of what is
//! left and the grants of every statement running at once together never
//! pass the budget (RULES.md D38), however many sessions run them. Where
//! half the free budget is no more than `WORK_FLOOR`, an operator holds
//! that floor unreserved, as every operator did before grants; a ceiling
//! set below the floor is held as set, unreserved, so a session that asks
//! for small work areas gets them. A grant changes when an operator
//! spills, never what it gives.

use std::sync::{Arc, PoisonError, RwLock};

use super::{WORK_FLOOR, WorkAreas};
use crate::memory::Reservation;

/// \[DESIGN\] An operator's work grant: its bytes, and their reservation of
/// the memory budget, if they are more than the floor.
#[derive(Debug)]
pub struct Grant {
    bytes: usize,
    held: Option<Reservation>,
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
        Arc::new(grant_of(&self.memory, ceiling))
    }

    /// Its areas, for the operators of a part of a statement whose grant
    /// already holds their bytes: each granted its ceiling, none reserved
    /// again, so the parts of one grant never take the budget twice.
    pub(crate) fn covered(&self) -> WorkAreas {
        let place = self.place.read().unwrap_or_else(PoisonError::into_inner).clone();
        WorkAreas { memory: Arc::clone(&self.memory), place: RwLock::new(place), covered: true }
    }
}

/// A grant of `budget` of up to `ceiling` bytes.
fn grant_of(budget: &Arc<crate::memory::MemoryBudget>, ceiling: usize) -> Grant {
    if ceiling <= WORK_FLOOR {
        return Grant { bytes: ceiling, held: None };
    }
    let free = budget.limit().saturating_sub(budget.reserved());
    let wanted = ceiling.min(free >> 1_u32);
    if wanted <= WORK_FLOOR {
        return Grant { bytes: WORK_FLOOR, held: None };
    }
    match budget.reserve(wanted, "a work grant") {
        Ok(held) => Grant { bytes: wanted, held: Some(held) },
        // Taken by another between the look and the reservation.
        Err(_) => Grant { bytes: WORK_FLOOR, held: None },
    }
}

#[cfg(test)]
mod tests {
    use super::grant_of;
    use crate::memory::MemoryBudget;
    use crate::spill::{WORK_FLOOR, WorkAreas};

    /// Verifies: [DESIGN] RULES.md D298 each grant takes half the free
    /// budget up to its ceiling, reserved while it is held; past the floor
    /// none is reserved; a ceiling below the floor is held as set.
    #[test]
    fn grants_halve_the_free_budget() {
        let budget = MemoryBudget::new(64 * WORK_FLOOR);
        let first = grant_of(&budget, usize::MAX);
        assert_eq!((first.bytes, budget.reserved()), (32 * WORK_FLOOR, 32 * WORK_FLOOR));
        let second = grant_of(&budget, 8 * WORK_FLOOR);
        assert_eq!(second.bytes, 8 * WORK_FLOOR, "its ceiling");
        let third = grant_of(&budget, usize::MAX);
        assert_eq!(third.bytes, 12 * WORK_FLOOR);
        let small = grant_of(&budget, 1024_usize);
        assert_eq!((small.bytes, small.reserved()), (1024_usize, false));
        let rest: Vec<_> = (0_u8..8).map(|_| grant_of(&budget, usize::MAX)).collect();
        assert!(budget.reserved() <= budget.limit());
        assert_eq!(rest.last().map(|g| (g.bytes, g.reserved())), Some((WORK_FLOOR, false)), "the floor, unreserved");
        drop((first, second, third, rest));
        assert_eq!(budget.reserved(), 0, "given back");
    }

    /// Verifies: [DESIGN] RULES.md D298 the operators of a part of a
    /// statement are granted their ceiling of its grant, none of the budget
    /// reserved again.
    #[test]
    fn covered_grants_reserve_nothing() {
        let budget = MemoryBudget::new(64 * WORK_FLOOR);
        let areas = WorkAreas::new(std::sync::Arc::clone(&budget)).covered();
        let grant = areas.grant(16 * WORK_FLOOR);
        assert_eq!((grant.bytes, grant.reserved(), budget.reserved()), (16 * WORK_FLOOR, false, 0));
    }
}
