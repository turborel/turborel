//! The mutations of MERGE (RULES.md R-TEST-9): a target row of two
//! matching rows taken, the last applicable clause taking a row instead of
//! the first, the WHEN NOT MATCHED condition read otherwise than as
//! printed, no data of a source of rows, ROW_COUNT of the pairs alone or of
//! every joined row whatever the clauses, rows outside a view WITH CHECK
//! OPTION written, a row another transaction changed written though it no
//! longer matches, a joined table as the source, the source exposing the
//! target's name, a value count unlike the column count refused as a format
//! error, the target in scope in the insert values, and the READ ONLY
//! transaction's state lost or a temporary target refused by it; and a
//! located read's rows, of a scan or of an index join, given without their
//! ids.
//!
//! A SET value computed as its row is written, not before, is no mutation.
//! The statement's reads never see its own writes, its subqueries' too, so
//! the values are the same either way.

use super::{Mutation, Package};

/// The file of what a MERGE does to each row, decided.
const DECIDE: &str = "src/db/merge/decide.rs";
/// The file of what a MERGE decided, done.
const APPLY: &str = "src/db/merge/apply.rs";
/// The file of a MERGE's source, bound.
const SOURCE: &str = "src/binder/merge/source.rs";
/// The file of a MERGE's WHEN clauses, bound.
const CLAUSES: &str = "src/binder/merge/clauses.rs";
/// The test of two source rows for one target row.
const CARDINALITY: &str = "merge::two_source_rows_for_one_target_row_raise_21000_14_12";
/// The test of the first applicable clause.
const FIRST: &str = "merge::the_first_applicable_clause_takes_the_row_14_12";
/// The test of the rows past the work area.
const LARGE: &str = "merge_rows::a_large_merge_past_the_work_area_14_12";
/// The test of the source's Syntax Rules.
const SOURCE_RULES: &str = "merge_rules::the_source_and_its_scope_refused_by_their_syntax_rules_14_12";
/// The test of a view WITH CHECK OPTION.
const CHECKED: &str = "merge_rules::check_option_refuses_rows_outside_the_view_14_12";
/// The test of a row another transaction changed.
const AGAIN: &str = "merge_concurrency::a_row_changed_meanwhile_is_checked_again_14_12";
/// The test of the READ ONLY transaction.
const READ_ONLY: &str = "merge_rules::a_read_only_transaction_merges_into_temporary_tables_alone_14_12";

