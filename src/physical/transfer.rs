//! \[DESIGN\] (RULES.md D665, D666) Predicate transfer: before the joins of
//! a chain of inner hash joins and LEFT joins, its leaves read in turn, each filtered by
//! Bloom filters of the join keys of the leaves read before it, as Yang,
//! Zhao, Yu and Koutris (CIDR 2024) transfer predicates, along a tree of
//! its leaves rooted at the largest, from the leaves to the root and back,
//! as Zhao et al., Robust Predicate Transfer (SIGMOD 2025), do with
//! LargestRoot. A hash join's runtime filter (RULES.md D369) goes from its
//! build to its probe side alone; this goes back too, so a selective
//! condition on a probe side's table shrinks the builds before they are
//! held, and so does one two joins away.
//!
//! The planner plans the phase after the rest of the plan (`transferred`):
//! of each chain (`transfer/chain.rs`) whose operators and leaves cannot
//! raise an error and whose batches every leaf gives, it builds the tree
//! (`transfer/tree.rs`), keeps a step of it only where the leaf filtered
//! is expected to lose rows by it (`transfer/payoff.rs`, as RPT+, Qiao,
//! Boncz and Zhang, PVLDB 2026, builds a filter only where it pays), and
//! plans the phase only if the rows its steps are expected to drop are
//! worth more than its filters cost. The executor runs it
//! (`exec/batch/transfer.rs`).
//!
//! Its answers are the join's (RULES.md D15). A step drops a row of a
//! leaf only if no row of the other matches its key, and so no row of the
//! chain holds it; the joins then run in their planned order over the
//! rows kept, each in its order, so the chain gives the same rows in the
//! same order. No operator of the chain or of its leaves can raise an
//! error or a warning, so none raises on fewer rows than it would, and
//! no leaf is a guard's operand (a guard's part can raise). A chain under
//! a limit not of a sort, or run in parallel, is planned as it was: the
//! phase reads every leaf whole.

mod chain;
mod payoff;
mod tree;

use super::Plan;
use crate::db::State;
use crate::settings::PREDICATE_TRANSFER;
pub(crate) use chain::{leaves_mut, link};

/// \[DESIGN\] The deepest a plan is walked for chains, as a plan is no
/// deeper than its planner made it (RULES.md R-CF-6).
const MAX_DEPTH: usize = 1024;

/// \[DESIGN\] (RULES.md D665) A plan of predicate transfer, `Plan::Transfer`.
#[derive(Debug, Clone)]
pub enum Transfer {
    /// The phase over a chain: its leaves read in the order of the steps,
    /// then the chain run over the rows they kept.
    Phase(Box<Phase>),
    /// A leaf's rows the phase kept, read in the leaf's place.
    Held(crate::exec::HeldLeaf),
}

/// \[DESIGN\] (RULES.md D665) The phase over a chain.
#[derive(Debug, Clone)]
pub struct Phase {
    /// The chain: inner hash joins, and filters and projections over
    /// them, down to its leaves (`transfer/chain.rs`).
    pub input: Plan,
    /// The steps, in order: those from the leaves to the root, then back.
    pub steps: Vec<Step>,
    /// What EXPLAIN shows of it.
    pub detail: String,
}

/// \[DESIGN\] A step of a phase: a Bloom filter of the keys of a column of
/// one leaf's rows kept, judging a column of another's. Leaves are
/// numbered in the order `transfer/chain.rs` walks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// The leaf the filter is made of, and the column of its rows.
    pub from: (usize, usize),
    /// The leaf it judges, and the column of its rows.
    pub to: (usize, usize),
    /// Whether a null of the key matches a null.
    pub nulls: bool,
}

impl Transfer {
    /// The plan read in its place: a phase's chain; none of a held leaf.
    pub fn input(&self) -> Option<&Plan> {
        match self {
            Transfer::Phase(p) => Some(&p.input),
            Transfer::Held(_) => None,
        }
    }

    /// As `input`, to change.
    pub(crate) fn input_mut(&mut self) -> Option<&mut Plan> {
        match self {
            Transfer::Phase(p) => Some(&mut p.input),
            Transfer::Held(_) => None,
        }
    }

    /// The line EXPLAIN shows of it.
    pub(crate) fn line(&self) -> String {
        match self {
            Transfer::Phase(p) => format!("PredicateTransfer {}", p.detail),
            Transfer::Held(_) => "TransferredRows".to_owned(),
        }
    }
}

/// `plan` with a phase of predicate transfer over each chain worth one,
/// if PLANNER.PREDICATE_TRANSFER is ON.
pub(crate) fn transferred(db: &State, mut plan: Plan) -> Plan {
    if db.settings.is_on(PREDICATE_TRANSFER) {
        walk(db, &mut plan, 0);
    }
    plan
}

/// Plans a phase over each chain of `plan`, `depth` below the top.
fn walk(db: &State, plan: &mut Plan, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    let deeper = depth.saturating_add(1);
    if link(plan) {
        let steps = payoff::planned(db, plan);
        leaves_mut(plan, &mut |_, leaf| walk(db, leaf, deeper));
        if let Some((steps, detail)) = steps {
            let input = std::mem::replace(plan, Plan::Values { rows: Vec::new(), detail: String::new() });
            *plan = Plan::Transfer(Box::new(Transfer::Phase(Box::new(Phase { input, steps, detail }))));
        }
        return;
    }
    // A limit of a stream reads only its first rows; a part run in
    // parallel is a pipeline of morsels the phase is no step of.
    let whole =
        if let Plan::Limit { input, .. } = plan { matches!(input.bare(), Plan::Sort { .. }) } else { !matches!(plan, Plan::Parallel(_)) };
    if whole {
        plan.each_input(&mut |input| walk(db, input, deeper));
    }
}
