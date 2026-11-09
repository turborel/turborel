//! \[DESIGN\] A prepared query learning its plan (RULES.md D45): each of
//! its first executions plans it with its values, which it runs, and with
//! its slots, its paths chosen by its values (`physical/kept.rs`). Once `LEARNED_RUNS` in a row make the
//! two plans one shape (`physical/kept/shape.rs`), it keeps the one with
//! slots; one whose plans keep changing stops learning after
//! `MOST_LEARNING_RUNS`. A table the kept plan reads that gains or loses
//! rows as its statistics would be gathered again for has it learn again.

use std::sync::Arc;

use super::Physical;
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::engine::rows::{DEFAULT_REFRESH_FRACTION, DEFAULT_REFRESH_ROWS};
use crate::physical::{Plan, same_shape};
use crate::storage::cost_of;

/// \[DESIGN\] The executions in a row whose plans, made with their values
/// and with their slots, must have one shape before the one with slots is
/// kept: five, enough that values spread over a column's range show
/// whether they choose its paths, few enough that planning twice is soon
/// over.
pub(super) const LEARNED_RUNS: u8 = 5;

/// \[DESIGN\] The executions after which a query whose plans have not
/// kept one shape `LEARNED_RUNS` times in a row stops learning, so a
/// query whose values choose its paths is planned twice no longer.
pub(super) const MOST_LEARNING_RUNS: u8 = 20;

/// What a query learning its plan has learned (`Physical::Learning`).
#[derive(Debug, Clone, Default)]
pub(super) struct Learning {
    /// The plan the last execution made with its slots, if of the shape of
    /// the one made with its values.
    last: Option<Box<Plan>>,
    /// The executions in a row whose plans were so, each of the shape of
    /// the one before.
    agreed: u8,
    /// The executions that learned.
    runs: u8,
}

impl Learning {
    /// After an execution that planned `custom` with its values and
    /// `slotted` with its slots.
    pub(super) fn after(self, custom: &Plan, slotted: Plan) -> Learning {
        let shaped = same_shape(&slotted, custom);
        let continued = shaped && self.last.as_deref().is_some_and(|last| same_shape(last, &slotted));
        let agreed = if continued { self.agreed.saturating_add(1) } else { u8::from(shaped) };
        Learning { last: shaped.then(|| Box::new(slotted)), agreed, runs: self.runs.saturating_add(1) }
    }

    /// What is kept of the query after it: its plan with slots, once
    /// learned, reading `reads`, of `rows`; nothing, once learning fails.
    pub(super) fn kept(mut self, reads: &Arc<[QualifiedName]>, rows: Arc<[usize]>) -> Physical {
        if self.agreed >= LEARNED_RUNS
            && let Some(plan) = self.last.take()
        {
            return Physical::Fixed(plan, Arc::clone(reads), Some(rows));
        }
        if self.runs >= MOST_LEARNING_RUNS { Physical::Varies } else { Physical::Learning(self) }
    }
}

/// The rows of each table of `reads` in `st`.
pub(super) fn rows_of(st: &State, reads: &[QualifiedName]) -> Arc<[usize]> {
    reads.iter().map(|t| st.row_count(t).unwrap_or(0)).collect()
}

/// Whether a table of `reads` has gained or lost too many of the `rows`
/// it had in `st` for a plan learned then (`drifted`).
pub(super) fn moved(st: &State, reads: &[QualifiedName], rows: &[usize]) -> bool {
    reads.iter().zip(rows).any(|(t, &was)| drifted(was, st.row_count(t).unwrap_or(0)))
}

/// \[DESIGN\] Whether a table of `was` rows that now has `now` gained or
/// lost more than its statistics are gathered again for by default
/// (`STATISTICS.REFRESH_FRACTION` of them and `REFRESH_ROWS`), so a plan
/// learned follows what the statistics follow.
fn drifted(was: usize, now: usize) -> bool {
    let least = u32::try_from(DEFAULT_REFRESH_ROWS).map_or(f64::MAX, f64::from);
    cost_of(now.abs_diff(was)) > cost_of(was) * DEFAULT_REFRESH_FRACTION + least
}

// Unit tests: what a query learning its plan keeps after each execution
// (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;

    /// A scan of table `name`.
    fn scan(name: &str) -> Plan {
        let table = QualifiedName { auth: "A".into(), ident: name.into() };
        Plan::SeqScan { table, columns: vec![0], rows: 0, cost: 0.0_f64, order: Vec::new(), backward: false }
    }

    /// The learning after `agreed` executions in a row of a scan of `T`,
    /// of `runs` in all.
    fn learned(agreed: u8, runs: u8) -> Learning {
        Learning { last: (agreed > 0).then(|| Box::new(scan("T"))), agreed, runs }
    }

    /// Verifies: [DESIGN] RULES.md D45: an execution whose plans with its
    /// values and with its slots have one shape, as the last one's did,
    /// adds one to the executions in a row; one of another shape than the
    /// last's begins them again at one; one whose two plans differ, at
    /// none. MC/DC pairs of continuing: the plans of one shape or not, the
    /// last's shape the same or not, each alone.
    #[test]
    fn executions_agree_in_a_row_while_their_plans_keep_one_shape() {
        let after = |was: Learning, custom: &str, slotted: &str| was.after(&scan(custom), scan(slotted));
        let both = after(learned(2, 2), "T", "T");
        assert_eq!((both.agreed, both.runs, both.last.is_some()), (3, 3, true));
        let unshaped = after(learned(2, 2), "U", "T");
        assert_eq!((unshaped.agreed, unshaped.runs, unshaped.last.is_some()), (0, 3, false), "two plans of two shapes");
        let other = after(learned(2, 2), "U", "U");
        assert_eq!((other.agreed, other.runs), (1, 3), "of another shape than the last's");
        assert_eq!(after(Learning::default(), "T", "T").agreed, 1, "the first");
    }

    /// Verifies: [DESIGN] RULES.md D45: a query keeps its plan with slots
    /// once `LEARNED_RUNS` executions in a row agree; stops learning once
    /// `MOST_LEARNING_RUNS` executions have not; else learns on. MC/DC
    /// pairs of keeping: enough in a row or not, with a plan or not.
    #[test]
    fn a_plan_is_kept_once_learned_and_learning_stops_after_enough_runs() {
        let kept = |l: Learning| format!("{:?}", l.kept(&Arc::from(Vec::new()), Arc::from(Vec::new())));
        assert!(kept(learned(LEARNED_RUNS, LEARNED_RUNS)).starts_with("Fixed"));
        assert!(kept(learned(LEARNED_RUNS - 1, LEARNED_RUNS)).starts_with("Learning"), "one too few in a row");
        let planless = Learning { last: None, agreed: LEARNED_RUNS, runs: LEARNED_RUNS };
        assert!(kept(planless).starts_with("Learning"), "no plan kept from the last");
        assert!(kept(learned(1, MOST_LEARNING_RUNS)).starts_with("Varies"));
        assert!(kept(learned(1, MOST_LEARNING_RUNS - 1)).starts_with("Learning"));
    }

    /// Verifies: [DESIGN] RULES.md D45: a table has drifted from a plan
    /// learned once it gains or loses more than a tenth of its rows and
    /// 50 more, as its statistics are gathered again by default; at the
    /// bound, not.
    #[test]
    fn a_table_drifts_past_its_statistics_refresh() {
        assert!(!drifted(1000, 1150) && drifted(1000, 1151));
        assert!(!drifted(1000, 850) && drifted(1000, 849));
        assert!(!drifted(0, 50) && drifted(0, 51));
    }
}
