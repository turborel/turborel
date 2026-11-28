//! The mutations of a sort's first rows taken from the second table of a
//! pair of groupings run as one part (RULES.md R-TEST-9, D1281): which
//! pairs give them from their tables, and the rows of a partition whose
//! groups of the first are out of the order of their first rows. The
//! table they are taken from is broken by `top-groups-of-a-pair`
//! (`grouped_parts.rs`).
//!
//! And of the plain set functions of a pair kept in a side by the second
//! grouping's columns (D1324): which pairs keep one, how its key is
//! grouped, the charge of what it holds, the order the first's groups
//! are given in, and the states a group of the second takes of it.

use super::super::{Mutation, Package};

/// A sort over a pair no partition makes whole in a table reads every
/// group.
const READ: &str = "pair_top::a_sort_over_a_pair_no_partition_makes_in_a_table_reads_every_group";

/// A partition's first rows of groups out of order are those in order.
const OUT_OF_ORDER: &str =
    "exec::parallel::grouped::top::tests::a_pairs_first_rows_of_groups_out_of_order_are_those_of_the_groups_in_order";

/// Mechanisms whose removal must fail the requirements of a pair's first
/// rows.
/// The side tables' unit tests.
const SIDE_ORDER: &str = "exec::table::sided::tests::groups_are_given_in_the_order_of_their_first_rows";
const SIDE_TAKEN: &str = "exec::table::sided::tests::a_second_table_takes_its_plain_functions_of_the_side";
/// The sided pairs' unit tests.
const SIDE_NONE: &str = "exec::parallel::grouped::sided::tests::a_pair_of_other_parts_keeps_no_side";
const SIDE_PAST: &str = "exec::parallel::grouped::sided::tests::a_sided_pair_spills_its_partitions_and_gives_way_past_its_side";
const SIDE_STRINGS: &str = "exec::parallel::grouped::sided::tests::a_side_groups_strings_as_the_pair_groups_them";
const SIDE_READ: &str = "exec::parallel::grouped::sided::tests::a_part_read_any_other_way_than_merged_whole_keeps_no_side";
const SIDE_PROBED: &str = "exec::parallel::grouped::sided::tests::the_first_morsel_s_groups_decide_whether_a_side_is_kept";
/// The analyzed plan of a pair that keeps a side and of one that keeps none.
const SIDE_SAID: &str = "sided_pairs::the_analyzed_plan_says_whether_a_side_was_kept";
/// Sided pairs by SQL against the plan run alone.
const SIDED_PAIRS: &str = "sided_pairs::plain_functions_beside_a_distinct_one_give_the_plans_groups";
/// A sided pair past the grant against the plan run alone.
const SIDED_PAST: &str = "sided_pairs::such_a_grouping_past_the_grant_gives_its_rows";

