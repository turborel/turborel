//! The mutations of the transaction layer (RULES.md R-TEST-9, D418 on):
//! a commit's one store in the status table, and a version committed but
//! not stamped appended stamped.

use super::super::{Mutation, Package};

/// Mechanisms of the transaction layer whose removal must fail their tests.
pub const TXN_LAYER: [Mutation; 2] = [
    Mutation {
        name: "txn-commit-not-stored",
        package: Package::Core,
        breaks: "a commit's timestamp is not stored in the status table, so its versions read as open until stamped",
        file: "src/engine/rows/mvcc/status.rs",
        original: "            slot.at.store(at, Ordering::SeqCst);\n            return;",
        replacement: "            return;",
        integration: &[],
        unit: &[
            "engine::rows::mvcc::status::tests::a_commit_is_one_store",
            "engine::rows::mvcc::stamps::tests::a_commit_is_seen_before_its_stamp",
        ],
    },
    Mutation {
        name: "txn-unstamped-appended-as-read",
        package: Package::Core,
        breaks: "a version committed but not stamped is appended to the undo store as read, naming its transaction",
        file: "src/engine/rows/mvcc/versions.rs",
        original: "                let appended = if version.maker == Maker::Committed(at) {",
        replacement: "                let appended = if true {",
        integration: &[],
        unit: &["engine::rows::mvcc::stamps::tests::an_unstamped_version_is_appended_stamped"],
    },
];
