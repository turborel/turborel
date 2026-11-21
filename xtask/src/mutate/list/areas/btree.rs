//! The mutations of the B-link tree's node layout and leaf hints (RULES.md
//! R-TEST-9, D500).

use super::super::{Mutation, Package};

/// Mechanisms of the B-link tree whose removal must fail their tests.
pub const BTREE: [Mutation; 6] = [
    Mutation {
        name: "btree-hints-misread",
        package: Package::Core,
        breaks: "a hint equal to the key's head bounds the search from below, past slots that may hold the key",
        file: "src/engine/tree/node.rs",
        original: "            if hint < head {",
        replacement: "            if hint <= head {",
        integration: &[],
        unit: &["engine::tree::node::tests::layout::heads_and_hints_find_every_key"],
    },
    Mutation {
        name: "btree-prefix-kept",
        package: Package::Core,
        breaks: "a key inserted without the node's prefix leaves the prefix as it was, so keys read back wrong",
        file: "src/engine/tree/node/build.rs",
        original: "        (kept < node.prefix.len()).then(",
        replacement: "        (kept > node.prefix.len()).then(",
        integration: &[],
        unit: &["engine::tree::node::tests::layout::prefixes_are_kept_once_and_keys_read_whole"],
    },
    Mutation {
        name: "btree-foreign-leaf-trusted",
        package: Package::Core,
        breaks: "a noted page of another tree is trusted as a leaf of this one",
        file: "src/engine/tree/near.rs",
        original: "    if node.tree != tree || node.level != 0 || node.deleted || node.len() == 0 {",
        replacement: "    if node.level != 0 || node.deleted || node.len() == 0 {",
        integration: &[],
        unit: &["engine::tree::tests::hinted::a_noted_leaf_gone_is_not_trusted"],
    },
    Mutation {
        name: "btree-split-leaf-misnoted",
        package: Package::Core,
        breaks: "a row written into the right half of a split is noted in the left",
        file: "src/engine/tree/write.rs",
        original: ".filter(|s| key > s.separator.as_slice())",
        replacement: ".filter(|s| key < s.separator.as_slice())",
        integration: &[],
        unit: &["engine::tree::tests::hinted::a_row_read_starts_at_the_leaf_noted_for_it"],
    },
    Mutation {
        name: "btree-no-right-hops",
        package: Package::Core,
        breaks: "a read from a noted leaf split since descends instead of moving right",
        file: "src/engine/tree/near.rs",
        original: "        for hop in 0..=HOPS {",
        replacement: "        for hop in 0..1 {",
        integration: &[],
        unit: &["engine::tree::tests::hinted::a_noted_leaf_split_since_leads_right"],
    },
    Mutation {
        name: "btree-entry-hint-stale",
        package: Package::Core,
        breaks: "an index entry keeps its old hint, not the leaf noted for its row as it is written",
        file: "src/index/blink/value.rs",
        original: "        Some(leaf) => Ok(leaf),",
        replacement: "        Some(_) => old.map_or(Ok(hints::NO_LEAF), hint_of),",
        integration: &[],
        unit: &["index::blink::tests::entries_carry_the_leaf_noted_for_their_row"],
    },
];
