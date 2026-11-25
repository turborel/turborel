//! The mutations of the executor's table of groups (RULES.md R-TEST-9,
//! D356, D357): texts equal without their trailing spaces, exact numbers
//! hashed alike at any scale, the keyed hash past the probe bound, a
//! table kept to its first groups, the columns of states fed as their
//! accumulators, a grouping run in parallel numbered and ordered by its
//! groups' first rows, its morsels of few groups merged on the statement's
//! thread, of the column store too, grouped by runs, its exchange weighed
//! by the optimizer's groups; and EXCEPT and INTERSECT counted in a table
//! (D358, D359).

use super::super::{Mutation, Package};

/// The groupings by batch of every type, in the order a row at a time
/// gives them, serial and in parallel.
const SAME_ROWS: &str = "parallel::groups_of_every_type_come_as_a_row_at_a_time_gives";

/// The groupings in parallel of few groups then many, and many then few,
/// and of the column store.
const FEW_MANY: &str = "parallel::groupings_of_few_then_many_groups_come_as_alone";

/// EXCEPT and INTERSECT against a model of their counts.
const SET_COUNTS: &str = "set_tables::set_operations_keep_what_their_counts_say";

/// The columns of states against their accumulators.
const STATES: &str = "exec::table::states::tests::columns_of_states_are_their_accumulators";

/// Mechanisms whose removal must fail the table's requirements.
pub const GROUP_TABLE: [Mutation; 17] = [
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
        original: "            *held = (*held).min(first);",
        replacement: "            *held = (*held).max(first);",
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
    Mutation {
        name: "group-table-direct-dropped",
        package: Package::Core,
        breaks: "the statement thread's table of few groups goes to the partitions' last wave",
        file: "src/exec/parallel/grouped/waves.rs",
        original: "        if !self.flushed()? {\n            return Ok(Held::GaveWay);\n        }\n        finished_wave",
        replacement: "        finished_wave",
        integration: &[FEW_MANY],
        unit: &[],
    },
    Mutation {
        name: "group-table-column-store-serial",
        package: Package::Core,
        breaks: "a grouping of the column store runs in parallel by morsels of its frozen groups",
        file: "src/physical/parallel.rs",
        original: "    if let Plan::Aggregate { .. } | Plan::Distinct { .. } = plan {\n        return false;",
        replacement: "    if let Plan::Aggregate { .. } | Plan::Distinct { .. } = plan {\n        return true;",
        integration: &["parallel::the_planner_marks_only_large_parts"],
        unit: &[],
    },
    Mutation {
        name: "group-table-runs-overrun",
        package: Package::Core,
        breaks: "a morsel's run of a column kept as runs ends where it ends",
        file: "src/exec/batch/groups/morsel.rs",
        original: "boundary = ends.next().unwrap_or(n.saturating_add(1));",
        replacement: "boundary = ends.next().map_or(n.saturating_add(1), |end| end.saturating_add(1));",
        integration: &[FEW_MANY],
        unit: &[],
    },
    Mutation {
        name: "group-table-exchange-one-group",
        package: Package::Core,
        breaks: "the exchange of a grouping is weighed by the groups the optimizer estimates",
        file: "src/physical/parallel/exchange.rs",
        original: "groups.iter().all(|&c| counted(db, input, c))).then_some(*est);",
        replacement: "groups.iter().all(|&c| counted(db, input, c))).then_some(1.0_f64);",
        integration: &["batch_kinds::groupings_of_many_groups_stay_serial"],
        unit: &[],
    },
    Mutation {
        name: "set-table-all-unmatched",
        package: Package::Core,
        breaks: "a duplicate of the second operand matches one of the first, once",
        file: "src/exec/set/tabled.rs",
        original: "                let had = *n > 0;\n                *n = n.saturating_sub(1);",
        replacement: "                let had = *n > 0;",
        integration: &[SET_COUNTS],
        unit: &[],
    },
    Mutation {
        name: "set-table-given-twice",
        package: Package::Core,
        breaks: "EXCEPT and INTERSECT give a row once",
        file: "src/exec/set/tabled.rs",
        original: "is_some_and(|given| !std::mem::replace(given, true))",
        replacement: "is_some_and(|given| {\n            *given = true;\n            true\n        })",
        integration: &[SET_COUNTS],
        unit: &[],
    },
    Mutation {
        name: "merged-groups-made-when-found",
        package: Package::Core,
        breaks: "a group a partition holds is made again of a morsel that holds it too (RULES.md D1110)",
        file: "src/exec/table/made.rs",
        original: "            if found.is_none() {\n                fresh.push(i);",
        replacement: "            if found.is_some() {\n                fresh.push(i);",
        integration: &[],
        unit: &["exec::table::made::tests::partitions_merged_by_columns_hold_what_the_morsels_hold"],
    },
    Mutation {
        name: "merged-counts-not-added",
        package: Package::Core,
        breaks: "a count merged by column takes the morsel's count in place of adding it (RULES.md D1110)",
        file: "src/exec/table/states/typed/merged.rs",
        original: "*count = count.saturating_add(n);",
        replacement: "*count = n;",
        integration: &[],
        unit: &["exec::table::made::tests::partitions_merged_by_columns_hold_what_the_morsels_hold"],
    },
    Mutation {
        name: "made-places-not-advanced",
        package: Package::Core,
        breaks: "each group of a partition is placed over the one before it (RULES.md D1110)",
        file: "src/exec/table/made.rs",
        original: "            *at = at.saturating_add(1);\n",
        replacement: "",
        integration: &[],
        unit: &["exec::table::made::tests::partitions_merged_by_columns_hold_what_the_morsels_hold"],
    },
];
