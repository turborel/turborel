//! The mutations of reads near the last (RULES.md R-TEST-9): what lets a
//! remembered leaf answer; and of what lets a probe go unpriced.

use super::{Mutation, Package};

/// The mutations of reads near the last and point paths, in the order they
/// run.
pub const NEAR: [Mutation; 3] = [
    Mutation {
        name: "near-stamp",
        package: Package::Core,
        breaks: "reads near the last: a leaf freed and made another tree's answers with that tree's row",
        file: "src/engine/tree/near.rs",
        original: "near.leaf.filter(|&(_, at)| at == stamp && near.misses < MOST_MISSES)",
        replacement: "near.leaf.filter(|&(_, at)| at <= stamp && near.misses < MOST_MISSES)",
        integration: &[],
        unit: &["engine::tree::tests::a_near_read_trusts_no_leaf_retired"],
    },
    Mutation {
        name: "near-first-key",
        package: Package::Core,
        breaks: "reads near the last: a key below a leaf's first is said to be absent, though the leaf left of it holds it",
        file: "src/engine/tree/near.rs",
        original: " && node.key(0)? <= key)",
        replacement: " && (node.key(0)? <= key || key.len() > 0))",
        integration: &[],
        unit: &["engine::tree::tests::a_near_read_says_what_a_get_says"],
    },
    Mutation {
        name: "point-unique",
        package: Package::Core,
        breaks: "point paths: an index that is not unique goes unpriced, and is chosen for a key every row has",
        file: "src/physical/access.rs",
        original: "    ix.unique && !ix.is_collated()",
        replacement: "    !ix.is_collated()",
        integration: &["point_plans::a_key_that_is_not_unique_is_still_priced"],
        unit: &[],
    },
];
