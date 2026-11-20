//! The mutations of the executor's table of groups (RULES.md R-TEST-9,
//! D356, D357): texts equal without their trailing spaces, exact numbers
//! hashed alike at any scale, the keyed hash past the probe bound, a
//! table kept to its first groups, the columns of states fed as their
//! accumulators, and a grouping run in parallel numbered and ordered by
//! its groups' first rows.

use super::super::{Mutation, Package};

/// The groupings by batch of every type, in the order a row at a time
/// gives them, serial and in parallel.
const SAME_ROWS: &str = "parallel::groups_of_every_type_come_as_a_row_at_a_time_gives";

/// The columns of states against their accumulators.
const STATES: &str = "exec::table::states::tests::columns_of_states_are_their_accumulators";

/// Mechanisms whose removal must fail the table's requirements.
pub const GROUP_TABLE: [Mutation; 8] = [
    Mutation {
        name: "group-table-texts-untrimmed",
        package: Package::Core,
        breaks: "a lane's texts are equal without their trailing spaces",
        file: "src/exec/table/keys.rs",
        original: "    text.trim_end_matches(' ')",
        replacement: "    text",
        integration: &[],
        unit: &["exec::table::keys::tests::lanes_equal_as_their_keys_bytes"],
    },
    Mutation {
        name: "group-table-scales-hashed-apart",
        package: Package::Core,
        breaks: "an exact number is hashed by its value, whatever its scale",
        file: "src/exec/table/hash.rs",
        original: "        if let (0, Ok(integer)) = (scale, i64::try_from(units)) {",
        replacement: "        if let (_, Ok(integer)) = (scale, i64::try_from(units)) {",
        integration: &[],
        unit: &["exec::table::tests::keys_hash_alike_in_any_vector"],
    },
    Mutation {
        name: "group-table-never-keyed",
        package: Package::Core,
        breaks: "a probe past the bound turns the table to the keyed hash",
        file: "src/exec/table/slots.rs",
        original: "pub(super) const MAX_PROBE: usize = 128;",
        replacement: "pub(super) const MAX_PROBE: usize = usize::MAX;",
        integration: &[],
        unit: &["exec::table::tests::colliding_keys_turn_the_table_keyed"],
    },
    Mutation {
        name: "group-table-slots-kept",
        package: Package::Core,
        breaks: "a table kept to its first groups gives back its slots' room",
        file: "src/exec/table.rs",
        original: "            None => self.slots.rebuilt(&self.hashes),\n        }\n        if len == 0 {",
        replacement: "            None => {}\n        }\n        if len == 0 {",
        integration: &[],
        unit: &["exec::table::tests::a_table_truncated_keeps_its_first_groups"],
    },
    Mutation {
        name: "group-table-max-reversed",
        package: Package::Core,
        breaks: "a column of MAX keeps the greatest value",
        file: "src/exec/table/states/typed.rs",
        original: "            ColumnarKind::Max => *held == 0 || units > *total,",
        replacement: "            ColumnarKind::Max => *held == 0 || units < *total,",
        integration: &[],
        unit: &[STATES],
    },
    Mutation {
        name: "group-table-scale-unchecked",
        package: Package::Core,
        breaks: "a column of integers of one scale turns to accumulators given another",
        file: "src/exec/table/states/typed.rs",
        original: "        if *self.scale.get_or_insert(ints.scale) != ints.scale {",
        replacement: "        if self.scale.get_or_insert(ints.scale) == &u32::MAX {",
        integration: &[],
        unit: &["exec::table::states::tests::columns_raise_and_widen_as_accumulators"],
    },
    Mutation {
        name: "group-table-first-latest",
        package: Package::Core,
        breaks: "a group merged from morsels keeps its earliest first row",
        file: "src/exec/table/made.rs",
        original: "                *held = (*held).min(first);",
        replacement: "                *held = (*held).max(first);",
        integration: &[SAME_ROWS],
        unit: &[],
    },
    Mutation {
        name: "group-table-partitions-merged-unordered",
        package: Package::Core,
        breaks: "the partitions' groups are merged by their first rows",
        file: "src/exec/parallel/grouped.rs",
        original: "            self.order.push(Reverse((first, at)));\n            next",
        replacement: "            self.order.push(Reverse((0, at)));\n            next",
        integration: &[SAME_ROWS],
        unit: &[],
    },
];