/// The mutations of MERGE, in the order they run.
pub const MERGE: [Mutation; 19] = [
    Mutation {
        name: "merge-cardinality-unchecked",
        package: Package::Core,
        breaks: "a target row two source rows match for its clause is updated or deleted by one, not refused with 21000",
        file: DECIDE,
        original: "if self.matching > 1 {",
        replacement: "if false {",
        integration: &[CARDINALITY, LARGE],
        unit: &[],
    },
    Mutation {
        name: "merge-last-matched-clause-wins",
        package: Package::Core,
        breaks: "of the WHEN MATCHED clauses whose conditions are true of a row, the last takes it",
        file: DECIDE,
        original: "for (j, clause) in merge.bound.clauses.iter().enumerate() {",
        replacement: "for (j, clause) in merge.bound.clauses.iter().enumerate().rev() {",
        integration: &[FIRST],
        unit: &[],
    },
    Mutation {
        name: "merge-last-not-matched-clause-wins",
        package: Package::Core,
        breaks: "of the WHEN NOT MATCHED clauses that apply to a source row, the last inserts it",
        file: DECIDE,
        original: "clauses.iter().position(|clause| match clause {",
        replacement: "clauses.iter().rposition(|clause| match clause {",
        integration: &[FIRST],
        unit: &[],
    },
    Mutation {
        name: "merge-not-matched-flag-inverted",
        package: Package::Core,
        breaks: "a source row is inserted when no target row makes ON OR c true, not ON OR NOT c as GR6a ii 1 prints it",
        file: "src/binder/merge/rows.rs",
        original: "let either = Cond::Or(Box::new(sc1.clone()), Box::new(Cond::Not(Box::new(scj))));",
        replacement: "let either = Cond::Or(Box::new(sc1.clone()), Box::new(scj));",
        integration: &["merge::the_not_matched_condition_as_printed_14_12"],
        unit: &[],
    },
    Mutation {
        name: "merge-no-data-of-rows",
        package: Package::Core,
        breaks: "a MERGE whose source has rows but changes none is no data, not only one of an empty source (GR8)",
        file: "src/db/merge.rs",
        original: "Ok(if m.read { Completion::Success } else { completion })",
        replacement: "Ok(if m.read { completion } else { completion })",
        integration: &["merge::an_empty_source_alone_is_no_data_14_12"],
        unit: &[],
    },
    Mutation {
        name: "merge-row-count-pairs-of-both",
        package: Package::Core,
        breaks: "ROW_COUNT of a MERGE of both kinds of clause counts the pairs ON matches, not every row of the RIGHT OUTER JOIN (§23.1 GR2f ii 3)",
        file: DECIDE,
        original: "(true, true) | (false, false) => self.rows,",
        replacement: "(true, true) | (false, false) => self.matched,",
        integration: &["merge::diagnostics_after_merge_23_1", "merge_rows::the_same_rows_by_batches_layouts_and_indexes_14_12"],
        unit: &[],
    },
    Mutation {
        name: "merge-row-count-unmatched-all",
        package: Package::Core,
        breaks: "ROW_COUNT of a MERGE of WHEN NOT MATCHED clauses alone counts every joined row, not the source rows ON matches to none (§23.1 GR2f ii 2)",
        file: DECIDE,
        original: "(false, true) => self.rows.saturating_sub(self.matched),",
        replacement: "(false, true) => self.rows,",
        integration: &["merge::diagnostics_after_merge_23_1"],
        unit: &[],
    },
    Mutation {
        name: "merge-check-option-updates-unchecked",
        package: Package::Core,
        breaks: "a row a MERGE updates out of a view WITH CHECK OPTION is written, not refused with 44000",
        file: APPLY,
        original: "                if checks.is_empty() {",
        replacement: "                if true {",
        integration: &[CHECKED],
        unit: &[],
    },
    Mutation {
        name: "merge-check-option-inserts-unchecked",
        package: Package::Core,
        breaks: "a row a MERGE inserts out of a view WITH CHECK OPTION is written, not refused with 44000",
        file: APPLY,
        original: "if !target.checks().is_empty() {",
        replacement: "if false {",
        integration: &[CHECKED],
        unit: &[],
    },
    Mutation {
        name: "merge-recheck-on-skipped",
        package: Package::Core,
        breaks: "at READ COMMITTED a target row another transaction changed is written though ON no longer matches it",
        file: APPLY,
        original: "if !holds(&merge.bound.on)? {",
        replacement: "if false {",
        integration: &[AGAIN],
        unit: &[],
    },
    Mutation {
        name: "merge-recheck-condition-skipped",
        package: Package::Core,
        breaks: "at READ COMMITTED a target row another transaction changed is written though its clause's condition no longer holds",
        file: APPLY,
        original: "Some(c) => holds(c)?,",
        replacement: "Some(_) => true,",
        integration: &[AGAIN],
        unit: &[],
    },
    Mutation {
        name: "merge-joined-source",
        package: Package::Core,
        breaks: "a joined table is taken as the source of a MERGE, against SR11",
        file: SOURCE,
        original: "if let TableReference::Joined(j) = tr {",
        replacement: "if let (TableReference::Joined(j), true) = (tr, false) {",
        integration: &[SOURCE_RULES],
        unit: &[],
    },
    Mutation {
        name: "merge-same-exposed-name",
        package: Package::Core,
        breaks: "a source exposing the name the target is exposed by is taken, against SR12",
        file: SOURCE,
        original: "if let Some(same) = items.iter().find(|it| same_exposed(target, it)) {",
        replacement: "if let Some(same) = items.iter().find(|it| same_exposed(target, it) && false) {",
        integration: &[SOURCE_RULES],
        unit: &[],
    },
    Mutation {
        name: "merge-value-count-syntax",
        package: Package::Core,
        breaks: "an insert value list of more or fewer values than its columns is a format error (-102), not SR18d's (-103)",
        file: CLAUSES,
        original: "rule(msg).or_at(row.at)",
        replacement: "SqlError::new(Code::Syntax, msg).or_at(row.at)",
        integration: &["merge_rules::the_clauses_refused_by_their_rules_14_12"],
        unit: &[],
    },
    Mutation {
        name: "merge-values-see-target",
        package: Package::Core,
        breaks: "the target's name is in scope in the insert values, against SR9",
        file: "src/binder/merge.rs",
        original: "source: &alone",
        replacement: "source: &both",
        integration: &[SOURCE_RULES],
        unit: &[],
    },
    Mutation {
        name: "merge-read-only-allowed",
        package: Package::Core,
        breaks: "a READ ONLY transaction merges into a persistent table, not refused with 25006",
        file: "src/db/session.rs",
        original: "SqlStatement::Merge(merge) => (!temporary(&merge.target), \"§14.12 GR1\"),",
        replacement: "SqlStatement::Merge(merge) => (!temporary(&merge.target) && false, \"§14.12 GR1\"),",
        integration: &[READ_ONLY],
        unit: &[],
    },
    Mutation {
        name: "merge-temporary-refused",
        package: Package::Core,
        breaks: "a READ ONLY transaction refuses a MERGE into a temporary table, which GR1 allows",
        file: "src/db/session.rs",
        original: "SqlStatement::Merge(merge) => (!temporary(&merge.target), \"§14.12 GR1\"),",
        replacement: "SqlStatement::Merge(merge) => (!temporary(&merge.target) || true, \"§14.12 GR1\"),",
        integration: &[READ_ONLY],
        unit: &[],
    },
    Mutation {
        name: "located-scan-without-ids",
        package: Package::Core,
        breaks: "a located scan gives its rows without their ids, so a MERGE finds no target row",
        file: "src/exec/scan.rs",
        original: "    if located {\n        row.push(storage::located(id));",
        replacement: "    if false {\n        row.push(storage::located(id));",
        integration: &[FIRST],
        unit: &[
            "exec::scan::tests::a_located_scan_follows_each_row_with_its_id",
            "exec::scan::tests::ordered_filtered_and_projected_located_scans_keep_their_ids",
            "exec::scan::tests::a_located_index_scan_follows_its_row_with_its_id",
        ],
    },
    Mutation {
        name: "located-index-join-without-ids",
        package: Package::Core,
        breaks: "a located LEFT index join gives the rows it finds without their ids",
        file: "src/exec/join/indexed.rs",
        original: "            if self.located {",
        replacement: "            if false {",
        integration: &[],
        unit: &["exec::scan::tests::a_located_left_index_join_gives_ids_and_nulls"],
    },
];
