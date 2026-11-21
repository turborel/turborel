//! The mutations of range joins (RULES.md R-TEST-9, D580-D581): an open
//! bound searched as closed, a left row's matches left in the sorted
//! order, the pruning tree keeping the least where it needs the greatest,
//! an unmatched LEFT row lost, a condition that can raise searched, and
//! the overlap of two periods read as closed at its end.

use super::super::{Mutation, Package};

/// The search of the sorted rows.
const INDEX_FILE: &str = "src/exec/join/range/index.rs";
/// The bands' integration test.
const BANDS: &str = "range_joins::bands_give_the_nested_loops_rows_in_its_order";

/// Mechanisms of range joins whose removal must fail their tests.
pub const RANGE_JOINS: [Mutation; 6] = [
    Mutation {
        name: "range-open-bound-closed",
        package: Package::Core,
        breaks: "a sorted value equal to an open lower bound is searched as within it",
        file: INDEX_FILE,
        original: "    ord == Ordering::Less || (!closed && ord == Ordering::Equal)",
        replacement: "    ord == Ordering::Less",
        integration: &[BANDS],
        unit: &[],
    },
    Mutation {
        name: "range-matches-unordered",
        package: Package::Core,
        breaks: "a left row's matches come in the order of the sorted value, not of the right operand",
        file: INDEX_FILE,
        original: "        out.sort_unstable();\n        Ok(searched)",
        replacement: "        Ok(searched)",
        integration: &["range_joins::matches_come_in_the_right_operands_order"],
        unit: &[],
    },
    Mutation {
        name: "range-prune-least-for-greatest",
        package: Package::Core,
        breaks: "the pruning tree keeps the least of a subtree's values where `>` needs the greatest",
        file: "src/exec/join/range/pruned.rs",
        original: "        let greatest = matches!(op, CompOp::Gt | CompOp::Ge);",
        replacement: "        let greatest = matches!(op, CompOp::Lt | CompOp::Le);",
        integration: &[BANDS, "range_joins::periods::each_period_predicate_joins_as_the_nested_loop_8_20"],
        unit: &["exec::join::range::pruned::tests"],
    },
    Mutation {
        name: "range-left-unmatched-lost",
        package: Package::Core,
        breaks: "a LEFT range join drops each left row that matches none",
        file: "src/exec/join/range.rs",
        original: "        nulls: outer.then(|| vec![Value::Null; widths.1]),",
        replacement: "        nulls: (outer && widths.1 == usize::MAX).then(|| vec![Value::Null; widths.1]),",
        integration: &["range_joins::outer_joins_keep_their_unmatched_rows"],
        unit: &[],
    },
    Mutation {
        name: "range-raising-searched",
        package: Package::Core,
        breaks: "a join whose condition can raise is a range join, judging its comparisons apart from the rest",
        file: "src/physical/range/planned.rs",
        original: "        if !self.db.settings.is_on(RANGE_JOINS) || self.cond_raises(cond, (left, right)) {",
        replacement: "        if !self.db.settings.is_on(RANGE_JOINS) {",
        integration: &["range_joins::raising_conditions_and_large_operands_keep_the_nested_loop"],
        unit: &[],
    },
    Mutation {
        name: "range-overlap-end-closed",
        package: Package::Core,
        breaks: "a period overlaps one that starts where it ends, its end taken as within it",
        file: "src/physical/range/periods.rs",
        original: "        PeriodOperator::Overlaps => vec![compared(s1, Lt, e2), compared(e1, Gt, s2)],",
        replacement: "        PeriodOperator::Overlaps => vec![compared(s1, Le, e2), compared(e1, Ge, s2)],",
        integration: &["range_joins::periods::each_period_predicate_joins_as_the_nested_loop_8_20"],
        unit: &["physical::range::tests::period_predicates_are_their_comparisons_8_20"],
    },
];
