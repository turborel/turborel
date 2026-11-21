//! The mutations of the row path (RULES.md R-TEST-9, D750): a row change's
//! log record made from the images as they are held, sealed from its
//! bytes; and collection reading only the versions whose entries it takes
//! back.

use super::super::{Mutation, Package};

/// Mechanisms of the row path whose removal must fail their tests.
pub const ROW_PATH: [Mutation; 4] = [
    Mutation {
        name: "row-path-update-images-swapped",
        package: Package::Core,
        breaks: "an update's record made from held rows writes its after-image before its before-image",
        file: "src/log/record/batch.rs",
        original: "    super::images::put_update(&mut out, (table, generation, id), before, after);",
        replacement: "    super::images::put_update(&mut out, (table, generation, id), after, before);",
        integration: &[],
        unit: &["log::record::tests::records_made_from_held_rows_are_their_changes"],
    },
    Mutation {
        name: "row-path-row-change-unsealed",
        package: Package::Core,
        breaks: "a row change of a sealed table is logged in the clear",
        file: "src/txn/write.rs",
        original: "            Some(pipeline) => tx.log_encoded(&seal_plain(&plain, id, &pipeline)?, 1),",
        replacement: "            Some(_) => tx.log_encoded(&plain, 1),",
        integration: &["table_encodings::an_encrypted_tables_log_records_are_sealed"],
        unit: &[],
    },
    Mutation {
        name: "row-path-collection-reads-the-unneeded",
        package: Package::Core,
        breaks: "collection reads the versions it does not need and takes back no entry of those it does",
        file: "src/engine/rows/mvcc/gc.rs",
        original: "        let gone = !removed.needs(kept) || removed.removed(",
        replacement: "        let gone = removed.needs(kept) || removed.removed(",
        integration: &[],
        unit: &["engine::rows::tests::collection_reads_only_the_versions_it_needs"],
    },
    Mutation {
        name: "row-path-global-entries-unread",
        package: Package::Core,
        breaks: "collection reads no version of a partition under a global index whose own entries are shared, so its global entries stay",
        file: "src/txn/kept.rs",
        original: "    globals || indexes.into_iter().any(|sharing| !sharing.shares(kept))",
        replacement: "    indexes.into_iter().any(|sharing| !sharing.shares(kept))",
        integration: &[],
        unit: &["txn::kept::tests::a_version_is_read_only_for_entries_of_its_own"],
    },
];
