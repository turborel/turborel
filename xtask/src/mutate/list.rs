//! The mutations `cargo xtask mutate` applies (RULES.md R-TEST-9), each breaking one mechanism and naming tests that fail when it is
//! broken. A new mechanism adds one; a change to a quoted snippet updates its mutation (`verify` refuses a stale one).

mod areas;
mod array_targets;
mod assertions;
mod backup;
mod bigint;
mod bounded;
mod call_floor;
mod check_rebind;
mod cli;
mod client;
mod collations;
mod composite;
mod contents;
mod delta_tables;
mod dependencies;
mod diagnostics;
mod distinct;
mod distinct_predicate;
mod distribution;
mod explain;
mod expression_index;
mod fill;
mod filter;
mod fulltext;
mod fulltext_languages;
mod generated;
mod geometry;
mod graph_adjacency;
mod graph_catalog;
mod graph_compat;
mod graph_gql;
mod graph_iterations;
mod graph_native;
mod graph_search;
mod graph_syntax;
mod graph_table;
mod grouping_sets;
mod guarded;
mod hash;
mod homes;
mod hypothetical;
mod information;
mod json;
mod json_index;
mod json_path;
mod json_table;
mod json_type;
mod kept;
mod limits;
mod listagg;
mod lobs;
mod merge;
mod modules;
mod multisets;
mod near;
mod null_ordering;
mod nulls;
mod order_by;
mod overloading;
mod partitioned_joins;
mod partitioning;
mod patterns;
mod plugins;
mod predicates;
mod projection;
mod psm;
mod ptf;
mod readers;
mod reads;
mod regex;
mod rekey;
mod rename;
mod roles;
mod routines;
mod rtree;
mod rtree_costs;
mod rtree_joins;
mod rtree_pack;
mod rtree_windows;
mod savepoints;
mod search_cycle;
mod sequences;
mod server;
mod set_probes;
mod shadow;
mod short_names;
mod simple_features;
mod spatial_curves;
mod spatial_lr;
mod spatial_mutators;
mod spatial_network;
mod spatial_planar;
mod spatial_regions;
mod spatial_routines;
mod spatial_solids;
mod spatial_srs;
mod spatial_support;
mod spatial_topology;
mod spatial_types;
mod sql2023;
mod sqlmm_fulltext;
mod stamps;
mod stats;
mod std_gaps;
mod structured;
mod system_versioned;
mod t621;
mod tailorings;
mod temporal;
mod temporal_references;
mod transport;
mod triggers;
mod truncate;
mod vector;
mod vector_embed;
mod vector_hybrid;
mod vector_indexes;
mod view_defaults;
mod vr;
mod waits;
mod wide_exact;
mod width_bucket;
mod window_distinct;
mod window_nested;
mod window_values;
mod windows;
mod with;
mod xml;
mod xml_composite;
mod xml_index;
mod xml_map;
mod xml_publishing;
mod xml_schema;
mod xml_schema_catalog;
mod xml_scoped;
mod xquery;
mod xquery_eval;
mod xquery_functions;
mod xquery_modules;
mod xquery_statics;
mod zones;

pub use self::{collations::COLLATIONS, fill::FILL, kept::KEPT, readers::READERS, server::SERVER, vr::VR, waits::WAITS, zones::ZONES};
pub use self::{reads::READS, shadow::SHADOW, stats::STATS, tailorings::TAILORINGS};
use super::{Mutation, Package};

/// Every mutation, in the order they run: the core's, then each area's, replication's and the server's last.
pub fn all() -> Vec<Mutation> {
    areas::core().into_iter().chain(areas::all()).chain(areas::late()).flatten().copied().collect()
}

