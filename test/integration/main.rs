//! Integration tests: the library seen from outside, through its public API
//! only. One test binary, one module per area of the standard (see RULES.md,
//! "Test layout"). Every test traces to its requirements with a
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
/// The plug-in type of the tests, written once in `support/` (RULES.md
/// R-TEST-10), included here as the core cannot depend on `support`'s
/// build of itself.
#[path = "../../support/src/types.rs"]
mod ident;

mod assertions;
mod batch_joins;
mod batches;
mod bits;
mod blocks;
mod buffer;
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
mod diagnostics;
mod direct;
mod disk;
mod domains;
mod drops;
mod durability;
mod encodings;
mod expressions;
mod extension_types;
mod failures;
mod faults;
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
mod language;
mod log;
mod memory;
mod mvcc;
mod named_schemas;
mod pages;
mod planning;
mod positions;
mod predicates;
mod prepared;
mod privileges;
mod queries;
mod query_expressions;
mod recovery;
mod refusals;
mod reopen;
mod revoke;
mod rowstore;
mod schema_statements;
mod schema_transactions;
mod scroll_cursors;
mod set_statements;
mod settings;
mod spilling;
mod statements;
mod statistics;
mod stores;
mod subqueries;
mod system_tables;
mod table_encodings;
mod table_settings;
mod temporary_tables;
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
mod views;
mod working_tables;
