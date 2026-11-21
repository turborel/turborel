//! The mutations of the commit path (RULES.md R-TEST-9, D740, D741): group
//! commit's reach and marks, the durable timestamp's waiters woken alone,
//! and a log appender parked behind earlier blocks woken alone.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail the commit path's requirements.
pub const COMMIT_PATH: [Mutation; 5] = [
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
        name: "commit-path-publish-wakes-no-one",
        package: Package::Core,
        breaks: "a block published wakes none of the appenders parked behind it, each then sleeping out its poll",
        file: "src/disk/block.rs",
        original: "        let woken = lock_builtin(&self.parking).remove(&end);",
        replacement: "        let woken = lock_builtin(&self.parking).remove(&u64::MAX);",
        integration: &[],
        unit: &["disk::block::publish::a_publish_wakes_only_the_next_appender"],
    },
    Mutation {
        name: "commit-path-publish-wakes-all",
        package: Package::Core,
        breaks: "a block published wakes every appender parked, not only the one whose block follows",
        file: "src/disk/block.rs",
        original: "        let woken = lock_builtin(&self.parking).remove(&end);",
        replacement: "        let woken = std::mem::take(&mut *lock_builtin(&self.parking)).into_values().next();",
        integration: &[],
        unit: &["disk::block::publish::a_publish_wakes_only_the_next_appender"],
    },
];
