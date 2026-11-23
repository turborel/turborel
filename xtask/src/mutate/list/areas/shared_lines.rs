//! The mutations of what keeps transactions off each other's cache lines
//! (RULES.md R-TEST-9, D782-D784): a value alone on its lines, a reader's
//! commit taking no lock of the statuses' map, and a table's open
//! transactions in shards; a transaction's id on its thread's stripe, and
//! its session's kept origin replaced as its setting changes (D844); and a
//! commit taking a table's shared lists only for what it left there (D870).

use super::super::{Mutation, Package};

/// The open transactions' file.
const OPENS: &str = "src/engine/rows/mvcc/opens.rs";

/// The test of a transaction that left nothing in a table.
const NOTHING_LEFT: &str = "engine::rows::mvcc::stamps::tests::a_transaction_that_left_nothing_takes_no_lock_of_the_table";

/// The shared lines' mutations, in the order they run.
pub const SHARED_LINES: [Mutation; 9] = [
    Mutation {
        name: "alone-unaligned",
        package: Package::Core,
        breaks: "values meant to be alone share their cache lines with their neighbours",
        file: "src/sync/alone.rs",
        original: "#[repr(align(128))]\n",
        replacement: "",
        integration: &[],
        unit: &["sync::alone::tests::values_alone_share_no_line"],
    },
    Mutation {
        name: "status-reader-locks-map",
        package: Package::Core,
        breaks: "every reader's commit takes the statuses' map for writing, a line every commit of the store writes",
        file: "src/engine/rows/mvcc/status.rs",
        original: "            return;\n        }\n        if self.overflowing.load(Ordering::SeqCst) == 0 {\n            return;\n        }\n",
        replacement: "            return;\n        }\n",
        integration: &[],
        unit: &["engine::rows::mvcc::status::tests::a_readers_commit_takes_no_lock"],
    },
    Mutation {
        name: "opens-one-shard",
        package: Package::Core,
        breaks: "a table's open transactions all under one lock, every row read writing its count of readers",
        file: OPENS,
        original: "const SHARDS: u64 = 32;",
        replacement: "const SHARDS: u64 = 1;",
        integration: &[],
        unit: &["engine::rows::mvcc::opens::tests::open_transactions_are_kept_in_shards_of_their_own"],
    },
    Mutation {
        name: "opens-count-unkept",
        package: Package::Core,
        breaks: "a table counted with no transaction open while one is, its versions undone released under a reader",
        file: OPENS,
        original: "        map.entry(tx).or_insert_with(|| Arc::new(Mutex::new(made())));\n        shard.len.store(map.len(), Ordering::SeqCst);",
        replacement: "        map.entry(tx).or_insert_with(|| Arc::new(Mutex::new(made())));",
        integration: &[],
        unit: &["engine::rows::mvcc::opens::tests::open_transactions_are_kept_in_shards_of_their_own"],
    },
    Mutation {
        name: "ids-off-their-stripe",
        package: Package::Core,
        breaks: "a transaction's id is not on its thread's stripe, so its registries by id are on lines other cores write (D844)",
        file: "src/txn/clock.rs",
        original: ".saturating_add(u64::try_from(thread_stripe(ID_STRIPES)).unwrap_or_default());",
        replacement: ".saturating_add(u64::try_from(thread_stripe(ID_STRIPES)).unwrap_or_default().min(0));",
        integration: &[],
        unit: &["txn::clock::tests::ids_go_on_in_order_each_on_its_threads_stripe"],
    },
    Mutation {
        name: "origin-kept-past-its-setting",
        package: Package::Core,
        breaks: "a session's kept origin outlives its MEMORY.WORK_BYTES, so its undo spills as the setting no longer says (D844)",
        file: "src/db/state.rs",
        original: "            (s.limit, s.batch) == (limit, batch) && areas && s.types.same(&self.types)",
        replacement: "            areas && s.types.same(&self.types)",
        integration: &["transaction_memory::a_sessions_work_memory_is_its_next_transactions"],
        unit: &[],
    },
    Mutation {
        name: "validated-kept-for-nothing",
        package: Package::Core,
        breaks: "a transaction that neither changed nor read a table takes the lock of its validated transactions and is kept among them (D870)",
        file: "src/engine/rows/mvcc/commit.rs",
        original: "            if open.changes.is_empty() && !open.reads.any() {\n                return Ok(());\n            }\n",
        replacement: "",
        integration: &[],
        unit: &[NOTHING_LEFT],
    },
    Mutation {
        name: "settle-locks-an-empty-queue",
        package: Package::Core,
        breaks: "a settle of a table with no commit queued takes the queue's lock, which every commit of the table takes (D870)",
        file: "src/engine/rows/mvcc/stamps.rs",
        original: "        if self.versions.queued.load(Ordering::SeqCst) == 0 {\n            return;\n        }\n",
        replacement: "",
        integration: &[],
        unit: &[NOTHING_LEFT],
    },
    Mutation {
        name: "small-lists-take-work-areas",
        package: Package::Core,
        breaks: "a commit within the bytes held takes the store's work areas for lists that never spill (D870)",
        file: "src/engine/rows/mvcc.rs",
        original: "if bytes <= self.held() { ListSpace::default() } else { self.space() }",
        replacement: "if bytes < self.held().min(0) { ListSpace::default() } else { self.space() }",
        integration: &[],
        unit: &["engine::rows::mvcc::stamps::tests::a_commit_within_the_bytes_held_takes_no_work_areas"],
    },
];
