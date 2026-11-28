//! \[DESIGN\] (RULES.md D1266) How far a scan judged each conjunct of a
//! run, kept as the run is judged (`Judgment::judge`): the conjuncts
//! judged exactly, each by its bit, and the one judged of the run's first
//! rows alone. The filter over the scan passes over the first and judges
//! the second of the rows left (`exec/batch/judge/vetted.rs`).

use super::Pred;
use crate::storage::{Conjunct, Vetted};

/// \[DESIGN\] (RULES.md D1266) How far a conjunct was judged of the rows
/// asked of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reach {
    /// The conjunct itself was evaluated of every row asked.
    Every,
    /// It was evaluated of every row asked before the one at this place
    /// from the run's first, and the rows from there on were kept
    /// unjudged.
    Before(u32),
    /// Some row was kept unjudged among the others, or kept or left by a
    /// summary's word.
    Part,
}

/// \[DESIGN\] (RULES.md D1266) The conjuncts judged exactly of a run so
/// far, and the one judged of its first rows alone, if one was.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Reached {
    pub(super) exact: u64,
    pub(super) partly: Option<(u64, u32)>,
}

impl Reached {
    /// Notes that the conjunct of `bit` was judged as far as `reach`.
    /// One conjunct judged of the first rows alone is noted, the first:
    /// another is judged whole by the filter.
    pub(super) fn note(&mut self, bit: u64, reach: Reach) {
        match reach {
            Reach::Every => self.exact |= bit,
            Reach::Before(place) => self.partly = self.partly.or(Some((bit, place))),
            Reach::Part => {}
        }
    }
}

impl Pred {
    /// \[DESIGN\] (RULES.md D1266) Its bit among the conjuncts judged
    /// exactly (`Vetted::bit`), by its place in the condition: none of a
    /// sort's bound, which is judged as the test it is now and may be a
    /// tighter one by the time the filter reads the rows, so no judging
    /// of it is the conjunct's own; and none of a join's keys (RULES.md
    /// D1294), which are no conjunct of the filter, are asked of a run
    /// or not as they keep rows, and are judged by the join whatever the
    /// scan left out.
    pub(super) fn bit(&self) -> u64 {
        if matches!(self.conjunct, Conjunct::Dynamic { .. } | Conjunct::Among { .. }) { 0 } else { Vetted::bit(self.at) }
    }
}
