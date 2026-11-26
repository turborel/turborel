//! \[DESIGN\] A prepared query learning its plan (RULES.md D45): each of
//! its first executions plans it with its values, which it runs, and with
//! its slots, its paths chosen by its values (`physical/kept.rs`). Once `LEARNED_RUNS` in a row make the
//! two plans one shape (`physical/kept/shape.rs`), it keeps the one with
//! slots; one whose plans keep changing stops learning after
//! `MOST_LEARNING_RUNS`. A table the kept plan reads that gains or loses
//! rows as its statistics would be gathered again for has it learn again,
//! as `grown.rs` says; so does one a query that stopped learning reads.

use std::sync::Arc;

use super::Physical;
use super::grown::Learnt;
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::physical::{Plan, Seen, prepared_plan, same_shape};

/// \[DESIGN\] The executions in a row whose plans, made with their values
/// and with their slots, must have one shape before the one with slots is
/// kept: five, enough that values spread over a column's range show
/// whether they choose its paths, few enough that planning twice is soon
/// over.
pub(super) const LEARNED_RUNS: u8 = 5;

/// \[DESIGN\] The executions after which a query whose plans have not
/// kept one shape `LEARNED_RUNS` times in a row stops learning, so a
/// query whose values choose its paths is planned twice no longer.
pub(crate) const MOST_LEARNING_RUNS: u8 = 20;

/// What a query learning its plan has learned (`Physical::Learning`).
#[derive(Debug, Clone, Default)]
pub(crate) struct Learning {
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
    pub(crate) fn after(self, custom: &Plan, slotted: Plan) -> Learning {
        let shaped = same_shape(&slotted, custom);
        let continued = shaped && self.last.as_deref().is_some_and(|last| same_shape(last, &slotted));
        let agreed = if continued { self.agreed.saturating_add(1) } else { u8::from(shaped) };
        Learning { last: shaped.then(|| Box::new(slotted)), agreed, runs: self.runs.saturating_add(1) }
    }

    /// What is kept of the query after it: its plan with slots, once
    /// learned, reading `reads`, measured from them as they are in `st`
    /// (`grown.rs`), its scalar calls prepared (RULES.md D610); nothing,
    /// once learning fails, until a table it reads drifts, or of a plan
    /// that cannot be kept. Its tables are measured as the execution that
    /// planned it saw them, `seen` (RULES.md D1089).
    pub(super) fn kept(self, reads: &Arc<[QualifiedName]>, (st, seen): (&State, &Seen)) -> Physical {
        match self.outcome() {
            Learned::Kept(plan) => {
                let learnt = Learnt::of((st, seen), reads, &plan);
                match prepared_plan(*plan, crate::optimizer::prepared::budget(st)) {
                    Ok(kept) => Physical::Fixed(Arc::new(kept), Arc::clone(reads), Some(learnt)),
                    Err(_) => Physical::Varies(None),
                }
            }
            Learned::Learning(learning) => Physical::Learning(learning),
            Learned::Stopped => Physical::Varies(Some(Learnt::rows((st, seen), reads))),
        }
    }

    /// What it has learned: its plan with slots, once learned; that it
    /// stopped, once learning fails; else itself, learning on.
    pub(crate) fn outcome(mut self) -> Learned {
        if self.agreed >= LEARNED_RUNS
            && let Some(plan) = self.last.take()
        {
            return Learned::Kept(plan);
        }
        if self.runs >= MOST_LEARNING_RUNS { Learned::Stopped } else { Learned::Learning(self) }
    }
}

/// What a plan learning has learned after an execution (`Learning::outcome`).
#[derive(Debug)]
pub(crate) enum Learned {
    /// Its plan with slots, kept.
    Kept(Box<Plan>),
    /// Not yet learned, so it learns on.
    Learning(Learning),
    /// Its plans keep changing, so it is planned at each execution.
    Stopped,
}

// Unit tests: what a query learning its plan keeps after each execution
// (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;

    /// A scan of table `name`.
    fn scan(name: &str) -> Plan {
        let table = QualifiedName { auth: "A".into(), ident: name.into() };
        Plan::SeqScan { table, columns: vec![0], rows: 0, cost: 0.0_f64, order: Vec::new(), backward: false, most: None, located: false }
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
        let st = State::with_builtins(Box::new(crate::catalog::MemoryCatalog::default()));
        let kept = |l: Learning| format!("{:?}", l.kept(&Arc::from(Vec::new()), (&st, &Seen::default())));
        assert!(kept(learned(LEARNED_RUNS, LEARNED_RUNS)).starts_with("Fixed"));
        assert!(kept(learned(LEARNED_RUNS - 1, LEARNED_RUNS)).starts_with("Learning"), "one too few in a row");
        let planless = Learning { last: None, agreed: LEARNED_RUNS, runs: LEARNED_RUNS };
        assert!(kept(planless).starts_with("Learning"), "no plan kept from the last");
        assert!(kept(learned(1, MOST_LEARNING_RUNS)).starts_with("Varies"));
        assert!(kept(learned(1, MOST_LEARNING_RUNS - 1)).starts_with("Learning"));
    }
}
