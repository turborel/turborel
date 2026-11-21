//! The mutations of the cost model (RULES.md R-TEST-9, D650 to D659): a
//! filter's conjuncts left in their written order, and a filter that can
//! raise an error ranked as one that cannot.

use super::super::{Mutation, Package};

/// The order of a filter's conjuncts.
const CONJUNCTS_FILE: &str = "src/physical/cost/conjuncts.rs";

/// Mechanisms of the cost model whose removal must fail their tests.
pub const COST_MODEL: [Mutation; 2] = [
    Mutation {
        name: "conjuncts-unranked",
        package: Package::Core,
        breaks: "a filter over a table read judges its conjuncts in their written order, not by rank",
        file: "src/physical/access/filter.rs",
        original: "    let plan = match and_all(ranked(conds, with.known)) {",
        replacement: "    let plan = match and_all(conds) {",
        integration: &["cost_model::a_filter_judges_its_selective_conjunct_first"],
        unit: &[],
    },
    Mutation {
        name: "conjuncts-raising-ranked",
        package: Package::Core,
        breaks: "a filter that can raise an error has its conjuncts reordered",
        file: CONJUNCTS_FILE,
        original: "    if conds.len() < 2 || conds.iter().any(Cond::can_raise) {",
        replacement: "    if conds.len() < 2 {",
        integration: &["cost_model::a_filter_that_can_raise_keeps_the_written_order"],
        unit: &["physical::cost::conjuncts::tests::a_filter_that_can_raise_keeps_its_order"],
    },
];
