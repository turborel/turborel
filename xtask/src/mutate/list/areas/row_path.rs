//! The mutations of the row store's write path (RULES.md R-TEST-9, D750):
//! an update's kept columns found by their stored bytes, the writer adding
//! no entry an index shares, collection, an index build and a checkpoint
//! taking back none it shares.

use super::super::{Mutation, Package};

/// Mechanisms of the row store's write path whose removal must fail their tests.
pub const ROW_PATH: [Mutation; 5] = [
    Mutation {
        name: "row-path-kept-by-bytes",
        package: Package::Core,
        breaks: "an update's kept columns are those whose stored bytes are the same, not every inline one",
        file: "src/engine/record.rs",
        original: "            (Field::Inline(left), Field::Inline(right)) => left == right,",
        replacement: "            (Field::Inline(left), Field::Inline(right)) => left.len() == right.len() || true,",
        integration: &["shared_entries::a_run_of_one_key_keeps_one_entry_until_its_key_changes"],
        unit: &[],
    },
    Mutation {
        name: "row-path-writer-shares",
        package: Package::Core,
        breaks: "an update of no column an index reads adds no entry to it",
        file: "src/txn/write.rs",
        original: "            if ix.shares_entry(kept) {",
        replacement: "            if ix.shares_entry(kept) && kept.is_empty() {",
        integration: &["shared_entries::an_update_of_no_key_column_adds_no_entry"],
        unit: &[],
    },
    Mutation {
        name: "row-path-collection-keeps-shared",
        package: Package::Core,
        breaks: "collection takes back no entry the version after shares",
        file: "src/txn/end.rs",
        original: "        if ix.shares_entry(kept) {",
        replacement: "        if ix.shares_entry(kept) && kept.is_empty() {",
        integration: &["shared_entries::an_update_of_no_key_column_adds_no_entry"],
        unit: &[],
    },
    Mutation {
        name: "row-path-build-skips-shared",
        package: Package::Core,
        breaks: "an index build gives no entry to a readable version whose entry the next shares",
        file: "src/engine/rows/mvcc.rs",
        original: "                let passed = appended.is_some_and(|lsn| shared.contains(&lsn.0));",
        replacement: "                let passed = appended.is_some_and(|lsn| shared.contains(&lsn.0) && lsn.0 == 0);",
        integration: &["shared_entries::an_index_built_after_a_shared_update_takes_the_entries_collection_does"],
        unit: &[],
    },
    Mutation {
        name: "row-path-checkpoint-skips-shared",
        package: Package::Core,
        breaks: "a checkpoint logs no entry a live version shares as stale, for recovery to take back",
        file: "src/db/checkpoint.rs",
        original: "    for ix in indexes.iter().filter(|ix| !ix.shares_entry(kept)) {",
        replacement: "    for ix in indexes.iter().filter(|ix| !ix.shares_entry(kept) || kept.len() < usize::MAX) {",
        integration: &["shared_entries::a_checkpoint_logs_no_shared_entry_as_stale"],
        unit: &[],
    },
];
