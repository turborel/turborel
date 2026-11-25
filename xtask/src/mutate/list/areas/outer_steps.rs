//! The mutations of a LEFT outer join's probe as a step of a pipeline
//! (RULES.md R-TEST-9, D1076): the nulls of a row none matches, by batch
//! and row by row, and the joins that are steps.

use super::super::{Mutation, Package};

/// The test of a LEFT join as a step of a pipeline.
const STEP: &str = "outer_steps::a_left_join_is_a_step_of_its_lefts_pipeline";

/// Mechanisms whose removal must fail the requirements of outer steps.
pub const OUTER_STEPS: [Mutation; 3] = [
    Mutation {
        name: "outer-step-unmatched-paired",
        package: Package::Core,
        breaks: "a left row no built row matches is joined with a built row, not with nulls",
        file: "src/exec/batch/joined/shared/outer.rs",
        original: "pairs.1.push(NO_ROW);",
        replacement: "pairs.1.push(0);",
        integration: &[STEP],
        unit: &[],
    },
    Mutation {
        name: "outer-step-row-drops-unmatched",
        package: Package::Core,
        breaks: "a pipeline read row by row drops a left row no built row matches",
        file: "src/exec/parallel/pipeline.rs",
        original: "if any { Ok(()) } else { feed(rest, unmatched(table, row), runner, out) }",
        replacement: "if any || row.is_empty() { Ok(()) } else { Ok(()) }",
        integration: &[STEP],
        unit: &[],
    },
    Mutation {
        name: "outer-step-with-condition",
        package: Package::Core,
        breaks: "a LEFT join with a condition beyond its keys is a step, the condition never judged",
        file: "src/physical/parallel.rs",
        original: "if let Plan::OuterJoin { kind: OuterJoin::Left, left, keys, cond: None, own: None, .. } = plan {",
        replacement: "if let Plan::OuterJoin { kind: OuterJoin::Left, left, keys, own: None, .. } = plan {",
        integration: &["outer_steps::a_left_join_on_more_than_its_keys_is_no_step"],
        unit: &[],
    },
];
