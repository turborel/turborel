//! The mutations of what keeps transactions off each other's cache lines
//! (RULES.md R-TEST-9, D782-D784): a value alone on its lines, a reader's
//! commit taking no lock of the statuses' map, and a table's open
//! transactions in shards; a transaction's id on its thread's stripe, and
//! its session's kept origin replaced as its setting changes (D840).

use super::super::{Mutation, Package};

/// The open transactions' file.
const OPENS: &str = "src/engine/rows/mvcc/opens.rs";

/// The shared lines' mutations, in the order they run.
pub const SHARED_LINES: [Mutation; 6] = [
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
        breaks: "a transaction's id is not on its thread's stripe, so its registries by id are on lines other cores write (D840)",
        file: "src/txn/clock.rs",
        original: ".saturating_add(u64::try_from(thread_stripe(ID_STRIPES)).unwrap_or_default());",
        replacement: ".saturating_add(u64::try_from(thread_stripe(ID_STRIPES)).unwrap_or_default().min(0));",
        integration: &[],
        unit: &["txn::clock::tests::ids_go_on_in_order_each_on_its_threads_stripe"],
    },
    Mutation {
        name: "origin-kept-past-its-setting",
        package: Package::Core,
        breaks: "a session's kept origin outlives its MEMORY.WORK_BYTES, so its undo spills as the setting no longer says (D840)",
        file: "src/db/state.rs",
        original: "            (s.limit, s.batch) == (limit, batch) && areas && s.types.same(&self.types)",
        replacement: "            areas && s.types.same(&self.types)",
        integration: &["transaction_memory::a_sessions_work_memory_is_its_next_transactions"],
        unit: &[],
    },
];
