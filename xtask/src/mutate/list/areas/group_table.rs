//! The mutations of the executor's table of groups (RULES.md R-TEST-9,
//! D356, D357): texts equal without their trailing spaces, exact numbers
//! hashed alike at any scale, the keyed hash past the probe bound, a
//! table kept to its first groups, the columns of states fed as their
//! accumulators, a grouping run in parallel numbered and ordered by its
//! groups' first rows, its morsels of few groups merged on the statement's
//! thread, of the column store too, grouped by runs, its exchange weighed
//! by the optimizer's groups; and EXCEPT and INTERSECT counted in a table
//! (D358, D359); a grouping's partitions held in the spill past its
//! grant, written and read back as they were (D1226); a partition's
//! groups adopted where they lie (D1300); and integers compared by their
//! words, a null told apart from the zero in its place (D1274).

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

/// A grouping in parallel whose partitions go to the spill past its grant.
const SPILLED: &str = "spilled_groups::partitions_past_the_grant_are_spilled_by_their_threads";

/// The unit test of a crossing second grouping's groups spilled.
const CROSSING: &str = "exec::parallel::grouped::regrouped::crossing::tests::groups_past_the_grant_merge_as_those_held";

/// A grouping in parallel spilled past its grant in its one area, given
/// way to in two.
const SPILLED_AREA: &str = "spilled_groups::a_grouping_spilled_holds_its_partitions_in_one_area";

/// Groups of every kind of state read back from the spill against those
/// written.
const SPILLED_MERGE: &str = "exec::table::made::written::tests::groups_read_back_finish_as_those_written";

/// Columns of states of every kind read back from the spill bit for bit.
const SPILLED_STATES: &str = "exec::table::states::typed::written::tests::every_column_of_states_is_read_back_as_written";

/// A wide partition against the table merged of the same Mades.
const WIDE: &str = "exec::table::made::wide::tests::a_wide_partition_holds_the_groups_a_table_holds_in_its_order";

/// A wide partition of keys that collide.
const WIDE_KEYED: &str = "exec::table::made::wide::tests::colliding_groups_turn_a_wide_partition_keyed";

/// Groupings with their partitions forced wide against the same not wide.
const WIDE_WAVES: &str = "exec::parallel::grouped::waves::tests::the_groups_are_the_same_whether_partitions_go_wide";

/// A sort's first rows of wide partitions against those of tables.
const WIDE_FIRST: &str = "exec::parallel::grouped::waves::tests::the_first_rows_by_a_set_function_are_the_same_of_wide_partitions";

/// A partition wide only where its groups are read as they lie.
const WIDE_READ: &str = "exec::parallel::grouped::waves::tests::a_partition_goes_wide_only_where_its_groups_are_read_as_they_lie";

