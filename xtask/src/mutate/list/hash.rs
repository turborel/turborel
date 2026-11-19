//! Mutations of the hash index: its streams holding a discarded index,
//! a row of a key inserted twice given once, and its table's pages split
//! and merged (RULES.md R-TEST-9, D34).

use super::{Mutation, Package};

/// Each hash index mechanism broken alone, and the tests that fail.
pub const HASH: [Mutation; 4] = [
    Mutation {
        name: "hash-key-stream-owner",
        package: Package::Core,
        breaks: "a discarded hash index frees pages while its key-row stream reads them",
        file: "src/index/hash.rs",
        original: "let _rows_owner = &self;",
        replacement: "let _rows_owner = ();",
        integration: &[],
        unit: &["index::hash::tests::streams_retain_discarded_indexes_until_they_close"],
    },
    Mutation {
        name: "hash-search-stream-owner",
        package: Package::Core,
        breaks: "a discarded hash index frees pages while a search still reads them",
        file: "src/index/hash.rs",
        original: "let _entries_owner = &self;",
        replacement: "let _entries_owner = ();",
        integration: &["indexes::indexes_are_dropped"],
        unit: &["index::hash::tests::streams_retain_discarded_indexes_until_they_close"],
    },
    Mutation {
        name: "hash-key-twice",
        package: Package::Core,
        breaks: "a hash index gives a row of a key inserted twice twice",
        file: "src/index/hash.rs",
        original: "            if given.contains(&v)? {",
        replacement: "            if given.contains(&v)? && v.is_empty() {",
        integration: &[],
        unit: &["index::hash::tests::a_key_of_many_rows_is_read_a_page_at_a_time"],
    },
    Mutation {
        name: "hash-tail-kept",
        package: Package::Core,
        breaks: "a hash table appends to a page a split or merge rewrote",
        file: "src/engine/hash.rs",
        original: "            self.set_tail(b, 0);",
        replacement: "            drop(b);",
        integration: &[],
        unit: &[
            "engine::hash::tests::it_holds_what_a_model_holds",
            "engine::hash::tests::a_hot_keys_bucket_splits_and_merges_a_page_at_a_time",
        ],
    },
];