pub const PAIR_TOP: [Mutation; 11] = [
    Mutation {
        name: "pair-top-of-a-crossing-second",
        package: Package::Core,
        breaks: "first rows are taken from the tables of a second grouping that crosses the partitions, each holding a part of a group (RULES.md D1281)",
        file: "src/exec/parallel/grouped/regrouped.rs",
        original: "self.read.is_some() && self.crossing.is_none()",
        replacement: "self.read.is_some()",
        integration: &[READ],
        unit: &[],
    },
    Mutation {
        name: "pair-top-of-a-second-made-of-rows",
        package: Package::Core,
        breaks: "first rows are asked of a pair whose second no grouping by batch reads, each partition cutting groups it made of rows (RULES.md D1281)",
        file: "src/exec/parallel/grouped/regrouped.rs",
        original: "self.read.is_some() && self.crossing.is_none()",
        replacement: "self.crossing.is_none()",
        integration: &[READ],
        unit: &[],
    },
    Mutation {
        name: "pair-top-of-groups-out-of-order",
        package: Package::Core,
        breaks: "a pair's first rows are taken from a second table made of groups out of order, each numbered by the group met first (RULES.md D1281)",
        file: "src/exec/parallel/grouped/top.rs",
        original: "if !outer.tabled(&table) {",
        replacement: "if !outer.in_tables() {",
        integration: &[],
        unit: &[OUT_OF_ORDER],
    },
    Mutation {
        name: "pair-top-rows-unnumbered",
        package: Package::Core,
        breaks: "the first rows a partition keeps of a second made of rows carry no first row's number, so groups that tie come in no order of the plan (RULES.md D1281)",
        file: "src/exec/parallel/grouped/top.rs",
        original: "row.push(Value::Exact(Decimal::new(i128::from(i64::try_from(first).unwrap_or(i64::MAX)), 0)));",
        replacement: "row.push(Value::Exact(Decimal::new(0, 0)));",
        integration: &[],
        unit: &[OUT_OF_ORDER],
    },
    Mutation {
        name: "side-of-any-part",
        package: Package::Core,
        breaks: "a pair keeps a side for a part that is no column of counts or exact totals, an approximate sum or the least of strings (RULES.md D1324)",
        file: "src/exec/parallel/grouped/sided.rs",
        original: "!lanes.is_empty() && GroupTable::parts_whole(&grouping.spec.aggs)",
        replacement: "!lanes.is_empty() && !grouping.spec.aggs.is_empty()",
        integration: &[],
        unit: &[SIDE_NONE],
    },
    Mutation {
        name: "side-part-read-twice",
        package: Package::Core,
        breaks: "a pair keeps a side though two of the second's set functions read one part (RULES.md D1324)",
        file: "src/exec/parallel/grouped/sided.rs",
        original: "if part.is_some() && (!whole || from.contains(&part)) {",
        replacement: "if part.is_some() && !whole {",
        integration: &[],
        unit: &[SIDE_READ],
    },
    Mutation {
        name: "side-keys-grouped-padded",
        package: Package::Core,
        breaks: "a side groups strings padded where the pair groups them by their octets, so groups of the second share one side group (RULES.md D1324, D1184)",
        file: "src/exec/parallel/grouped/sided.rs",
        original: "seed: grouping.seed.unpadded(&unpadded),",
        replacement: "seed: grouping.seed.unpadded(&[]),",
        integration: &[],
        unit: &[SIDE_STRINGS],
    },
    Mutation {
        name: "side-held-uncharged",
        package: Package::Core,
        breaks: "what a pair's side holds is charged to no grant, so a side past the grant is held all the same (RULES.md D1324)",
        file: "src/exec/parallel/grouped/waves.rs",
        original: "self.grouping.side().map_or(0, |side| side.bytes().saturating_sub(self.side_bytes))",
        replacement: "self.grouping.side().map_or(0, |_| 0)",
        integration: &[SIDED_PAST],
        unit: &[SIDE_PAST],
    },
    Mutation {
        name: "side-first-groups-as-they-lie",
        package: Package::Core,
        breaks: "the first's groups of a sided pair are given to the second out of the order of their first rows (RULES.md D1324)",
        file: "src/exec/table/sided.rs",
        original: "        if firsts.is_sorted() {",
        replacement: "        if !firsts.is_empty() {",
        integration: &[],
        unit: &[SIDE_ORDER],
    },
    Mutation {
        name: "side-states-not-taken",
        package: Package::Core,
        breaks: "a group of the second grouping takes no state of its side group, so its plain set functions are of no row (RULES.md D1324)",
        file: "src/exec/table/sided.rs",
        original: "                states.merge_part(part, (&groups, &places))?;",
        replacement: "                drop((part, &groups, &places));",
        integration: &[SIDED_PAIRS],
        unit: &[SIDE_TAKEN],
    },
    Mutation {
        name: "side-kept-whatever-the-first-morsel",
        package: Package::Core,
        breaks: "a pair keeps a side though its first morsel makes a side group for every group of the first grouping, so the side saves no state (RULES.md D1324)",
        file: "src/exec/parallel/grouped/sided.rs",
        original: "Ok((side > 0 && side.saturating_mul(FIRSTS_A_SIDE) <= firsts, side))",
        replacement: "Ok((side > 0 && firsts > 0, side))",
        integration: &[SIDE_SAID],
        unit: &[SIDE_PROBED],
    },
];
