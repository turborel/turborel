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

mod collated_indexes;
mod collation_usage;
mod collations;
mod common;
mod composite_columns;
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

mod array_targets;
mod arrays;
mod assertion_changes;
mod assertion_forms;
mod assertion_queries;
mod assertion_totals;
mod assertions;
mod batch_joins;
mod batches;
mod bigint;
mod binary;
mod bitemporal;
mod blocks;
mod booleans;
mod buffer;
mod buffer_claims;
mod cancel;
mod candidate_keys;
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
mod decfloat;
mod defaults;
mod deferred;
mod dependencies;
mod dependencies_drops;
mod descriptor_lobs;
mod descriptor_nesting;
mod descriptor_types;
mod descriptors;
mod diagnostics;
mod direct;
mod disk;
mod distinct_methods;
mod distinct_predicate;
mod distinct_type_dynamic;
mod distinct_type_information;
mod distinct_type_privileges;
mod distinct_type_routines;
mod distinct_type_values;
mod distinct_types;
mod distribution;
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
mod fetch_bounded;
mod fetch_first;
mod fetch_ties;
mod file_manager;
mod filter;
mod following;
mod formatted_casts;
mod functions;
mod functions_overloading;
mod generated;
mod generated_named;
mod geometry;
mod get_diagnostics;
mod get_diagnostics_items;
mod grouping_sets;
mod held_lobs;
mod hybrid_layout;
mod hypothetical;
mod identity;
mod index_contracts;
mod index_only;
mod index_replay;
mod index_settings;
mod indexes;
mod information_schema;
mod intervals;
mod isolation;
mod joined_tables;
mod joins;
mod kept_plans;
mod language;
mod language_collations;
mod length_units;
mod listagg;
mod lob_composites;
mod lob_functions;
mod lob_locators;
mod lob_predicates;
mod lob_regex;
mod lob_storage;
mod lob_types;
mod log;
mod log_durability;
mod log_sync;
mod memory;
mod merge;
mod merge_concurrency;
mod merge_rows;
mod merge_rules;
mod method_privileges;
mod modules;
mod modules_privileges;
mod multiset_columns;
mod multiset_rules;
mod multisets;
mod mvcc;
mod named_schemas;
mod null_ordering;
mod order_by;
mod ordered_joins;
mod pages;
mod partial_groups;
mod partitioned_joins;
mod period_predicates;
mod periods;
mod planning;
mod plugin_files;
mod plugin_settings;
mod point_plans;
mod portions;
mod positions;
mod predicate_indexes;
mod predicates;
mod prepared;
mod privileges;
mod probes;
mod psm_compound;
mod psm_control;
mod psm_functions;
mod psm_handlers;
mod psm_rules;
mod psm_transactions;
mod ptf_arguments;
mod ptf_basics;
mod ptf_components;
mod ptf_partitions;
mod ptf_rules;
mod ptf_statements;
mod queries;
mod query_expressions;
mod random_queries;
mod read_ahead;
mod recovery;
mod referenceable_views;
mod refusals;
mod regex_functions;
mod regular_expressions;
mod rekeying;
mod renamed_columns;
mod reopen;
mod replicated_state;
mod restrict;
mod revoke;
mod role_views;
mod roles;
mod roles_revoke;
mod roles_session;
mod roles_settle;
mod routines;
mod routines_call;
mod routines_drops;
mod routines_fixed;
mod routines_overloading;
mod routines_privileges;
mod routines_schema;
mod row_pattern_rules;
mod row_pattern_windows;
mod row_patterns;
mod row_waits;
mod rows;
mod rowstore;
mod rule_settings;
mod schema_statements;
mod schema_transactions;
mod scroll_cursors;
mod search_cycle;
mod select_into;
mod sequences;
mod served;
mod set_statements;
mod settings;
mod spatial_index;
mod spill_encodings;
mod spilled_actions;
mod spilled_checks;
mod spilled_rows;
mod spilled_sorts;
mod spilling;
mod standby;
mod statements;
mod statistics;
mod statistics_background;
mod storage_defaults;
mod stores;
mod structured_alterations;
mod structured_ddl;
mod structured_ddl_rules;
mod structured_information_schema;
mod structured_orderings;
mod structured_semantics;
mod structured_type_privileges;
mod structured_values;
mod subqueries;
mod sums;
mod system_tables;
mod system_time_queries;
mod system_versioned;
mod table_contents;
mod table_encodings;
mod table_settings;
mod temporal_keys;
mod temporal_references;
mod temporary_changes;
mod temporary_lobs;
mod temporary_spills;
mod temporary_tables;
mod thread_manager;
mod threads;
mod timestamps;
mod transaction_memory;
mod transactions;
mod trees;
mod trigger_views;
mod triggers;
mod triggers_defaults;
mod triggers_drops;
mod triggers_firing;
mod triggers_instead;
mod triggers_privileges;
mod triggers_routed;
mod truncate;
mod type_contract;
mod typed_families;
mod typed_tables;
mod typed_vectors;
mod unique_checks;
mod unique_nulls;
mod unnest;
mod user_casts;
mod users;
mod varying;
mod vector_operators;
mod views;
mod warnings;
mod width_bucket;
mod window_distinct;
mod window_nested;
mod window_nested_rules;
mod window_rules;
mod window_value_rules;
mod window_values;
mod windows;
mod with_queries;
mod with_strata;
mod working_tables;
mod xml;
mod xml_cast;
mod xml_composite;
mod xml_elements;
mod xml_information;
mod xml_operators;
mod xml_publishing;
mod xml_storage;
