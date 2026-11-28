//! The mutations of a morsel's rows handed on ungrouped and written once
//! by partition (RULES.md R-TEST-9, D1313): each row's place among its
//! partition's, the keys and first rows written there, the rows fed to
//! the groups at their places, the hash of a key's lanes, and a table
//! handed over as it lies only by the partitions it was written by.

use super::super::{Mutation, Package};

/// Where the rows are placed and the table handed over.
const APART_FILE: &str = "src/exec/table/made/apart.rs";

/// Where the key lanes are written.
const KEYS_FILE: &str = "src/exec/table/keys/placed.rs";

/// Rows written by partition are the rows appended and gathered.
const WRITTEN: &str = "exec::table::made::apart::tests::rows_written_by_partition_are_the_rows_appended_and_gathered";

/// A table written by other partitions is gathered again.
const GATHERED: &str = "exec::table::made::apart::tests::a_table_written_by_other_partitions_is_gathered_again";

/// Mechanisms whose removal must fail the requirements of rows written
/// by partition.
pub const PLACED_ROWS: [Mutation; 9] = [
    Mutation {
        name: "placed-rows-one-place-a-partition",
        package: Package::Core,
        breaks: "every row of a partition is given the partition's first place, so each overwrites the one before (RULES.md D1313)",
        file: APART_FILE,
        original: "        let place = *at;\n        *at = at.saturating_add(1);",
        replacement: "        let place = *at;",
        integration: &[],
        unit: &[WRITTEN],
    },
    Mutation {
        name: "placed-rows-first-rows-one-number",
        package: Package::Core,
        breaks: "every row handed on ungrouped is numbered as the morsel's first, so no group's first row is its own and their order is lost (RULES.md D1260, D1313)",
        file: APART_FILE,
        original: "scattered((0_u64..).map(|n| first.saturating_add(n)), &places)",
        replacement: "scattered((0_u64..).map(|_| first), &places)",
        integration: &[],
        unit: &[WRITTEN],
    },
    Mutation {
        name: "placed-rows-handed-as-they-lie-by-other-lanes",
        package: Package::Core,
        breaks: "a table written by whole keys is handed over as it lies when asked by a lane, its starts those of other partitions (RULES.md D1313)",
        file: APART_FILE,
        original: "(placed.starts.len() == parts.saturating_add(1) && placed.lanes == lanes).then_some(placed.starts)",
        replacement: "(placed.starts.len() == parts.saturating_add(1)).then_some(placed.starts)",
        integration: &[],
        unit: &[GATHERED],
    },
    Mutation {
        name: "placed-rows-handed-as-they-lie-by-another-count",
        package: Package::Core,
        breaks: "a table written by 8 partitions is handed over as it lies when asked for 16 (RULES.md D1313)",
        file: APART_FILE,
        original: "(placed.starts.len() == parts.saturating_add(1) && placed.lanes == lanes).then_some(placed.starts)",
        replacement: "(placed.lanes == lanes).then_some(placed.starts)",
        integration: &[],
        unit: &[GATHERED],
    },
    Mutation {
        name: "placed-rows-fed-from-the-first",
        package: Package::Core,
        breaks: "the rows of every batch are fed to the groups of the morsel's first rows (RULES.md D1313)",
        file: APART_FILE,
        original: "        placed.fed = from.saturating_add(count);\n",
        replacement: "        placed.fed = from;\n",
        integration: &[],
        unit: &["exec::table::made::apart::tests::rows_taken_of_a_batch_are_fed_at_their_places"],
    },
    Mutation {
        name: "placed-rows-nulls-not-noted",
        package: Package::Core,
        breaks: "a null key of integers is written as a number, so nulls group with it (RULES.md D1313)",
        file: KEYS_FILE,
        original: "                    *at = null;",
        replacement: "                    *at = false;",
        integration: &[],
        unit: &[WRITTEN],
    },
    Mutation {
        name: "placed-rows-lane-of-nulls-told-to-hold-none",
        package: Package::Core,
        breaks: "a lane of integers written by partition that holds nulls says it holds none, so a null reads as the number 0 (RULES.md D1313, D1274)",
        file: KEYS_FILE,
        original: "Lane { data, nulls, nulled, room: places.len(), ..Lane::default() }",
        replacement: "Lane { data, nulls, room: places.len(), ..Lane::default() }",
        integration: &[],
        unit: &[WRITTEN],
    },
    Mutation {
        name: "placed-rows-values-of-the-first-batch",
        package: Package::Core,
        breaks: "a key that is no integer is read of the morsel's first batch whichever batch its row is of (RULES.md D1313)",
        file: KEYS_FILE,
        original: "                *at = (b, u32::try_from(nth(rows, n)).unwrap_or(u32::MAX));",
        replacement: "                *at = (0, u32::try_from(nth(rows, n)).unwrap_or(u32::MAX));",
        integration: &[],
        unit: &[WRITTEN],
    },
    Mutation {
        name: "placed-rows-lanes-hashed-as-the-first-column",
        package: Package::Core,
        breaks: "a lane of a key is hashed as the key's first column is, padded where its own is not, so a row's partition is not its group's (RULES.md D1313, D1183)",
        file: "src/exec/table/hash.rs",
        original: "at.get(lane).map(|&c| (lane, c))",
        replacement: "at.get(lane).map(|&c| (0, c))",
        integration: &[],
        unit: &[WRITTEN],
    },
];
