//! The mutations of the one estimator (RULES.md R-TEST-9, D370-D372): the
//! floor of one row lost, Cardenas' formula taken as the lesser of the
//! rows and the values, its power rounded to 1, the whole table taken as a
//! sample, an index's sketch ignored, an outer join's preserved rows lost,
//! lineage stopped at a grouping, a table joined with itself taken as one
//! relation, and the optimizer's local fixpoint lost.

use super::super::{Mutation, Package};

/// The estimator's shared definitions.
const CARDINALITY_FILE: &str = "src/physical/cardinality.rs";
/// The groups.
const GROUPS_FILE: &str = "src/physical/cardinality/groups.rs";
/// The lineage of a column.
const LINEAGE_FILE: &str = "src/physical/cardinality/lineage.rs";

/// Mechanisms of the estimator and the rule driver whose removal must fail
/// their tests.
pub const ESTIMATES: [Mutation; 9] = [
    Mutation {
        name: "estimates-one-row-floor",
        package: Package::Core,
        breaks: "a filter keeping no row of an input of rows is estimated to give none",
        file: CARDINALITY_FILE,
        original: "rows.max(of.min(1.0_f64)).max(0.0_f64)",
        replacement: "rows.max(0.0_f64)",
        integration: &[],
        unit: &["physical::cardinality::tests::defaults_of_what_has_no_count"],
    },
    Mutation {
        name: "estimates-cardenas-as-least",
        package: Package::Core,
        breaks: "n rows of D values are taken to meet min(n, D) of them, not D(1 - (1 - 1/D)^n)",
        file: GROUPS_FILE,
        original: "(d * met).clamp(1.0_f64, n)",
        replacement: "d.clamp(1.0_f64, n)",
        integration: &[],
        unit: &["physical::cardinality::tests::cardenas_at_its_boundaries"],
    },
    Mutation {
        name: "estimates-cardenas-power-rounded",
        package: Package::Core,
        breaks: "rows drawn from far more values than a number tells from 1 are estimated to meet one of them",
        file: GROUPS_FILE,
        original: "let met = -((-1.0_f64 / d).ln_1p() * n).exp_m1();",
        replacement: "let met = 1.0_f64 - ((-1.0_f64 / d).ln_1p() * n).exp();",
        integration: &[],
        unit: &["physical::cardinality::tests::cardenas_of_values_past_a_number_s_digits"],
    },
    Mutation {
        name: "estimates-whole-table-sampled",
        package: Package::Core,
        breaks: "a grouping of every row of its table by a key is estimated as a sample's groups",
        file: GROUPS_FILE,
        original: "            return distinct.min(n).max(n.min(1.0_f64));",
        replacement: "            return cardenas(distinct, n);",
        integration: &[],
        unit: &["physical::cardinality::tests::groupings_are_estimated_by_their_groups"],
    },
    Mutation {
        name: "estimates-sketch-ignored",
        package: Package::Core,
        breaks: "a grouping by an index's key multiplies its columns' distinct values, correlated or not",
        file: GROUPS_FILE,
        original: "(self.sketched(table, &keys).unwrap_or(capped), rows)",
        replacement: "(capped, rows)",
        integration: &[],
        unit: &["physical::cardinality::tests::groupings_are_estimated_by_their_groups"],
    },
    Mutation {
        name: "estimates-outer-left-lost",
        package: Package::Core,
        breaks: "a LEFT join is estimated below its left operand's rows",
        file: "src/physical/cardinality/nodes.rs",
        original: "rows = rows.max(l.rows);",
        replacement: "rows = rows.max(0.0_f64);",
        integration: &[],
        unit: &["physical::cardinality::tests::outer_joins_keep_their_preserved_rows"],
    },
    Mutation {
        name: "estimates-grouping-lineage",
        package: Package::Core,
        breaks: "a grouping column above a grouping has no table, so a filter or join over it takes defaults",
        file: LINEAGE_FILE,
        original: "LogicalPlan::Aggregate { groups, input, .. } => Some((&**input, *groups.get(column)?)),",
        replacement: "LogicalPlan::Aggregate { .. } => None,",
        integration: &[],
        unit: &["physical::cardinality::tests::lineage_through_each_node"],
    },
    Mutation {
        name: "estimates-self-join-one-relation",
        package: Package::Core,
        breaks: "two scans of one table are taken as one relation, so an equality of their columns is a filter",
        file: LINEAGE_FILE,
        original: "if first.is_some_and(|known| !std::ptr::eq(known.scan, found.scan)) {",
        replacement: "if first.is_some_and(|known| known.table != found.table) {",
        integration: &[],
        unit: &["physical::cardinality::tests::lineage_through_each_node"],
    },
    Mutation {
        name: "estimates-local-fixpoint-lost",
        package: Package::Core,
        breaks: "a node a rule rewrote is left to the next pass, so the rules take as many passes as when restarted",
        file: "src/optimizer.rs",
        original: "    while *budget > 0 {",
        replacement: "    for _ in 0..usize::from(*budget > 0) {",
        integration: &[],
        unit: &["optimizer::drivers::rewriting_a_node_again_takes_fewer_passes"],
    },
];
