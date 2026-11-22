//! The mutations of the range path (RULES.md R-TEST-9, D785, D786, D787,
//! D821): a grouping by no column made as its one group, a decoded key
//! kept only once its bytes repeat, a projection of ascending columns made
//! in the row it is given, and a row's key compared with its entry's as
//! the bytes the index holds.

use super::super::{Mutation, Package};

/// Mechanisms of the range path whose removal must fail their tests.
pub const RANGE_PATH: [Mutation; 7] = [
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
    Mutation {
        name: "range-path-encoded-bytes-compared",
        package: Package::Core,
        breaks: "a row is kept by any entry, so a key an update moved finds its row under the new key too",
        file: "src/exec/entries.rs",
        original: "    if made.as_slice() == key {",
        replacement: "    if !made.is_empty() {",
        integration: &[],
        unit: &["exec::scan::encoded::tests::an_encoded_scan_keeps_a_row_under_the_key_it_sees"],
    },
    Mutation {
        name: "range-path-encoded-unchecked-only",
        package: Package::Core,
        breaks: "a range whose keys must be checked is searched with its keys unchecked",
        file: "src/index/blink/encoded.rs",
        original: "    let range = index.range(probe).filter(|range| !range.checked)?;",
        replacement: "    let range = index.range(probe)?;",
        integration: &[],
        unit: &["index::blink::tests::an_encoded_search_gives_the_keys_search_finds"],
    },
    Mutation {
        name: "range-path-encoded-row-id-cut",
        package: Package::Core,
        breaks: "an entry's key is lent with its row id, so no row's key equals it and none decodes",
        file: "src/index/blink/encoded.rs",
        original: "            let (key, row) = split(entry)?;",
        replacement: "            let (key, row) = (entry, split(entry)?.1);",
        integration: &[],
        unit: &["index::blink::tests::an_encoded_search_gives_the_keys_search_finds"],
    },
    Mutation {
        name: "range-path-encoded-search-counted",
        package: Package::Core,
        breaks: "a scan whose keys are compared as bytes counts no search and no entry as the index's",
        file: "src/exec/scan/encoded.rs",
        original: "        using.gave(1);",
        replacement: "        using.gave(0);",
        integration: &["table_statistics::an_indexs_searches_and_entries_are_counted"],
        unit: &[],
    },
];
