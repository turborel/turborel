//! Integration tests: the library seen from outside, through its public API
//! only. One test binary, one module per area (see RULES.md, "Project
//! layout"). Every test traces to its requirements with a
//! `/// Verifies:` line.
//!
//! A panic is how a test reports failure, so test code may unwrap, index and
//! panic (RULES.md R-ERR-1 applies to production code only).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_in_result,
    clippy::panic_in_result_fn,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    missing_docs,
    reason = "test code: a panic is a test failure (RULES.md R-TEST-4)"
)]
#![allow(
    clippy::default_numeric_fallback,
    clippy::shadow_unrelated,
    clippy::wildcard_enum_match_arm,
    clippy::cognitive_complexity,
    reason = "test code: literal fixtures, rebinding a session after drop, plug-in fixtures that \
              handle one variant, and flat lists of assertions (RULES.md R-TEST-4)"
)]

mod common;
mod concepts;
/// The fixtures over a database and the suites every log passes, written
/// once in `support/` for the tests of the replicated log too (RULES.md
/// R-TEST-10), included so for the same reason.
#[path = "../../support/src/database.rs"]
mod database;
/// The plug-in type of the tests, written once in `support/` (RULES.md
/// R-TEST-10), included here as the core cannot depend on `support`'s
/// build of itself.
#[path = "../../support/src/types.rs"]
mod ident;
#[path = "../../support/src/logs.rs"]
mod logs;

mod assertion_changes;
mod assertion_forms;
mod assertions;
mod batch_joins;
mod batches;
mod bits;
mod blocks;
mod buffer;
mod buffer_claims;
mod cancel;
mod checkpoints;
mod checks;
mod column_store;
mod constraints;
mod contracts;
mod creation;
mod cursors;
mod damage;
mod datetimes;
mod ddl;
mod defaults;
mod deferred;
mod descriptor_types;
mod descriptors;
mod diagnostics;
mod direct;
mod disk;
mod domains;
mod drops;
mod durability;
mod dynamic_cursors;
mod dynamic_manual;
mod dynamic_statements;
mod encodings;
mod engine_settings;
mod expressions;
mod extension_types;
mod failures;
mod faults;
mod file_manager;
mod functions;
mod get_diagnostics;
mod index_contracts;
mod index_settings;
mod indexes;
mod information_schema;
mod intervals;
mod isolation;
mod joined_tables;
mod joins;
mod kept_plans;
mod language;
mod log;
mod log_durability;
mod log_sync;
mod memory;
mod mvcc;
mod named_schemas;
mod ordered_joins;
mod pages;
mod planning;
mod plugin_files;
mod plugin_settings;
mod positions;
mod predicates;
mod prepared;
mod privileges;
mod probes;
mod queries;
mod query_expressions;
mod random_queries;
mod read_ahead;
mod recovery;
mod refusals;
mod reopen;
mod replicated_state;
mod revoke;
mod rowstore;
mod rule_settings;
mod schema_statements;
mod schema_transactions;
mod scroll_cursors;
mod served;
mod set_statements;
mod settings;
mod spill_encodings;
mod spilled_actions;
mod spilled_checks;
mod spilled_rows;
mod spilled_sorts;
mod spilling;
mod statements;
mod statistics;
mod stores;
mod subqueries;
mod sums;
mod system_tables;
mod table_encodings;
mod table_settings;
mod temporary_changes;
mod temporary_spills;
mod temporary_tables;
mod thread_manager;
mod threads;
mod timestamps;
mod transaction_memory;
mod transactions;
mod trees;
mod type_contract;
mod typed_vectors;
mod unique_checks;
mod users;
mod varying;
mod vector_operators;
mod views;
mod warnings;
mod working_tables;
