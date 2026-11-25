//! The mutations of sampled statistics (RULES.md R-TEST-9): a large table
//! sampled, a sample's distinct values scaled, and a sample's least and
//! greatest values not taken as bounds.

use super::{Mutation, Package};

/// The mutations of sampled statistics, in the order they run.
pub const STATS: [Mutation; 8] = [
    Mutation {
        name: "stats-large-read-whole",
        package: Package::Core,
        breaks: "sampled statistics: a large table is read whole by each pass, so its writes pay for passes in proportion to its rows",
        file: "src/engine/rows/stats.rs",
        original: "        let sampling = rows > SAMPLED_ROWS.saturating_mul(2);",
        replacement: "        let sampling = false;",
        integration: &[],
        unit: &["engine::rows::stats::tests::a_large_table_is_sampled_and_a_small_one_read_whole"],
    },
    Mutation {
        name: "stats-distinct-unscaled",
        package: Package::Core,
        breaks: "sampled statistics: a sample's distinct values are taken as the table's, so a column of distinct values looks repetitive",
        file: "src/stats.rs",
        original: "    let scaled = if below > 0.0_f64 { sampled * distinct / below } else { of };",
        replacement: "    let scaled = distinct;",
        integration: &[],
        unit: &[
            "stats::tests::a_sample_scales_its_distinct_values_and_bounds_nothing",
            "engine::rows::stats::tests::a_large_table_is_sampled_and_a_small_one_read_whole",
        ],
    },
    Mutation {
        name: "stats-sample-bounded",
        package: Package::Core,
        breaks: "sampled statistics: a value outside a sample's least and greatest is estimated to match no row",
        file: "src/stats/selectivity.rs",
        original: "let bounded = !self.unbounded && !since.changed();",
        replacement: "let bounded = !since.changed();",
        integration: &[],
        unit: &["stats::tests::a_sample_scales_its_distinct_values_and_bounds_nothing"],
    },
    Mutation {
        name: "stats-first-not-asked",
        package: Package::Core,
        breaks: "first statistics: a table opened gives the plan first asking none, and plans without them",
        file: "src/engine/rows/stats.rs",
        original: "        self.gather_first();",
        replacement: "",
        integration: &[],
        unit: &["engine::rows::stats::tests::a_table_opened_gathers_its_first_statistics_as_a_plan_asks"],
    },
    Mutation {
        name: "stats-sampled-group-read",
        package: Package::Core,
        breaks: "a sampled pass: the header of a group none of whose rows it chose is read",
        file: "src/engine/rows/frozen/sampled.rs",
        original: "            if wanted.as_ref().is_some_and(Vec::is_empty) {",
        replacement: "            if false {",
        integration: &[],
        unit: &["engine::rows::frozen::shadowed::a_sampled_pass_reads_only_the_groups_of_its_runs"],
    },
    Mutation {
        name: "frozen-fetch-chunk-before",
        package: Package::Core,
        breaks: "a frozen row read by id: the first row of a chunk is looked for in the chunk before",
        file: "src/engine/columns/group.rs",
        original: "z.run.first.saturating_add(z.run.rows) <= position))",
        replacement: "z.run.first.saturating_add(z.run.rows) < position))",
        integration: &[],
        unit: &["engine::rows::frozen::shadowed::a_frozen_row_is_read_by_id_of_its_listed_columns_alone"],
    },
    Mutation {
        name: "header-read-given-up",
        package: Package::Core,
        breaks: "headers held: a header read since it was put is given up before one not read",
        file: "src/engine/columns/cache.rs",
        original: "            if looks > 0 && held.slot.read.swap(false, Ordering::Relaxed) {",
        replacement: "            if looks > 0 && !held.slot.read.swap(false, Ordering::Relaxed) {",
        integration: &[],
        unit: &["engine::columns::cache::tests::headers_not_read_lately_go_first"],
    },
    Mutation {
        name: "header-bound-no-share",
        package: Package::Core,
        breaks: "headers held: a setting of -1 holds none, not a share of the memory budget",
        file: "src/engine/columns/cache.rs",
        original: "        return budget.checked_div(BUDGET_SHARE).unwrap_or(0);",
        replacement: "        return 0;",
        integration: &[],
        unit: &["engine::columns::cache::tests::a_bound_changed_takes_effect_at_once"],
    },
];
