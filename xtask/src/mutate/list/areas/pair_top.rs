//! The mutations of a sort's first rows taken from the second table of a
//! pair of groupings run as one part (RULES.md R-TEST-9, D1281): which
//! pairs give them from their tables, and the rows of a partition whose
//! groups of the first are out of the order of their first rows. The
//! table they are taken from is broken by `top-groups-of-a-pair`
//! (`grouped_parts.rs`).

use super::super::{Mutation, Package};

/// A sort over a pair no partition makes whole in a table reads every
/// group.
const READ: &str = "pair_top::a_sort_over_a_pair_no_partition_makes_in_a_table_reads_every_group";

/// A partition's first rows of groups out of order are those in order.
const OUT_OF_ORDER: &str =
    "exec::parallel::grouped::top::tests::a_pairs_first_rows_of_groups_out_of_order_are_those_of_the_groups_in_order";

/// Mechanisms whose removal must fail the requirements of a pair's first
/// rows.
pub const PAIR_TOP: [Mutation; 4] = [
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
];
