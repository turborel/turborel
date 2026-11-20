//! The mutations of a hash join's table and its runtime filters (RULES.md
//! R-TEST-9, D368, D369): the rows of a hash found in the order they were
//! built, the tag bits of a slot's word, integers of two scales rescaled
//! as they are compared, a filter kept above a condition that can raise,
//! and a filter that keeps most rows stopped.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail the join table's requirements.
pub const JOIN_TABLE: [Mutation; 5] = [
    Mutation {
        name: "join-table-build-order",
        package: Package::Core,
        breaks: "a sorted slot gives the rows of one hash in the order they were built",
        file: "src/exec/table/join/placed.rs",
        original: "        pairs.sort_by_key(|&(h, _)| h);",
        replacement: "        pairs.sort_by_key(|&(h, r)| (h, u32::MAX - r));",
        integration: &[],
        unit: &["exec::table::join::tests::a_table_finds_a_hashs_rows_in_build_order"],
    },
    Mutation {
        name: "join-table-tags-unset",
        package: Package::Core,
        breaks: "a slot's word holds a tag bit of each of its rows' hashes",
        file: "src/exec/table/join.rs",
        original: "            let tags = table.hashes.get(*from..*to).unwrap_or_default().iter().fold(0_u64, |t, &h| t | tag(h));",
        replacement: "            let tags = table.hashes.get(*from..*to).unwrap_or_default().iter().fold(0_u64, |t, _| t);",
        integration: &[],
        unit: &["exec::table::join::tests::tags_turn_away_a_probe_of_no_bit"],
    },
    Mutation {
        name: "join-table-scales-unscaled",
        package: Package::Core,
        breaks: "integers of two scales compared in place are rescaled",
        file: "src/exec/table/join/equal.rs",
        original: "    Some(RADIX.checked_pow(by).and_then(|f| low.checked_mul(f)) == Some(high))",
        replacement: "    Some(by > 0 && low == high)",
        integration: &[],
        unit: &["exec::table::join::tests::keys_compared_in_place_match_as_values_do"],
    },
    Mutation {
        name: "join-filter-below-raising",
        package: Package::Core,
        breaks: "a runtime filter goes below no condition that can raise (D15)",
        file: "src/exec/batch/runtime.rs",
        original: "        Plan::Filter { cond, .. } => (!cond.can_raise() && !cond.has_subquery()).then_some(column),",
        replacement: "        Plan::Filter { cond, .. } => (!cond.has_subquery()).then_some(column),",
        integration: &["join_tables::a_filter_hides_no_error"],
        unit: &[],
    },
    Mutation {
        name: "join-filter-never-stops",
        package: Package::Core,
        breaks: "a filter whose first batches kept most of their rows stops judging",
        file: "src/exec/table/runtime.rs",
        original: "        kept.saturating_mul(TENTHS) <= judged.saturating_mul(KEEP_MOST)",
        replacement: "        kept.saturating_mul(TENTHS) <= judged.saturating_mul(TENTHS)",
        integration: &[],
        unit: &["exec::table::runtime::tests::a_filter_that_keeps_most_rows_stops"],
    },
];
