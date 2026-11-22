//! The mutations of the commit path (RULES.md R-TEST-9, D740, D795, D796):
//! group commit's reach and marks, the durable timestamp's waiters woken
//! alone, the shared collection state an end leaves as it is, and blocks
//! filed after a publish published by it.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail the commit path's requirements.
pub const COMMIT_PATH: [Mutation; 8] = [
    Mutation {
        name: "commit-path-group-claims-past-its-sync",
        package: Package::Core,
        breaks: "a group's leader claims every commit queued durable, those whose records its sync never reached among them",
        file: "src/txn/group.rs",
        original: ".partition(|q| same(&q.log, &log) && q.lsn < reach);",
        replacement: ".partition(|q| same(&q.log, &log));",
        integration: &[],
        unit: &["txn::group::tests::commits_that_come_during_a_sync_share_the_next"],
    },
    Mutation {
        name: "commit-path-group-marks-nothing",
        package: Package::Core,
        breaks: "a group's sync marks none of its commits durable, so the durable timestamp never moves past them",
        file: "src/txn/group.rs",
        original: "            self.clock.end_syncs(covered.iter().map(|q| q.at));\n",
        replacement: "",
        integration: &[],
        unit: &["txn::group::tests::commits_that_come_during_a_sync_share_the_next"],
    },
    Mutation {
        name: "commit-path-durable-wakes-no-one",
        package: Package::Core,
        breaks: "the durable timestamp moves and wakes none of its waiters, each then sleeping out its poll",
        file: "src/txn/clock.rs",
        original: "        if moved && self.waiting.load(Ordering::SeqCst) > 0 {",
        replacement: "        if false && moved && self.waiting.load(Ordering::SeqCst) > 0 {",
        integration: &[],
        unit: &["txn::clock::tests::a_waiter_is_woken_once_its_timestamp_is_durable"],
    },
    Mutation {
        name: "commit-path-publish-files-nothing",
        package: Package::Core,
        breaks: "a publish leaves the blocks filed after it unpublished, each appender waiting out its poll and none publishing it (D796)",
        file: "src/disk/block.rs",
        original: "            while let Some((end, thread)) = parking.remove(&at) {",
        replacement: "            while let Some((end, thread)) = parking.remove(&u64::MAX) {",
        integration: &[],
        unit: &["disk::block::publish::a_publish_publishes_the_filed_blocks_that_follow_it"],
    },
    Mutation {
        name: "commit-path-publish-settles-nothing",
        package: Package::Core,
        breaks: "a publish never looks for blocks filed after it (D796)",
        file: "src/disk/block.rs",
        original: "        if self.parked.load(Ordering::SeqCst) == 0 {\n            return;\n        }\n        let mut woken",
        replacement: "        if self.parked.load(Ordering::SeqCst) < usize::MAX {\n            return;\n        }\n        let mut woken",
        integration: &[],
        unit: &["disk::block::publish::a_publish_publishes_the_filed_blocks_that_follow_it"],
    },
    Mutation {
        name: "commit-path-noted-never-filed",
        package: Package::Core,
        breaks: "a table not yet due is never filed as awaiting collection, so no later end collects it (D795)",
        file: "src/txn/end.rs",
        original: "(Some(true), Some(true)) | (None, Some(false))",
        replacement: "(Some(true), _) | (None, Some(false))",
        integration: &["collection::a_busy_table_is_collected_a_batch_of_commits_at_a_time"],
        unit: &[],
    },
    Mutation {
        name: "commit-path-open-count-unread",
        package: Package::Core,
        breaks: "a table with no transaction open is not found so, and its garbage waits for a batch of commits (D795)",
        file: "src/engine/rows/mvcc/gc.rs",
        original: "            || self.versions.opened.load(Ordering::Relaxed) == 0",
        replacement: "            || self.versions.opened.load(Ordering::Relaxed) == usize::MAX",
        integration: &["collection::a_session_collects_once_its_reply_is_sent"],
        unit: &[],
    },
    Mutation {
        name: "commit-path-awaiting-never-stored",
        package: Package::Core,
        breaks: "whether tables await collection is never stored, so an end that wrote nothing never collects them (D795)",
        file: "src/txn/end.rs",
        original: "        if self.tx.awaiting.load(Ordering::Relaxed) != now {",
        replacement: "        if false && self.tx.awaiting.load(Ordering::Relaxed) != now {",
        integration: &["collection::a_session_collects_once_its_reply_is_sent"],
        unit: &[],
    },
];
