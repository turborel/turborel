//! The mutations of a grouping run in parallel under its HAVING
//! condition and of a grouping a join reads (RULES.md R-TEST-9, D1072,
//! D1073): the groups a part keeps, the conditions a part takes, and the
//! filters a grouping that takes none is told.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail the requirements of grouped parts.
pub const GROUPED_PARTS: [Mutation; 3] = [
    Mutation {
        name: "having-part-keeps-unknown",
        package: Package::Core,
        breaks: "a part keeps the groups its HAVING condition is unknown of, as those it is true of",
        file: "src/exec/parallel/grouped.rs",
        original: "Ok(truth) => truth == Truth::True,",
        replacement: "Ok(truth) => truth != Truth::False,",
        integration: &["grouped_parts::a_having_condition_is_judged_by_its_part"],
        unit: &[],
    },
    Mutation {
        name: "having-part-takes-raising",
        package: Package::Core,
        breaks: "a HAVING condition that can raise is judged by its grouping's part, in no order",
        file: "src/physical/parallel.rs",
        original: "return (calm_cond(cond) && !cond.can_raise()).then(|| rooted(input)).flatten();",
        replacement: "return calm_cond(cond).then(|| rooted(input)).flatten();",
        integration: &["grouped_parts::a_having_condition_that_can_raise_stays_above_its_part"],
        unit: &[],
    },
    Mutation {
        name: "having-part-not-one",
        package: Package::Core,
        breaks: "a filter over a grouping run in parallel is no part with it",
        file: "src/physical/parallel.rs",
        original: "&& matches!(**input, Plan::Aggregate { streamed: false, .. })\n    {",
        replacement: "&& matches!(**input, Plan::Distinct { streamed: false, .. })\n    {",
        integration: &["grouped_parts::a_having_condition_is_judged_by_its_part"],
        unit: &[],
    },
];
