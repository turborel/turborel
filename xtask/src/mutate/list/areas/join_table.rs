//! The mutations of a hash join's table and its runtime filters (RULES.md
//! R-TEST-9, D368, D369): the rows of a hash found in the order they were
//! built, the tag bits of a slot's word, integers of two scales rescaled
//! as they are compared, a filter kept above a condition that can raise,
//! and a filter that keeps most rows stopped; of D389, a pipeline run in
//! parallel in its plan's order, a pipeline's scan told its filters, a
//! scan's range judged once, a spilled join's probes numbered, and a
//! filter stopped at a projection that can raise, as a part hoisted under
//! its guard is.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail the join table's requirements.
pub const JOIN_TABLE: [Mutation; 10] = [
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
    Mutation {
        name: "join-parallel-out-of-order",
        package: Package::Core,
        breaks: "a pipeline run in parallel gives its batches in its plan's order, so a scan in none is gathered in none (D389)",
        file: "src/exec/batch/joined.rs",
        original: "        Plan::Parallel(p) => !crate::exec::parallel::pipelined(&p.input) || in_order(&p.input),",
        replacement: "        Plan::Parallel(_) => true,",
        integration: &[],
        unit: &["exec::batch::joined::tests::rows_asked_for_in_an_order"],
    },
    Mutation {
        name: "join-pipeline-filters-untold",
        package: Package::Core,
        breaks: "a pipeline's hash join tells its probing scan its runtime filters (D389)",
        file: "src/exec/parallel/pipeline.rs",
        original: "            told.extend(table.shared.filters(keys)?);",
        replacement: "            told.extend(table.shared.filters(keys)?.into_iter().take(0));",
        integration: &[],
        unit: &["exec::parallel::pipeline::tests::a_probing_side_is_told_the_filters"],
    },
    Mutation {
        name: "join-scan-range-judged-twice",
        package: Package::Core,
        breaks: "a scan judges a filter's range among its conjuncts, so the filter does not again (D389)",
        file: "src/exec/batch/runtime.rs",
        original: "    (blooms || !at_scan).then_some((!at_scan, blooms))",
        replacement: "    Some((true, blooms))",
        integration: &[],
        unit: &["exec::batch::runtime::tests::a_scan_judges_its_range_once"],
    },
    Mutation {
        name: "join-spill-probes-misnumbered",
        package: Package::Core,
        breaks: "a spilled join's partition probed by batch writes each joined row under its left row's number (D389)",
        file: "src/exec/join/partitioned.rs",
        original: "        let (Some(&n), Some(l)) = (numbers.get(r), lefts.get(r)) else { continue };",
        replacement: "        let (Some(&n), Some(l)) = (numbers.get(r.saturating_add(1)), lefts.get(r)) else { continue };",
        integration: &["join_tables::a_join_keeps_its_left_order_spilled_and_in_parallel"],
        unit: &[],
    },
    Mutation {
        name: "join-filter-below-raising-projection",
        package: Package::Core,
        breaks: "a runtime filter goes below no projection that can raise, so into no part hoisted under its guard (D15, D373)",
        file: "src/exec/batch/runtime.rs",
        original: "        Plan::Project { exprs, .. } => moved(exprs, column).filter(|_| !exprs.iter().any(Expr::can_raise)),",
        replacement: "        Plan::Project { exprs, .. } => moved(exprs, column),",
        integration: &["join_tables::a_filter_enters_no_guarded_part"],
        unit: &[],
    },
];
