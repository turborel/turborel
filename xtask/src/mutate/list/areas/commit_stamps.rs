//! The mutations of a commit's stamps from where its writes left its rows
//! (RULES.md R-TEST-9, D843): the stamp written in place, the leaf trusted
//! only while the tree's stamp is unchanged, and the entry found from it;
//! and an index fill counted through a commit timestamp, not an id (D844).

use super::super::{Mutation, Package};

/// A commit's stamps of rows its own inserts moved across splits.
const STAMPED: &str = "engine::rows::mvcc::stamps::tests::a_commit_stamps_its_rows_where_their_writes_left_them";
/// Changes from where a write left its entry.
const PLACED: &str = "engine::tree::tests::placed::a_change_starts_where_the_write_left_the_entry";

/// Mechanisms of the stamps in place whose removal must fail their tests.
pub const COMMIT_STAMPS: [Mutation; 4] = [
    Mutation {
        name: "commit-stamps-flag",
        package: Package::Core,
        breaks: "a stamp in place writes the commit timestamp but not the flag, so the version still reads as made by a transaction",
        file: "src/engine/rows/version.rs",
        original: "        *flags |= COMMITTED;\n",
        replacement: "",
        integration: &[],
        unit: &["engine::rows::version::tests::a_stamp_in_place_encodes_the_committed_version", STAMPED],
    },
    Mutation {
        name: "commit-stamps-trusted",
        package: Package::Core,
        breaks: "a change from a write's leaf trusts the leaf after the tree retired one, when it may have left the tree",
        file: "src/engine/tree/placed.rs",
        original: "if !placed.is_some() || self.epochs.stamp() != placed.stamp {",
        replacement: "if !placed.is_some() {",
        integration: &[],
        unit: &[PLACED],
    },
    Mutation {
        name: "commit-stamps-entry-found",
        package: Package::Core,
        breaks: "a change from a write's leaf changes the leaf's first entry rather than the one of its key",
        file: "src/engine/tree/placed.rs",
        original: "Ok(node.search(key)?.ok())",
        replacement: "Ok(Some(0))",
        integration: &[],
        unit: &[PLACED, STAMPED],
    },
    Mutation {
        name: "commit-stamps-fill-through",
        package: Package::Core,
        breaks: "an index filled with the writers kept out is filled through the highest id, so later commits look older than the fill (D844)",
        file: "src/db/recovery.rs",
        original: "self.tx.latest_commit() } else { as_of };",
        replacement: "self.tx.highest() } else { as_of };",
        integration: &["undo_memory::a_crash_with_batches_kept_recovers_what_was_committed"],
        unit: &[],
    },
];
