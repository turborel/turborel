//! The mutations of sampled statistics (RULES.md R-TEST-9): a large table
//! sampled, a sample's distinct values scaled, and a sample's least and
//! greatest values not taken as bounds.

use super::{Mutation, Package};

/// The mutations of sampled statistics, in the order they run.
pub const STATS: [Mutation; 3] = [
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
        file: "src/stats.rs",
        original: "        let outside = !self.unbounded",
        replacement: "        let outside = true",
        integration: &[],
        unit: &["stats::tests::a_sample_scales_its_distinct_values_and_bounds_nothing"],
    },
];
