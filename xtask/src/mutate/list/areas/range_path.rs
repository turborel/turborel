//! The mutations of the range path (RULES.md R-TEST-9, D785, D786, D787):
//! a grouping by no column made as its one group, a decoded key kept only
//! once its bytes repeat, and a projection of ascending columns made in
//! the row it is given.

use super::super::{Mutation, Package};

/// Mechanisms of the range path whose removal must fail their tests.
pub const RANGE_PATH: [Mutation; 3] = [
    Mutation {
        name: "range-path-grouped-made-whole",
        package: Package::Core,
        breaks: "a grouping by a column is made as one group",
        file: "src/exec/group/streamed.rs",
        original: "    if !spec.groups.is_empty() || !spec.seeds.is_empty() || alone {",
        replacement: "    if !spec.seeds.is_empty() || alone {",
        integration: &[],
        unit: &["exec::group::tests::a_grouping_by_no_column_is_made_whole"],
    },
    Mutation {
        name: "range-path-first-key-kept",
        package: Package::Core,
        breaks: "a key is kept as it is first decoded and its bytes are not, so a repeat decodes it again",
        file: "src/index/blink/entries.rs",
        original: "        if again {",
        replacement: "        if !again {",
        integration: &[],
        unit: &["index::blink::tests::a_key_is_decoded_once_for_its_entries"],
    },
    Mutation {
        name: "range-path-repeat-taken-in-place",
        package: Package::Core,
        breaks: "a projection naming a column twice is made in place, the second taking another column's value",
        file: "src/exec/project.rs",
        original: "w.first().map(|c| c.0) < w.get(1).map(|c| c.0)",
        replacement: "w.first().map(|c| c.0) <= w.get(1).map(|c| c.0)",
        integration: &[],
        unit: &["exec::project::tests::ascending_projections_keep_their_row"],
    },
];
