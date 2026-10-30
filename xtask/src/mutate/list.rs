//! The mutations `cargo xtask mutate` applies (RULES.md R-TEST-9): each
//! breaks one mechanism of the core, and names tests, by their full paths,
//! that each fail when it is broken. A mechanism a change adds gets one
//! here; a change to a quoted snippet brings its mutation up to date
//! (`verify` refuses one that no longer applies).

use super::Mutation;

/// The mutations, in the order they run.
pub const MUTATIONS: [Mutation; 12] = [
    Mutation {
        name: "mvcc-snapshot",
        breaks: "MVCC visibility: a reader sees every committed version, past its snapshot",
        file: "src/engine/rows/mvcc.rs",
        original: "(Reader::At { snapshot, .. }, Maker::Committed(at)) => at <= snapshot,",
        replacement: "(Reader::At { snapshot, .. }, Maker::Committed(at)) => at <= snapshot || true,",
        integration: &[
            "mvcc::readers_see_their_snapshot_and_never_wait",
            "isolation::each_level_reads_what_its_snapshot_holds_4_28",
            "isolation::read_committed_keeps_its_snapshot_while_a_cursor_is_open_14_1",
        ],
        unit: &["engine::rows::zones::tests::zones_skip_what_no_reader_can_match"],
    },
    Mutation {
        name: "mvcc-uncommitted",
        breaks: "MVCC visibility: a reader sees other transactions' uncommitted versions",
        file: "src/engine/rows/mvcc.rs",
        original: "(Reader::At { tx, .. }, Maker::Open(by)) => by == tx,",
        replacement: "(Reader::At { tx, .. }, Maker::Open(by)) => by == tx || true,",
        integration: &[
            "mvcc::readers_see_their_snapshot_and_never_wait",
            "isolation::each_level_reads_what_its_snapshot_holds_4_28",
            "isolation::constraints_are_checked_serializably_at_every_level_4_28",
        ],
        unit: &["engine::rows::tests::an_undone_change_keeps_its_older_version_for_open_readers"],
    },
    Mutation {
        name: "page-checksum",
        breaks: "the page checksum: a page read is not checked against its digest",
        file: "src/disk/pager.rs",
        original: "if stored == Some(id) && self.hash.hash_parts(&[body, &id.to_le_bytes()]).as_bytes() == digest {",
        replacement: "if stored == Some(id) {",
        integration: &[
            "pages::torn_corrupt_and_misdirected_pages_are_caught",
            "buffer::failed_write_backs_and_loads_leave_the_pool_consistent",
        ],
        unit: &[],
    },
    Mutation {
        name: "block-checksum",
        breaks: "the block checksum: a torn or corrupt block of the log is read as valid",
        file: "src/disk/block.rs",
        original: "if !format::verify(&*self.hash, len, digest, out.get(prefix_len..).unwrap_or_default()) {",
        replacement: "if false && !format::verify(&*self.hash, len, digest, out.get(prefix_len..).unwrap_or_default()) {",
        integration: &[
            "blocks::a_torn_or_corrupt_tail_is_cut",
            "blocks::refusing_a_bad_tail_leaves_the_file_unchanged",
            "log::old_segments_sync_and_damage_stops_a_read",
        ],
        unit: &[],
    },
    Mutation {
        name: "commit-sync",
        breaks: "the log's sync at commit: COMMIT returns before its record is durable",
        file: "src/txn/logging.rs",
        original: "Ok(self.shared.sync(&**log, lsn).map_err(|e| {",
        replacement: "Ok(self.shared.taking_changes().map_err(|e| {",
        integration: &["durability::a_commit_is_its_record_made_durable", "durability::a_failed_sync_stops_the_database_taking_changes"],
        unit: &[],
    },
    Mutation {
        name: "file-log-sync",
        breaks: "the file log's sync: its blocks are counted durable without a sync",
        file: "src/log/file/segments.rs",
        original: "let synced = segment.blocks.sync(self.options.durability).map_err(|e| self.failed(e, segment.number));",
        replacement: "let synced: Result<()> = Ok(());",
        integration: &[
            "failures::a_log_survives_failed_rotations_and_syncs",
            "checkpoints::a_failed_log_sync_stops_checkpoints_until_reopened",
        ],
        unit: &[],
    },
    Mutation {
        name: "not-null",
        breaks: "NOT NULL: a null is let into a NOT NULL column",
        file: "src/db/integrity.rs",
        original: "if row.get(*at).is_some_and(Value::is_null) {",
        replacement: "if row.get(*at).is_none() {",
        integration: &["statements::insert_enforces_unique_and_not_null_4_5", "cursors::positioned_update_keeps_not_null_8_11"],
        unit: &[],
    },
    Mutation {
        name: "check-unknown",
        breaks: "CHECK: a row is refused when its condition is unknown, kept when false",
        file: "src/db/integrity.rs",
        original: "if eval_cond(&cond, &row, &Heard(tx.stop.warnings()))? == Truth::False {",
        replacement: "if eval_cond(&cond, &row, &Heard(tx.stop.warnings()))? == Truth::Unknown {",
        integration: &["constraints::primary_keys_and_checks_11_7_11_9", "domains::a_column_of_a_domain_11_21"],
        unit: &[],
    },
    Mutation {
        name: "foreign-key",
        breaks: "FOREIGN KEY: a row referencing no row is let in",
        file: "src/db/integrity.rs",
        original: "if !self.matches(tx, fk, &values)? {",
        replacement: "if false && !self.matches(tx, fk, &values)? {",
        integration: &["constraints::references_are_kept_11_8", "constraints::partial_matches_11_8"],
        unit: &[],
    },
    Mutation {
        name: "unique",
        breaks: "UNIQUE: two rows of one key are let in",
        file: "src/db/assignment.rs",
        original: "if holders > 1 {",
        replacement: "if holders > 2 {",
        integration: &[
            "statements::insert_enforces_unique_and_not_null_4_5",
            "unique_checks::update_checks_the_unique_constraints_it_could_break_8_12",
            "mvcc::unique_sees_rows_a_transaction_cannot",
        ],
        unit: &[],
    },
    Mutation {
        name: "lock-wait",
        breaks: "the lock wait: a transaction set to wait takes a table locked against it",
        file: "src/txn/locks.rs",
        original: "if conflicting.is_empty() {",
        replacement: "if conflicting.is_empty() || tx.lock_wait == LockWait::Wait {",
        integration: &["threads::a_deadlock_is_refused", "cancel::a_wait_for_a_lock_past_its_timeout_is_refused"],
        unit: &[],
    },
    Mutation {
        name: "not-unknown",
        breaks: "three-valued logic: NOT of unknown is true",
        file: "src/exec/eval.rs",
        original: "Unknown => Unknown,",
        replacement: "Unknown => True,",
        integration: &[
            "language::a_comparison_with_null_is_unknown_5_11",
            "predicates::between_is_a_range_5_12",
            "predicates::in_lists_are_any_equal_5_13",
        ],
        unit: &[],
    },
];
