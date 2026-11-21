//! The mutations of groupjoins (RULES.md R-TEST-9, D490): a LEFT join's
//! unmatched groups, the order of groups by their first probe rows, the
//! uniqueness that makes each group one row of the group operand, the
//! planner's note that a grouping is over a join, and the sampled distinct
//! values the choice of form reads.

use super::{Mutation, Package};

/// The mutations of groupjoins, in the order
/// they run.
pub const GROUPJOINS: [Mutation; 5] = [
    Mutation {
        name: "groupjoin-left-unmatched-dropped",
        package: Package::Core,
        breaks: "a LEFT join's group no row joined is left out, not fed a row of nulls",
        file: "src/exec/groupjoin.rs",
        original: "        let Some(values) = joined.or_else(|| empty.clone()) else { continue };",
        replacement: "        let Some(values) = joined else { continue };",
        integration: &["groupjoins::groupjoins_give_the_groupings_rows"],
        unit: &[],
    },
    Mutation {
        name: "groupjoin-first-order-skipped",
        package: Package::Core,
        breaks: "groups of a build that was the join's right operand come in the build's order, not their first probe rows'",
        file: "src/exec/groupjoin.rs",
        original: "    if g.by_first {",
        replacement: "    if g.by_first && g.outer {",
        integration: &["groupjoins::groupjoins_give_the_groupings_rows"],
        unit: &[],
    },
    Mutation {
        name: "groupjoin-uniqueness-unchecked",
        package: Package::Core,
        breaks: "a grouping whose columns hold no unique set of the group operand is planned as a groupjoin",
        file: "src/physical/groupjoin/recognized.rs",
        original: "    found_once(&kept, keys, |k| k.left)",
        replacement: "    !keys.is_empty()",
        integration: &["groupjoins::groupings_no_groupjoin_computes"],
        unit: &[],
    },
    Mutation {
        name: "sampled-distinct-runs-uncollapsed",
        package: Package::Core,
        breaks: "a sample read by runs counts keys whose rows lie together as the sample's few",
        file: "src/stats.rs",
        original: "let collapsed = count(seen.values().map(|r| u64::from(r.runs)).sum());",
        replacement: "let collapsed = present;",
        integration: &[],
        unit: &["stats::tests::a_sample_of_runs_counts_keys_whose_rows_lie_together"],
    },
    Mutation {
        name: "groupjoin-walk-never-asked",
        package: Package::Core,
        breaks: "a grouping planned over a join is not noted, so no groupjoin is made",
        file: "src/physical/grouping.rs",
        original: "self.est.inspect(|e| e.grouped_a_join());",
        replacement: "self.est.inspect(|_| ());",
        integration: &["groupjoins::groupjoins_give_the_groupings_rows"],
        unit: &[],
    },
];
