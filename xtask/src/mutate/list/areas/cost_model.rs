//! The mutations of the cost model (RULES.md R-TEST-9, D650 to D659): a
//! filter's conjuncts left in their written order, a filter that can
//! raise an error ranked as one that cannot, rows past the grant not
//! spilled, the degree of a part ignored, and a runtime filter's rows
//! ignored.

use super::super::{Mutation, Package};

/// The order of a filter's conjuncts.
const CONJUNCTS_FILE: &str = "src/physical/cost/conjuncts.rs";

/// Mechanisms of the cost model whose removal must fail their tests.
pub const COST_MODEL: [Mutation; 5] = [
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
    Mutation {
        name: "spill-ignored",
        package: Package::Core,
        breaks: "an operator past its grant is priced as if it held its rows in memory",
        file: "src/physical/cost/spill.rs",
        original: "    moved * (per_row + SPILLED_BYTE * bytes)",
        replacement: "    0.0_f64 * moved * per_row * bytes",
        integration: &[
            "cost_model::a_sort_past_its_grant_gives_way_to_the_index_order",
            "cost_model::a_grouping_past_its_grant_streams_over_a_sort",
        ],
        unit: &[
            "physical::cost::tests::rows_past_the_grant_are_spilled_once_more",
            "physical::enumerate::weighed::tests::a_build_past_its_grant_costs_its_spill",
        ],
    },
    Mutation {
        name: "degree-ignored",
        package: Package::Core,
        breaks: "a join's method is chosen as if every part ran on one thread",
        file: "src/physical/cost/parallel.rs",
        original: "        cost_of(self.degree(crate::physical::parallel::rows_of(rows)))",
        replacement: "        cost_of(self.degree(crate::physical::parallel::rows_of(rows)).min(1))",
        integration: &["cost_model::a_parallel_part_hands_the_index_join_to_the_hash_join_sooner"],
        unit: &["physical::enumerate::weighed::tests::a_parallel_part_shares_the_hashing"],
    },
    Mutation {
        name: "runtime-filter-ignored",
        package: Package::Core,
        breaks: "a hash join's probing rows are all hashed though its runtime filter drops most",
        file: "src/physical/enumerate/weighed.rs",
        original: "        let share = if kind == Kind::Inner && w.runtime_filters {",
        replacement: "        let share = if kind == Kind::Inner && w.runtime_filters && false {",
        integration: &[],
        unit: &["physical::enumerate::weighed::tests::a_selective_build_filters_its_probing_rows"],
    },
];
