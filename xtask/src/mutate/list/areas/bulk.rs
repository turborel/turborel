//! The mutations of rows inserted together (RULES.md R-TEST-9, D530): the
//! rows of a record of rows inserted together not redone by recovery; a
//! key not alone in its unique index left unchecked; rows given a prepared
//! INSERT inserted but the first.

use super::super::{Mutation, Package};

/// Mechanisms of rows inserted together whose removal must fail their tests.
pub const BULK: [Mutation; 3] = [
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
        original: "        self.unique.iter().any(|ix| !tx.alone(ix, id))",
        replacement: "        self.unique.iter().any(|ix| !tx.alone(ix, id) && id == 0)",
        integration: &["bulk_insert::rows_inserted_together_are_as_if_inserted_one_at_a_time"],
        unit: &[],
    },
    Mutation {
        name: "bulk-given-rows-first-only",
        package: Package::Core,
        breaks: "of the rows given a prepared INSERT only the first is inserted",
        file: "src/db/write/given.rs",
        original: "    for values in given {",
        replacement: "    for values in given.iter().take(1) {",
        integration: &["bulk_insert::a_prepared_insert_given_many_rows_inserts_each"],
        unit: &[],
    },
];
