//! The mutations of the read-only path (RULES.md R-TEST-9, D29): what a
//! transaction that wrote nothing collects as it ends, when maintenance
//! is due, and the shared bookkeeping every transaction takes and gives
//! back, its logged first and what cancels its statements.

use super::{Mutation, Package};

/// The mutations of the read-only path, in the order they run.
pub const READERS: [Mutation; 5] = [
    Mutation {
        name: "reader-collects-nothing",
        package: Package::Core,
        breaks: "read-only path: a transaction that wrote nothing leaves the tables awaiting collection to the next writer",
        file: "src/txn/end.rs",
        original: "        if self.tx.awaiting.load(Ordering::Acquire) {",
        replacement: "        if false {",
        integration: &["mvcc::an_idle_tables_garbage_is_collected_once_readers_end"],
        unit: &[],
    },
    Mutation {
        name: "reader-awaiting-unset",
        package: Package::Core,
        breaks: "read-only path: the flag of tables awaiting collection never set, so no reader collects them",
        file: "src/txn/end.rs",
        original: "            self.tx.awaiting.store(!uncollected.is_empty(), Ordering::Release);",
        replacement: "            self.tx.awaiting.store(false, Ordering::Release);",
        integration: &["mvcc::an_idle_tables_garbage_is_collected_once_readers_end"],
        unit: &[],
    },
    Mutation {
        name: "first-kept-by-writer",
        package: Package::Core,
        breaks: "read-only path: a transaction that logged a record keeps its first after it ends, holding the log",
        file: "src/txn.rs",
        original: "        if self.state.get_mut().map_or(true, |st| st.logged) {",
        replacement: "        if self.state.get_mut().map_or(true, |st| !st.logged) {",
        integration: &[],
        unit: &["txn::tests::a_transactions_first_record_and_stop_last_until_it_ends"],
    },
    Mutation {
        name: "stop-kept-after-end",
        package: Package::Core,
        breaks: "read-only path: what cancels an ended transaction's statements kept in its shard",
        file: "src/txn/stop.rs",
        original: "            lock_builtin(shard).remove(&tx);",
        replacement: "            drop(lock_builtin(shard));",
        integration: &[],
        unit: &["txn::tests::a_transactions_first_record_and_stop_last_until_it_ends"],
    },
    Mutation {
        name: "maintenance-due-after-take",
        package: Package::Core,
        breaks: "read-only path: maintenance looked at as due after the tables noted were taken",
        file: "src/txn/end.rs",
        original: "        self.noted.store(false, Ordering::Release);",
        replacement: "        let _ = &self.noted;",
        integration: &[],
        unit: &["txn::tests::maintenance_is_due_from_a_note_until_taken"],
    },
];