/// The core's mutations: transactions, durability, constraints; assertions' are in `assertions.rs`. Rekeying's are in `rekey.rs`, the hash index's in `hash.rs`.
pub const MUTATIONS: [Mutation; 13] = [
    Mutation {
        name: "mvcc-snapshot",
        package: Package::Core,
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
        package: Package::Core,
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
        package: Package::Core,
        breaks: "the page checksum: a page read is not checked against its digest",
        file: "src/disk/pager/sealed.rs",
        original: "        if stored == Some(id) && verified {",
        replacement: "        if stored == Some(id) {",
        integration: &[
            "pages::torn_corrupt_and_misdirected_pages_are_caught",
            "buffer::failed_write_backs_and_loads_leave_the_pool_consistent",
        ],
        unit: &[],
    },
    Mutation {
        name: "block-checksum",
        package: Package::Core,
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
        package: Package::Core,
        breaks: "the log's sync at commit: COMMIT returns before its record is durable",
        file: "src/txn/logging.rs",
        original: "        let synced = self.shared.sync_in_group(log, lsn, at);",
        replacement: "        let synced = self.shared.taking_changes().map(|()| self.shared.clock.end_syncs(std::iter::once(at)));",
        integration: &["durability::a_commit_is_its_record_made_durable", "durability::a_failed_sync_stops_the_database_taking_changes"],
        unit: &[],
    },
    Mutation {
        name: "file-log-sync",
        package: Package::Core,
        breaks: "the file log's sync: its blocks are counted durable without a sync",
        file: "src/log/file/segments.rs",
        original: "let synced = segment.blocks.sync(durability).map_err(|e| self.failed(e, &format!(\"segment {}\", segment.number)));",
        replacement: "let synced: Result<()> = Ok(());",
        integration: &[
            "failures::a_log_survives_failed_rotations_and_syncs",
            "checkpoints::a_failed_log_sync_stops_checkpoints_until_reopened",
        ],
        unit: &[],
    },
    Mutation {
        name: "not-null",
        package: Package::Core,
        breaks: "NOT NULL: a null is let into a NOT NULL column",
        file: "src/db/integrity.rs",
        original: "if row.get(*at).is_some_and(Value::is_null) {",
        replacement: "if row.get(*at).is_none() {",
        integration: &["statements::insert_enforces_unique_and_not_null_4_5", "cursors::positioned_update_keeps_not_null_8_11"],
        unit: &[],
    },
    Mutation {
        name: "check-unknown",
        package: Package::Core,
        breaks: "CHECK: a row is refused when its condition is unknown, kept when false",
        file: "src/db/integrity.rs",
        original: "if eval_cond(&cond, row, &Heard(tx.stop.warnings()))? == Truth::False {",
        replacement: "if eval_cond(&cond, row, &Heard(tx.stop.warnings()))? == Truth::Unknown {",
        integration: &["constraints::primary_keys_and_checks_11_7_11_9", "domains::a_column_of_a_domain_11_21"],
        unit: &[],
    },
    Mutation {
        name: "foreign-key",
        package: Package::Core,
        breaks: "FOREIGN KEY: a row referencing no row is let in",
        file: "src/db/integrity.rs",
        original: "if !self.matches_row(tx, (desc, fk), row)? {",
        replacement: "if false && !self.matches_row(tx, (desc, fk), row)? {",
        integration: &["constraints::references_are_kept_11_8", "constraints::partial_matches_11_8"],
        unit: &[],
    },
    Mutation {
        name: "unique",
        package: Package::Core,
        breaks: "UNIQUE: two rows of one key are let in",
        file: "src/db/integrity/keys.rs",
        original: "            }\n            if holders > 1 {",
        replacement: "            }\n            if holders > 2 {",
        integration: &[
            "statements::insert_enforces_unique_and_not_null_4_5",
            "unique_checks::update_checks_the_unique_constraints_it_could_break_8_12",
            "mvcc::unique_sees_rows_a_transaction_cannot",
        ],
        unit: &[],
    },
    Mutation {
        name: "lock-wait",
        package: Package::Core,
        breaks: "the lock wait: a transaction set to wait takes a table locked against it",
        file: "src/txn/locks.rs",
        original: "if conflicting.is_empty() {",
        replacement: "if conflicting.is_empty() || tx.lock_wait == LockWait::Wait {",
        integration: &["threads::a_deadlock_is_refused", "cancel::a_wait_for_a_lock_past_its_timeout_is_refused"],
        unit: &[],
    },
    Mutation {
        name: "not-unknown",
        package: Package::Core,
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
    Mutation {
        name: "sort-merge-whole",
        package: Package::Core,
        breaks: "a spilled sort's merge: rows of equal keys are equal, though a string longer than its key may differ past it",
        file: "src/exec/sort/merge.rs",
        original: "if by_keys.is_ne() || !x.truncates {",
        replacement: "if by_keys.is_ne() || x.layout != 0 {",
        integration: &["normal_sorts::normal_sorts_give_what_rows_give"],
        unit: &["exec::sort::merge::tests::merged_by_keys_as_compared_whole"],
    },
];
