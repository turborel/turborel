//! The mutations of join enumeration (RULES.md R-TEST-9, D300-D309): a
//! conflict rule dropped, an implied equality counted again, the floor
//! of one row lost, a csg-cmp pair enumerated twice, the budget ignored,
//! the gain ignored, a merge join offered with both operands sorted, its
//! runs read again for each left row, its unmatched LEFT rows lost, a
//! conjunct over an outer join's nulls placed below it, an OR's repeated
//! equality left in it, the OR implied of a table not added, and an
//! index prefix counted past the limit guessed below the count.

use super::super::{Mutation, Package};

/// The conflict detector.
const CONFLICTS_FILE: &str = "src/physical/enumerate/conflicts.rs";
/// The estimates.
const MODEL_FILE: &str = "src/physical/enumerate/model.rs";
/// The driver and the adaptive switch.
const DRIVER_FILE: &str = "src/physical/enumerate.rs";
/// The merge join's executor.
const MERGE_FILE: &str = "src/exec/join/merge.rs";
/// The rule factoring disjunctions.
const DISJUNCTIONS_FILE: &str = "src/optimizer/disjunctions.rs";
/// The integration test of an OR whose operands repeat a join's equality.
const SHAPED_OR: &str = "benchmark_shapes::an_equality_each_disjunct_repeats_joins";
/// The merge join's integration test.
const MERGED: &str = "join_enumeration::merge_joins_pair_runs_of_equal_keys";

/// Mechanisms of join enumeration whose removal must fail its tests.
pub const JOINS: [Mutation; 13] = [
    Mutation {
        name: "joins-conflict-rule",
        package: Package::Core,
        breaks: "a LEFT join below an inner one loses its conflict rule, so its null-supplying side is joined first",
        file: CONFLICTS_FILE,
        original: "if !assoc(a, upper) {",
        replacement: "if assoc(a, upper) && a.strong && !a.strong {",
        integration: &[],
        unit: &["physical::enumerate::conflicts::tests::a_left_join_below_an_inner_one_keeps_its_preserved_side"],
    },
    Mutation {
        name: "joins-class-counted-again",
        package: Package::Core,
        breaks: "an equality other equalities imply is counted again in a join's rows",
        file: MODEL_FILE,
        original: "distinct.iter().skip(1)",
        replacement: "distinct.iter().skip(0)",
        integration: &[],
        unit: &["physical::enumerate::tests::a_class_of_equalities_is_counted_once"],
    },
    Mutation {
        name: "joins-one-row-floor",
        package: Package::Core,
        breaks: "a join is estimated to give less than one row",
        file: MODEL_FILE,
        original: "rows.max(product.min(1.0_f64))",
        replacement: "rows.max(0.0_f64)",
        integration: &[],
        unit: &["physical::enumerate::tests::a_class_of_equalities_is_counted_once"],
    },
    Mutation {
        name: "joins-pair-twice",
        package: Package::Core,
        breaks: "EmitCsg leaves the relations before its set's least out of the excluded set, so pairs are enumerated twice",
        file: "src/physical/enumerate/dphyp.rs",
        original: "let x = s1 | lowest(s1).map_or(0, through) | !u;",
        replacement: "let x = s1 | !u;",
        integration: &[],
        unit: &["physical::enumerate::tests::dpccp_enumerates_each_pair_once"],
    },
    Mutation {
        name: "joins-budget-ignored",
        package: Package::Core,
        breaks: "a graph of 14 relations or more is never searched exactly, however few its subgraphs",
        file: DRIVER_FILE,
        original: "if n < ALWAYS_EXACT || space.count(all, budget) <= budget {",
        replacement: "if n < ALWAYS_EXACT {",
        integration: &[],
        unit: &["physical::enumerate::tests::the_switch_adapts_to_the_budget"],
    },
    Mutation {
        name: "joins-gain-ignored",
        package: Package::Core,
        breaks: "no enumerated plan is ever taken over the FROM clause's",
        file: DRIVER_FILE,
        original: "if best.cost >= self.db.cost().reorder_gain * written {",
        replacement: "if best.cost >= 0.0_f64 {",
        integration: &["join_enumeration::explain_tells_the_order_and_the_algorithm"],
        unit: &[],
    },
    Mutation {
        name: "joins-merge-both-sorted",
        package: Package::Core,
        breaks: "a merge join is offered where both its operands would be sorted",
        file: "src/physical/enumerate/methods.rs",
        original: "if !(left.2 || right.2) {",
        replacement: "if left.2 && !left.2 {",
        integration: &["probes::a_raising_conjunct_of_one_of_three_operands"],
        unit: &[],
    },
    Mutation {
        name: "joins-merge-run-again",
        package: Package::Core,
        breaks: "the merge join reads past a key's run for each left row of the key, so the second finds none",
        file: MERGE_FILE,
        original: "Some((key, _)) => key_equal(key, value, false)?,",
        replacement: "Some((key, _)) => key_equal(key, value, false)? && key.is_null(),",
        integration: &[MERGED],
        unit: &[],
    },
    Mutation {
        name: "joins-merge-left-lost",
        package: Package::Core,
        breaks: "a LEFT merge join drops the left rows no right row matches",
        file: MERGE_FILE,
        original: "if let (false, Some(nulls)) = (matched, &self.nulls) {",
        replacement: "if let (true, Some(nulls)) = (matched, &self.nulls) {",
        integration: &[MERGED],
        unit: &[],
    },
    Mutation {
        name: "joins-null-side-conjunct",
        package: Package::Core,
        breaks: "a conjunct over a LEFT join's null-supplying side is placed below it",
        file: "src/physical/enumerate/extract.rs",
        original: "if !pending.iter().all(preserved) {",
        replacement: "if pending.iter().all(preserved) && pending.is_empty() && !pending.is_empty() {",
        integration: &["join_enumeration::random_chains_agree_with_the_greedy_order"],
        unit: &[],
    },
    Mutation {
        name: "joins-or-not-factored",
        package: Package::Core,
        breaks: "an equality each operand of an OR holds stays in each, so its tables are multiplied",
        file: DISJUNCTIONS_FILE,
        original: "    if common.is_empty() {",
        replacement: "    if common.is_empty() || !common.is_empty() {",
        integration: &[SHAPED_OR],
        unit: &[],
    },
    Mutation {
        name: "joins-implied-or-lost",
        package: Package::Core,
        breaks: "no table is filtered first by the OR of its own conjuncts",
        file: DISJUNCTIONS_FILE,
        original: "if !filtered.contains(&key(&own)) {",
        replacement: "if filtered.contains(&key(&own)) {",
        integration: &[SHAPED_OR],
        unit: &[],
    },
    Mutation {
        name: "joins-prefix-floor",
        package: Package::Core,
        breaks: "an index prefix counted past the limit is guessed to find fewer rows than were counted",
        file: "src/physical/access.rs",
        original: "let guessed = || by_keys().unwrap_or(share).max(floor);",
        replacement: "let guessed = || by_keys().unwrap_or(share).max(0.0_f64);",
        integration: &["benchmark_shapes::a_longer_equal_prefix_is_read"],
        unit: &[],
    },
];
