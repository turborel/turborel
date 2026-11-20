//! The mutations of the statistics of RULES.md D381 to D383 (R-TEST-9):
//! the common values ignored, a run's repeats counted each, the bounds
//! kept after rows are written, the sample's count replaced by the
//! synopses', a rare combination's joint distinct values ignored, an
//! unseen one's share ignored, conjuncts multiplied, a LIKE's prefix ignored, an index's
//! range share lost and an index's joint distinct keys ignored.

use super::super::{Mutation, Package};

/// The estimate of one column's condition.
const SELECTIVITY_FILE: &str = "src/stats/selectivity.rs";
/// The planner's estimator of a condition.
const ESTIMATE_FILE: &str = "src/physical/estimate.rs";

/// Equalities' joint distinct values in the sample.
const JOINT_FILE: &str = "src/physical/estimate/joint.rs";
/// The test of rare equalities.
const RARE: &str = "physical::estimate::tests::rare_equalities_are_estimated_by_joint_distinct_values";

/// Mechanisms of the statistics whose removal must fail their tests.
pub const STATISTICS: [Mutation; 10] = [
    Mutation {
        name: "statistics-common-ignored",
        package: Package::Core,
        breaks: "an equality with a common value keeps that value's share",
        file: SELECTIVITY_FILE,
        original: "let equal = share_of(&self.common, value).unwrap_or(",
        replacement: "let equal = None::<f64>.unwrap_or(",
        integration: &["statistics_estimates::a_common_value_is_estimated_by_its_share_and_scanned"],
        unit: &["stats::common::tests::a_skewed_columns_common_values_are_estimated_by_their_shares"],
    },
    Mutation {
        name: "statistics-runs-uncollapsed",
        package: Package::Core,
        breaks: "a value's repeats within one run of row ids count once",
        file: "src/stats.rs",
        original: "let collapsed = count(seen.values().map(|r| u64::from(r.runs)).sum());",
        replacement: "let collapsed = present;",
        integration: &[],
        unit: &["engine::rows::stats::tests::a_large_table_is_sampled_and_a_small_one_read_whole"],
    },
    Mutation {
        name: "statistics-bounds-kept-after-change",
        package: Package::Core,
        breaks: "once rows are written, a range past the greatest value keeps rows",
        file: SELECTIVITY_FILE,
        original: "let bounded = !self.unbounded && !since.changed();",
        replacement: "let bounded = !self.unbounded;",
        integration: &["statistics_estimates::a_range_past_the_greatest_value_keeps_rows_once_written"],
        unit: &["stats::since::tests::a_range_past_the_greatest_value_keeps_rows_written_since"],
    },
    Mutation {
        name: "statistics-sample-count-ignored",
        package: Package::Core,
        breaks: "a conjunction the sample keeps enough rows of is estimated by them",
        file: "src/physical/estimate/sampled.rs",
        original: "        return cost_of(hits) / n;",
        replacement: "        return synopses;",
        integration: &["statistics_estimates::a_correlated_conjunction_is_estimated_on_the_sample"],
        unit: &["physical::estimate::tests::a_correlated_conjunction_is_estimated_on_the_sample"],
    },
    Mutation {
        name: "statistics-backoff-multiplied",
        package: Package::Core,
        breaks: "each further conjunct counts by a root of its share",
        file: "src/physical/estimate/backoff.rs",
        original: "        kept *= share.powf(root);",
        replacement: "        kept *= share;",
        integration: &[],
        unit: &[
            "physical::estimate::backoff::tests::the_most_selective_counts_whole_and_the_rest_less",
            "physical::estimate::tests::conjuncts_no_sample_answers_combine_by_backoff",
        ],
    },
    Mutation {
        name: "statistics-like-prefix-ignored",
        package: Package::Core,
        breaks: "a LIKE of a constant prefix keeps the range of strings beginning with it",
        file: ESTIMATE_FILE,
        original: "Cond::Like(..) => like::selectivity(cond, known).unwrap_or(guess.range_selectivity),",
        replacement: "Cond::Like(..) => like::selectivity(cond, known).and(None).unwrap_or(guess.range_selectivity),",
        integration: &["statistics_estimates::a_like_of_a_prefix_is_estimated_as_its_range"],
        unit: &[],
    },
    Mutation {
        name: "statistics-index-range-share-lost",
        package: Package::Core,
        breaks: "an index's range past its counted entries is estimated by its share of the tree",
        file: "src/index/blink.rs",
        original: "            let share = self.tree.range_share(lower, upper).ok()?;",
        replacement: "            let share = self.tree.range_share(lower, upper).ok().map(|_| 0.0_f64)?;",
        integration: &["statistics_estimates::an_index_range_is_estimated_by_its_share_of_the_tree"],
        unit: &["index::blink::tests::a_range_is_estimated_by_its_share_of_the_tree"],
    },
    Mutation {
        name: "statistics-joint-keys-ignored",
        package: Package::Core,
        breaks: "columns that are exactly an index's key are as distinct as its keys",
        file: "src/stats/keyed.rs",
        original: "self.exact(&columns).or_else(",
        replacement: "self.exact(&columns).and(None).or_else(",
        integration: &[],
        unit: &["index::blink::tests::an_indexs_columns_are_as_distinct_as_its_keys"],
    },
    Mutation {
        name: "statistics-joint-seen-ignored",
        package: Package::Core,
        breaks: "equalities too rare for the sample to count take their columns' joint distinct values",
        file: JOINT_FILE,
        original: "        return Some(present / n / scaled.max(1.0_f64));",
        replacement: "        return None;",
        integration: &[],
        unit: &[RARE],
    },
    Mutation {
        name: "statistics-joint-unseen-ignored",
        package: Package::Core,
        breaks: "a combination the sample never holds, of columns it saw whole, keeps no row",
        file: JOINT_FILE,
        original: "    Some(if unseen < 1.0_f64 { 0.0_f64 } else { once / n / unseen })",
        replacement: "    None",
        integration: &[],
        unit: &[RARE],
    },
];