/// Mechanisms whose removal must fail the table's requirements.
pub const GROUP_TABLE: [Mutation; 46] = [
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
        file: "src/exec/parallel/grouped/runs.rs",
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
        original: "        if !self.flushed()? {\n            return Ok(Held::GaveWay);\n        }\n        let Some(spilled)",
        replacement: "        let Some(spilled)",
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
        name: "merged-groups-new-not-noted",
        package: Package::Core,
        breaks: "a group new to a partition is given its number and slot but its key is never appended (RULES.md D1110)",
        file: "src/exec/table/made.rs",
        original: "                fresh.push(i);\n",
        replacement: "",
        integration: &[],
        unit: &["exec::table::made::tests::partitions_merged_by_columns_hold_what_the_morsels_hold"],
    },
    Mutation {
        name: "merged-group-not-appended-read-by-its-number",
        package: Package::Core,
        breaks: "a key that meets the slot of a group not yet appended is compared with the key at that group's number, not at its place (RULES.md D1275)",
        file: "src/exec/table/made.rs",
        original: "Some(new) => unappended_equal(made, fresh.get(new).copied(), key, scratch),",
        replacement: "Some(new) => unappended_equal(made, u32::try_from(new).ok(), key, scratch),",
        integration: &[],
        unit: &["exec::table::made::tests::groups_merged_in_one_pass_are_those_found_or_made_in_turn"],
    },
    Mutation {
        name: "merged-groups-dropped-past-a-long-probe",
        package: Package::Core,
        breaks: "the groups after the one whose probe grew past the longest are taken for looked up, so none of them is merged (RULES.md D1275)",
        file: "src/exec/table/made.rs",
        original: "return (n.saturating_add(1), true);",
        replacement: "return (places.len(), true);",
        integration: &[],
        unit: &["exec::table::made::tests::groups_merged_in_one_pass_are_those_found_or_made_in_turn"],
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
    Mutation {
        name: "few-lanes-one-added",
        package: Package::Core,
        breaks: "the sums of few groups fed by lanes keep only the first lane's partials (RULES.md D1120)",
        file: "src/exec/table/states/typed/few.rs",
        original: "    for sum in &sums {",
        replacement: "    for sum in sums.iter().take(1) {",
        integration: &[],
        unit: &["exec::table::states::typed::few::tests::sums_and_counts_by_lanes_are_those_fed_one_at_a_time"],
    },
    Mutation {
        name: "few-lanes-nulls-fed",
        package: Package::Core,
        breaks: "a sum of few groups is fed by lanes past its nulls, each counted as a value of 0 (RULES.md D1120)",
        file: "src/exec/table/states/typed.rs",
        original: "            && ints.nulls.is_none()\n",
        replacement: "",
        integration: &["grouped_parts::a_having_condition_is_judged_by_its_part"],
        unit: &[],
    },
    Mutation {
        name: "coded-null-place-of-code",
        package: Package::Core,
        breaks: "a null key's place is that of its code, so it goes to the group of the dictionary's first entry (RULES.md D1120, D1161)",
        file: "src/exec/batch/groups/coded.rs",
        original: "if texts.is_null(r) { entries.len() } else { entry }",
        replacement: "entry",
        integration: &[],
        unit: &["exec::batch::groups::coded::tests::rows_grouped_by_their_codes_make_the_groups_of_their_texts"],
    },
    Mutation {
        name: "spilled-groups-give-way",
        package: Package::Core,
        breaks: "partitions past the grant give way to a grouping a row at a time, the threads' work dropped (RULES.md D1226)",
        file: "src/exec/parallel/grouped/waves.rs",
        original: "    fn spilled_all(&mut self) -> Result<bool> {\n",
        replacement: "    fn spilled_all(&mut self) -> Result<bool> {\n        if self.spilled.is_none() {\n            return Ok(false);\n        }\n",
        integration: &[SPILLED, SPILLED_AREA],
        unit: &[],
    },
    Mutation {
        name: "spilled-split-by-the-same-bits",
        package: Package::Core,
        breaks: "a spilled partition past the grant is split at every level by the bits that chose it, so all of it falls in one part again and the grouping gives way at the depth (RULES.md D1226)",
        file: "src/exec/parallel/grouped/waves/spilled.rs",
        original: "u32::try_from(level.saturating_add(1)).unwrap_or(u32::MAX)",
        replacement: "0_u32",
        integration: &[SPILLED_AREA],
        unit: &[],
    },
    Mutation {
        name: "spilled-firsts-as-hashes",
        package: Package::Core,
        breaks: "groups read back from the spill take their hashes for their first rows (RULES.md D1226)",
        file: "src/exec/table/made/written.rs",
        original: "        words(&self.firsts, out);",
        replacement: "        words(&self.hashes, out);",
        integration: &[],
        unit: &[SPILLED_MERGE],
    },
    Mutation {
        name: "spilled-key-float-sign",
        package: Package::Core,
        breaks: "an approximate key read back from the spill loses its sign, -0 read as 0 (RULES.md D1226)",
        file: "src/exec/table/keys/written.rs",
        original: "out.put_u64(at(numbers, g, 0.0_f64).to_bits())",
        replacement: "out.put_u64(at(numbers, g, 0.0_f64).abs().to_bits())",
        integration: &[],
        unit: &["exec::table::keys::written::tests::every_lane_is_read_back_as_written"],
    },
    Mutation {
        name: "spilled-sum-small-part",
        package: Package::Core,
        breaks: "an approximate sum read back from the spill loses the part of its values below 2^-900 (RULES.md D1226)",
        file: "src/exec/table/states/typed/written.rs",
        original: "                    out.put_u64(small.to_bits());",
        replacement: "                    out.put_u64(0.0_f64.to_bits());",
        integration: &[],
        unit: &[SPILLED_STATES],
    },
    Mutation {
        name: "spilled-finished-unnumbered",
        package: Package::Core,
        breaks: "a spilled part's finished groups are written without their first rows' numbers, so the merge gives them by partition, not in the order of their first rows (RULES.md D1226)",
        file: "src/exec/parallel/grouped/waves/spilled.rs",
        original: "            run.push(&numbered(n, &row, &lobs))?;",
        replacement: "            run.push(&numbered(0, &row, &lobs))?;",
        integration: &[SPILLED],
        unit: &[],
    },
    Mutation {
        name: "crossing-held-dropped",
        package: Package::Core,
        breaks: "the groups a crossing second grouping held when they passed the grant are not written to the spill (RULES.md D1228)",
        file: "src/exec/parallel/grouped/regrouped/crossing.rs",
        original: "Spill::of(held, spec, (db, tx, &limit))",
        replacement: "Spill::of(Vec::new(), spec, (db, tx, &limit))",
        integration: &[],
        unit: &[CROSSING],
    },
    Mutation {
        name: "crossing-runs-by-first",
        package: Package::Core,
        breaks: "a crossing second grouping's groups go to the spill's runs by their firsts, not their keys, so a key's groups are merged apart (RULES.md D1228)",
        file: "src/exec/parallel/grouped/regrouped/crossing.rs",
        original: "let p = part_of(&group_key(spec, &values)?, seed);",
        replacement: "let p = part_of(&first.to_le_bytes(), seed);",
        integration: &[],
        unit: &[CROSSING],
    },
    Mutation {
        name: "wide-group-met-again-left-where-it-lay",
        package: Package::Core,
        breaks: "a group of a wide partition that a second row meets is taken up into the partition's own table but still listed where it lay, so it is taken up again by every later row and read with its first row's states alone (RULES.md D1300)",
        file: "src/exec/table/made/wide.rs",
        original: "        *lies = (OWN, number);",
        replacement: "        *lies = (segment, place);",
        integration: &[],
        unit: &[WIDE],
    },
    Mutation {
        name: "wide-group-taken-up-with-no-state",
        package: Package::Core,
        breaks: "a group of a wide partition taken up into the partition's own table is given its key and first row but not the states of its first row, so its results are those of its later rows alone (RULES.md D1300)",
        file: "src/exec/table/made/wide.rs",
        original: "            states.merge((index(g), agg), (sent, at))?;",
        replacement: "            if at == usize::MAX { states.merge((index(g), agg), (sent, at))?; }",
        integration: &[],
        unit: &[WIDE],
    },
    Mutation {
        name: "wide-held-rows-not-merged",
        package: Package::Core,
        breaks: "the rows of a Made that met a group a wide partition holds are not merged into it (RULES.md D1300)",
        file: "src/exec/table/made/wide.rs",
        original: "        self.own.merge_into(&groups, made, &rows)?;",
        replacement: "        self.own.merge_into(&[], made, &[])?;",
        integration: &[],
        unit: &[WIDE],
    },
    Mutation {
        name: "wide-gathered-out-of-order",
        package: Package::Core,
        breaks: "a wide partition gathered into a table takes a Made's groups up last first, so its groups are not in the order of their first rows (RULES.md D1300)",
        file: "src/exec/table/made/wide.rs",
        original: "            run.push(place);",
        replacement: "            run.insert(0, place);",
        integration: &[],
        unit: &[WIDE],
    },
    Mutation {
        name: "wide-rekeyed-forgotten",
        package: Package::Core,
        breaks: "a wide partition whose probe grew long places its groups by keyed hashes but looks keys up by the hashes they came with, so no group is found again (RULES.md D1300, D357)",
        file: "src/exec/table/made/wide.rs",
        original: "        self.keyed = Some(keyed);",
        replacement: "        drop(keyed);",
        integration: &[],
        unit: &[WIDE_KEYED],
    },
    Mutation {
        name: "partition-never-wide",
        package: Package::Core,
        breaks: "a partition's table never goes wide, however many groups it holds (RULES.md D1300)",
        file: "src/exec/parallel/grouped/waves/wide.rs",
        original: "if table.len() > widen {",
        replacement: "if table.len() > widen.max(usize::MAX) {",
        integration: &[],
        unit: &[WIDE_WAVES],
    },
    Mutation {
        name: "wide-first-rows-of-its-own-table-alone",
        package: Package::Core,
        breaks: "a sort's first rows of a wide partition are picked of the groups of several rows alone, those of one row where they lie never read (RULES.md D1300)",
        file: "src/exec/parallel/grouped/top/wide.rs",
        original: "let batches = own_batch.into_iter().chain(rest);",
        replacement: "let batches = own_batch.into_iter().chain(rest.take(0));",
        integration: &[],
        unit: &[WIDE_FIRST],
    },
    Mutation {
        name: "wide-first-rows-of-its-mades-alone",
        package: Package::Core,
        breaks: "a sort's first rows of a wide partition are picked of the groups of one row alone, the partition's own table never read (RULES.md D1300)",
        file: "src/exec/parallel/grouped/top/wide.rs",
        original: "(held.len() > 0).then(",
        replacement: "(held.len() > held.len()).then(",
        integration: &[],
        unit: &[WIDE_FIRST],
    },
    Mutation {
        name: "wide-last-batch-of-mades-not-given",
        package: Package::Core,
        breaks: "the Mades left after the last full batch of a wide partition's groups of one row are read in no batch, so a sort's first rows are picked without them (RULES.md D1300)",
        file: "src/exec/parallel/grouped/top/wide.rs",
        original: "    if open.to > open.from {",
        replacement: "    if open.to > open.to {",
        integration: &[],
        unit: &[WIDE_FIRST],
    },
    Mutation {
        name: "wide-batch-numbered-from-the-first",
        package: Package::Core,
        breaks: "every batch of a wide partition's Mades numbers its groups from the partition's first, so a group picked of a later batch is made a row of a group of the first (RULES.md D1300)",
        file: "src/exec/parallel/grouped/top/wide.rs",
        original: "            open = Span { from: open.to, to: open.to, base };",
        replacement: "            open = Span { from: open.to, to: open.to, base: own };",
        integration: &[],
        unit: &[WIDE_FIRST],
    },
    Mutation {
        name: "wide-picked-row-of-another-made",
        package: Package::Core,
        breaks: "a group picked where it lies in a wide partition is made a row of the Made before its own (RULES.md D1300)",
        file: "src/exec/parallel/grouped/top/wide.rs",
        original: "let k = bases.partition_point(|&base| base <= group).saturating_sub(1);",
        replacement: "let k = bases.partition_point(|&base| base <= group).saturating_sub(2);",
        integration: &[],
        unit: &[WIDE_FIRST],
    },
    Mutation {
        name: "group-read-where-it-lies-fed-nothing",
        package: Package::Core,
        breaks: "the results of groups read where they lie are those of states fed nothing, the Made's states not merged into them (RULES.md D1300)",
        file: "src/exec/table/states/placed.rs",
        original: "states.merge_all(agg, run, (sent, places))?;",
        replacement: "states.merge_all(agg, &[], (sent, &[]))?;",
        integration: &[],
        unit: &[WIDE_FIRST],
    },
    Mutation {
        name: "wide-whatever-reads-the-groups",
        package: Package::Core,
        breaks: "a partition goes wide of a grouping every group of which is read, and is gathered into a table again (RULES.md D1300)",
        file: "src/exec/parallel/grouped/waves/wide.rs",
        original: "let past = if widen.every || lies(grouping) { widen.past } else { usize::MAX };",
        replacement: "let past = widen.past;",
        integration: &[],
        unit: &[WIDE_READ],
    },
    Mutation {
        name: "key-words-of-any-scale",
        package: Package::Core,
        breaks: "a lane of integers is compared by its words with a column of another scale, so 100 equals 1.00 (RULES.md D1274)",
        file: "src/exec/table/keys/words.rs",
        original: "(ints.units.get(r), ints.scale == scale, ints.is_null(r))",
        replacement: "(ints.units.get(r), true, ints.is_null(r))",
        integration: &[],
        unit: &["exec::table::keys::tests::integers_are_found_equal_by_their_words_as_by_their_cells"],
    },
    Mutation {
        name: "key-words-null-as-its-place",
        package: Package::Core,
        breaks: "integers compared by their words take a null for the zero in its place (RULES.md D1274)",
        file: "src/exec/table/keys/words.rs",
        original: "Some(if mine || null { mine && null } else { units.get(g) == Some(&theirs) })",
        replacement: "Some(units.get(g) == Some(&theirs) || (mine && null))",
        integration: &[],
        unit: &["exec::table::keys::tests::integers_are_found_equal_by_their_words_as_by_their_cells"],
    },
    Mutation {
        name: "lane-null-not-told",
        package: Package::Core,
        breaks: "a lane given a null does not note that it holds one, so the group reads as the value in its place (RULES.md D1274)",
        file: "src/exec/table/keys.rs",
        original: "            self.nulled = true;\n",
        replacement: "",
        integration: &[],
        unit: &["exec::table::keys::tests::integers_are_found_equal_by_their_words_as_by_their_cells"],
    },
];
