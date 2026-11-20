//! The mutations of the transaction layer (RULES.md R-TEST-9, D440 on):
//! a commit's one store in the status table, a version committed but not
//! stamped appended stamped, snapshots at the durable timestamp, a reply
//! held for what it read past its snapshot, and an exclusive request
//! waiting for the shared holds in stripes.

use super::super::{Mutation, Package};

/// Mechanisms of the transaction layer whose removal must fail their tests.
pub const TXN_LAYER: [Mutation; 5] = [
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
    Mutation {
        name: "txn-snapshot-at-stable",
        package: Package::Core,
        breaks: "a snapshot is taken at the stable timestamp, so it sees a commit not yet synced",
        file: "src/txn/clock.rs",
        original: "        stripe.oldest.store(0, Ordering::SeqCst);\n        let snapshot = self.durable.load(Ordering::SeqCst);",
        replacement: "        stripe.oldest.store(0, Ordering::SeqCst);\n        let snapshot = self.stable.load(Ordering::SeqCst);",
        integration: &[],
        unit: &["txn::clock::tests::a_snapshot_waits_for_the_sync"],
    },
    Mutation {
        name: "txn-dependency-not-marked",
        package: Package::Core,
        breaks: "a commit read past the snapshot is not marked, so a reply resting on it is not held",
        file: "src/txn/durable.rs",
        original: "            self.depends.fetch_max(at, Ordering::SeqCst);",
        replacement: "",
        integration: &["commit_visibility::a_reply_waits_for_the_commit_it_wrote_over"],
        unit: &[],
    },
    Mutation {
        name: "txn-stripes-not-waited",
        package: Package::Core,
        breaks: "an exclusive request does not wait for the shared holds taken in stripes",
        file: "src/txn/locks.rs",
        original: "        let fast = if striped { lk.fast.others(tx.id) } else { Vec::new() };",
        replacement: "        let fast: Vec<Holder> = Vec::new();",
        integration: &["row_waits::a_definition_waits_for_holds_in_stripes_and_its_deadlock_is_refused"],
        unit: &[],
    },
];
