//! The mutations of groupjoins and index joins by batch (RULES.md R-TEST-9,
//! D490, D491): a LEFT join's unmatched groups, the order of groups by
//! their first probe rows, the uniqueness that makes each group one row of
//! the group operand, and the outer rows' order of a join by batch.

use super::{Mutation, Package};

/// The mutations of groupjoins and index joins by batch, in the order
/// they run.
pub const GROUPJOINS: [Mutation; 7] = [
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
        name: "batched-index-join-order-lost",
        package: Package::Core,
        breaks: "an index join by batch gives its outer rows last first",
        file: "src/exec/join/indexed/batched.rs",
        original: "let Some((row, at)) = self.queue.pop_front() else",
        replacement: "let Some((row, at)) = self.queue.pop_back() else",
        integration: &["index_join_batches::batches_of_outer_rows_join_as_rows_alone"],
        unit: &[],
    },
    Mutation {
        name: "batched-index-join-repeat-taken",
        package: Package::Core,
        breaks: "an index join by batch gives a key's rows to the first of the outer rows that repeat it alone",
        file: "src/exec/join/indexed/batched.rs",
        original: "let taken = if *uses == 0 {",
        replacement: "let taken = if *uses <= 1 {",
        integration: &["index_join_batches::batches_of_outer_rows_join_as_rows_alone"],
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
