//! The mutations of semi joins and antijoins (RULES.md R-TEST-9, D540,
//! D541): the rows a built left keeps, the right rows a hashed join
//! holds, the conflict rule that lets an inner join reassociate over one,
//! NOT IN's exact truth with nulls, and the set functions and joins the
//! reduction to semi joins takes.

use super::super::{Mutation, Package};

/// The test that compares every form made a semi join with its subquery.
const MATCHED: &str = "decorrelation::semi::semi_joins_match_the_subqueries";

/// Mechanisms whose removal must fail the semi join requirements.
pub const SEMI_JOINS: [Mutation; 7] = [
    Mutation {
        name: "semi-built-left-keeps-matched",
        package: Package::Core,
        breaks: "a right antijoin gives the left rows a right row matched, as a semi join does",
        file: "src/exec/join/semi.rs",
        original: ".filter(move |&(_, marked)| marked != anti)",
        replacement: ".filter(move |&(_, marked)| marked || anti)",
        integration: &[MATCHED],
        unit: &[],
    },
    Mutation {
        name: "semi-hash-first-of-key-only",
        package: Package::Core,
        breaks: "a hashed semi join holds only the first right row of each key though a condition beyond the keys decides",
        file: "src/exec/join/semi.rs",
        original: "if judge.cond.is_none() && first_held(bucket, &row, judge)? {",
        replacement: "if first_held(bucket, &row, judge)? {",
        integration: &[MATCHED],
        unit: &[],
    },
    Mutation {
        name: "semi-inner-reassociates",
        package: Package::Core,
        breaks: "an inner join may not reassociate over a semi join or an antijoin below it",
        file: "src/physical/enumerate/conflicts.rs",
        original: "        (Kind::Inner, Kind::Inner | Kind::Left | Kind::Semi | Kind::Anti) => true,",
        replacement: "        (Kind::Inner, Kind::Inner | Kind::Left | Kind::Anti) => true,\n        (Kind::Inner, Kind::Semi) => false,",
        integration: &[],
        unit: &["physical::enumerate::conflicts::tests::semi_joins_move_as_the_tables_say"],
    },
    Mutation {
        name: "semi-not-in-ignores-nulls",
        package: Package::Core,
        breaks: "NOT IN as an antijoin drops only the rows a value equals, keeping those a null may equal",
        file: "src/optimizer/unnesting/semi.rs",
        original: "let decides = Cond::Is(Box::new(holds), Truth::False, !all);",
        replacement: "let decides = Cond::Is(Box::new(holds), Truth::True, all);",
        integration: &["decorrelation::semi::not_in_with_nulls_as_an_antijoin", MATCHED],
        unit: &[],
    },
    Mutation {
        name: "reduce-count-blind",
        package: Package::Core,
        breaks: "COUNT and SUM are taken as blind to duplicates, their joins reduced to semi joins",
        file: "src/physical/blind.rs",
        original: "aggs.iter().all(|a| a.distribution.is_none() && extreme(a))",
        replacement: "aggs.iter().all(|a| a.distribution.is_none())",
        integration: &["join_order_workload::only_duplicate_blind_set_functions_are_reduced"],
        unit: &[],
    },
    Mutation {
        name: "reduce-read-relation-semi",
        package: Package::Core,
        breaks: "a hash join whose right operand a set function reads is made a semi join, its columns lost",
        file: "src/physical/blind.rs",
        original: "let semi = of_right.is_empty();",
        replacement: "let semi = true;",
        integration: &[
            "join_order_workload::only_duplicate_blind_set_functions_are_reduced",
            "join_order_workload::every_query_agrees_reduced_to_semi_joins",
        ],
        unit: &[],
    },
    Mutation {
        name: "reduce-read-table-semi",
        package: Package::Core,
        breaks: "an index join whose table a set function reads stops at each left row's first match, its columns lost",
        file: "src/physical/blind.rs",
        original: "let semi = !needed.iter().any(|&c| c >= lw) && ",
        replacement: "let semi = !needed.iter().any(|&c| c > lw + 1_000) && ",
        integration: &["join_order_workload::every_query_agrees_reduced_to_semi_joins"],
        unit: &[],
    },
];
