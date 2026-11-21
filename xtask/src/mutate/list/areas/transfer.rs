//! The mutations of predicate transfer (RULES.md R-TEST-9, D665-D668): a
//! chain that can raise given a phase, a chain under a limit of no sort
//! given one, a LEFT join's key transferred back to its preserved operand,
//! a step kept of more keys than half the values it judges, and a filter
//! told the wrong column of the leaf it judges.

use super::super::{Mutation, Package};

/// The plan's file.
const PAYOFF: &str = "src/physical/transfer/payoff.rs";
/// The planned phase's integration test.
const PLANNED: &str = "predicate_transfer::a_phase_is_planned_where_it_pays";

/// Mechanisms of predicate transfer whose removal must fail its tests.
pub const TRANSFER: [Mutation; 5] = [
    Mutation {
        name: "transfer-raising-chain",
        package: Package::Core,
        breaks: "a chain that can raise an error is given no phase (D15)",
        file: PAYOFF,
        original: "    if plan.can_raise() || !crate::exec::transferable(plan, db) {",
        replacement: "    if !crate::exec::transferable(plan, db) {",
        integration: &[PLANNED],
        unit: &[],
    },
    Mutation {
        name: "transfer-under-limit",
        package: Package::Core,
        breaks: "a chain under a limit of no sort, which reads its first rows alone, is given no phase",
        file: "src/physical/transfer.rs",
        original: "if let Plan::Limit { input, .. } = plan { matches!(input.bare(), Plan::Sort { .. }) }",
        replacement: "if let Plan::Limit { .. } = plan { true }",
        integration: &[PLANNED],
        unit: &[],
    },
    Mutation {
        name: "transfer-left-join-back",
        package: Package::Core,
        breaks: "a LEFT join's key transfers from its left operand to its right alone",
        file: PAYOFF,
        original: "(k.both && k.step == Step { from: step.to, to: step.from, nulls: step.nulls })",
        replacement: "(k.step == Step { from: step.to, to: step.from, nulls: step.nulls })",
        integration: &[],
        unit: &["physical::transfer::payoff::tests::a_left_joins_key_transfers_to_its_right_alone"],
    },
    Mutation {
        name: "transfer-share-ignored",
        package: Package::Core,
        breaks: "a step is kept only of keys at most half the values it judges",
        file: PAYOFF,
        original: "    keys <= TRANSFER_SHARE * values",
        replacement: "    keys <= values",
        integration: &[],
        unit: &["physical::transfer::payoff::tests::a_step_keeps_half_the_values_at_most"],
    },
    Mutation {
        name: "transfer-filter-wrong-column",
        package: Package::Core,
        breaks: "a step's filter judges the column of the leaf its key is of",
        file: "src/exec/batch/transfer.rs",
        original: "Told { filter, column: step.to.1 }",
        replacement: "Told { filter, column: step.from.1 }",
        integration: &["predicate_transfer::transfer_changes_no_answer"],
        unit: &[],
    },
];
