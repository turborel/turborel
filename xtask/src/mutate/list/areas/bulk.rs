//! The mutations of rows inserted together (RULES.md R-TEST-9, D530): the
//! rows of a record of rows inserted together not redone by recovery; a
//! key not alone in its unique index left unchecked; rows given a prepared
//! INSERT inserted but the first; the index entries of a batch undone but
//! the newest (D531); a load never ended, nor by a statement no part of
//! it (D532); a load's groups seen by every reader, never undone, or kept
//! by recovery though their transaction did not commit (D533).

use super::super::{Mutation, Package};

/// Mechanisms of rows inserted together whose removal must fail their tests.
pub const BULK: [Mutation; 11] = [
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
    Mutation {
        name: "bulk-group-seen-by-all",
        package: Package::Core,
        breaks: "every reader sees the groups of a load not yet committed",
        file: "src/engine/rows/frozen/stamped.rs",
        original: "        if !directory.pending {\n            return Ok(directory);",
        replacement: "        if directory.pending || !directory.pending {\n            return Ok(directory);",
        integration: &["bulk_groups::a_load_goes_straight_to_groups_its_transaction_alone_sees"],
        unit: &[],
    },
    Mutation {
        name: "bulk-group-not-undone",
        package: Package::Core,
        breaks: "a load's group stays in the directory when its making is undone",
        file: "src/engine/rows/mvcc.rs",
        original: "            return self.unpublish_group(tx, change.id);",
        replacement: "            return Ok(());",
        integration: &["bulk_groups::the_groups_of_a_load_are_undone_with_it"],
        unit: &[],
    },
    Mutation {
        name: "bulk-group-doubtful-kept",
        package: Package::Core,
        breaks: "recovery keeps the groups of a load that never committed",
        file: "src/engine/rows/frozen/stamped.rs",
        original: "            if redone(entry.stamp() & !OPEN) {",
        replacement: "            if redone(entry.stamp() & !OPEN) && entry.stamp() == 0 {",
        integration: &["bulk_groups::a_checkpoint_during_an_open_load_recovers_without_it"],
        unit: &[],
    },
    Mutation {
        name: "bulk-load-rows-untold",
        package: Package::Core,
        breaks: "a copy is never told a load's group rows, so it writes groups of a batch",
        file: "src/txn/undo/loading.rs",
        original: "        Ok(self.state()?.undo.loading.as_ref().and_then(|l| l.grouped))",
        replacement: "        Ok(self.state()?.undo.loading.as_ref().and_then(|_| None))",
        integration: &["bulk_groups::the_groups_of_a_load_are_undone_with_it"],
        unit: &[],
    },
    Mutation {
        name: "bulk-groups-unasked",
        package: Package::Core,
        breaks: "a load into a table that does not qualify for extents writes groups though its table leaves COLUMNSTORE.LOAD unset",
        file: "src/engine/rows/frozen/stamped.rs",
        original: "            LoadTo::Default => qualifies,",
        replacement: "            LoadTo::Default => true,",
        integration: &["bulk_groups::a_load_goes_through_the_tree_unless_asked"],
        unit: &[],
    },
];
