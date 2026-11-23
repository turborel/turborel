//! The mutations of a row's fetch by its id (RULES.md R-TEST-9, D950,
//! D955, D954): a key found in a leaf's bytes, the leaf found through
//! the inner nodes' copies, and the snapshot of a transaction whose
//! reads are not checked.

use super::super::{Mutation, Package};

/// Mechanisms of a fetch whose removal must fail their tests.
pub const FETCH_PATH: [Mutation; 4] = [
    Mutation {
        name: "fetch-lean-taken-out",
        package: Package::Core,
        breaks: "a key is found in a leaf marked taken out of its tree, whose keys have moved right",
        file: "src/engine/tree/node/lean.rs",
        original: "if lean.kind() != (0, false) || u64::from_le_bytes",
        replacement: "if lean.kind().0 != 0 || u64::from_le_bytes",
        integration: &[],
        unit: &["engine::tree::node::tests::lean::a_key_is_not_found_in_bytes_that_are_no_leaf_of_the_tree"],
    },
    Mutation {
        name: "fetch-copies-inner-as-leaf",
        package: Package::Core,
        breaks: "the walk through the inner nodes' copies stops a level early and gives an inner node as the leaf",
        file: "src/engine/tree/inner.rs",
        original: "            if level == 1 {",
        replacement: "            if level <= 2 {",
        integration: &[],
        unit: &["engine::tree::tests::by_copies::a_read_through_the_copies_finds_what_a_descent_finds"],
    },
    Mutation {
        name: "fetch-copies-unchecked",
        package: Package::Core,
        breaks: "the walk through the inner nodes' copies follows a copy whatever its frame's version",
        file: "src/engine/tree/inner.rs",
        original: "child_in(&copies.current(pool, (file, id))?.bytes, key)",
        replacement: "child_in(&copies.kept.get(*copies.at.get(&(file, id))?)?.bytes, key)",
        integration: &[],
        unit: &["engine::tree::tests::by_copies::a_copy_changed_since_is_not_followed"],
    },
    Mutation {
        name: "fetch-unchecked-always",
        package: Package::Core,
        breaks: "a transaction's bookkeeping says its reads are not checked even while they are, so a serializable read records nothing",
        file: "src/engine/rows/mvcc/opens.rs",
        original: "self.unchecked.store(if checked { CHECKED } else { snapshot }, Ordering::Release);",
        replacement: "self.unchecked.store(snapshot, Ordering::Release);",
        integration: &[],
        unit: &["engine::rows::mvcc::found::tests::a_read_not_checked_takes_no_lock_of_its_bookkeeping"],
    },
];
