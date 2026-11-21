//! The mutations of rows inserted together (RULES.md R-TEST-9, D530): the
//! rows of a record of rows inserted together not redone by recovery; a
//! key not alone in its unique index left unchecked; rows given a prepared
//! INSERT inserted but the first; the index entries of a batch undone but
//! the newest (D531); a load never ended, nor by a statement no part of
//! it (D532).

use super::super::{Mutation, Package};

/// Mechanisms of rows inserted together whose removal must fail their tests.
pub const BULK: [Mutation; 6] = [
    Mutation {
        name: "bulk-record-rows-not-redone",
        package: Package::Core,
        breaks: "recovery redoes none of the rows of a record of rows inserted together, so a load's committed batches are lost",
        file: "src/db/recovery.rs",
        original: "                return Ok(rows.into_iter().map(|(row, after)| one(table.clone(), generation, row, None, Some(after))).collect());",
        replacement: "                return Ok(rows.into_iter().map(|(row, after)| one(table.clone(), generation, row, None, Some(after))).take(0).collect());",
        integration: &["bulk_insert::a_load_crashed_midway_keeps_its_committed_batches"],
        unit: &[],
    },
    Mutation {
        name: "bulk-unique-never-noted",
        package: Package::Core,
        breaks: "a row whose key another row of its statement holds is never noted, so the duplicate goes in",
        file: "src/db/integrity/inserted.rs",
        original: "        self.unique && !alone",
        replacement: "        self.unique && !alone && alone",
        integration: &["bulk_insert::rows_inserted_together_are_as_if_inserted_one_at_a_time"],
        unit: &[],
    },
    Mutation {
        name: "bulk-given-rows-first-only",
        package: Package::Core,
        breaks: "of the rows given a prepared INSERT only the first is inserted",
        file: "src/db/write/given.rs",
        original: "    for values in taken(given)? {",
        replacement: "    for values in taken(given)?.into_iter().take(1) {",
        integration: &["bulk_insert::a_prepared_insert_given_many_rows_inserts_each"],
        unit: &[],
    },
    Mutation {
        name: "bulk-undo-rows-newest-only",
        package: Package::Core,
        breaks: "of the index entries of a batch, undone, only the newest is taken out",
        file: "src/txn/undo.rs",
        original: "    entries.iter().rev().map(|(key, id)| index.remove(key, *id)).fold(Ok(()), Result::and)",
        replacement: "    entries.iter().rev().take(1).map(|(key, id)| index.remove(key, *id)).fold(Ok(()), Result::and)",
        integration: &[],
        unit: &["txn::undo::tests::entries_of_rows_inserted_together_spill_and_are_undone_newest_first"],
    },
    Mutation {
        name: "bulk-load-never-ended",
        package: Package::Core,
        breaks: "a load is never ended, so its index lacks the entries it held back",
        file: "src/db/write/bulk/load.rs",
        original: "    if !matches!(load, Some(Load::Alone | Load::Ending)) {",
        replacement: "    if !matches!(load, Some(Load::Alone | Load::Ending)) || load.is_some() {",
        integration: &["bulk_load::a_load_of_many_executions_builds_its_index_as_it_ends"],
        unit: &[],
    },
    Mutation {
        name: "bulk-load-not-ended-by-a-statement",
        package: Package::Core,
        breaks: "a statement no part of a load under way does not end it, so it misses the entries held back",
        file: "src/db/running.rs",
        original: "        if tx.begin_statement(|| !self.cursors_open())? && !continues {",
        replacement: "        if tx.begin_statement(|| !self.cursors_open())? && !continues && continues {",
        integration: &["bulk_load::a_failed_copy_leaves_its_index_empty_and_no_reader_misses_an_entry"],
        unit: &[],
    },
];
