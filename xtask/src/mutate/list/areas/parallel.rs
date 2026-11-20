//! The mutations of parallel queries and work grants (RULES.md R-TEST-9,
//! D290 to D298): a part marked whatever its rows, a filter's step lost,
//! groups given out of their first rows' order or merged out of morsel
//! order, a sort's ties taken from the later run, a probe's matches out of
//! build order, a set operation's place left on its rows, a window's
//! partitions not merged by their keys, a row lost at each morsel's
//! start, an index range's last part lost, and a grant of the whole free
//! budget.

use super::super::{Mutation, Package};

/// The test of every operator run in parallel against it alone.
const SAME_ROWS: &str = "parallel::parallel_parts_give_the_rows_alone_gives";
/// The test of hash joins built and probed by morsels.
const JOINS: &str = "parallel::joins_build_and_probe_by_morsels";
/// The test of the planner's choice.
const PLANNER: &str = "parallel::the_planner_marks_only_large_parts";
/// The test of index ranges split.
const INDEX_RANGES: &str = "parallel::index_ranges_split_into_morsels";
/// The unit test of work grants.
const GRANTS: &str = "spill::grant::tests::grants_halve_the_free_budget";

/// Mechanisms of parallel queries and grants whose removal must fail their tests.
pub const PARALLEL: [Mutation; 11] = [
    Mutation {
        name: "parallel-least-rows-ignored",
        package: Package::Core,
        breaks: "a part reading fewer rows than EXECUTOR.PARALLEL_MIN_ROWS is run in parallel",
        file: "src/physical/parallel.rs",
        original: "if rows < self.least {",
        replacement: "if rows < self.least.min(0) {",
        integration: &[PLANNER],
        unit: &[],
    },
    Mutation {
        name: "parallel-filter-lost",
        package: Package::Core,
        breaks: "a filter over a table split into morsels keeps every row its scan reads",
        file: "src/exec/parallel/pipeline.rs",
        original: "steps: vec![Step::Filter(cond.clone())]",
        replacement: "steps: Vec::new()",
        integration: &[SAME_ROWS],
        unit: &[],
    },
    Mutation {
        name: "parallel-groups-out-of-order",
        package: Package::Core,
        breaks: "groups made in parallel are given last first, not in their first rows' order",
        file: "src/exec/parallel/grouped.rs",
        original: "groups.sort_unstable_by_key(|g| g.0);",
        replacement: "groups.sort_unstable_by_key(|g| std::cmp::Reverse(g.0));",
        integration: &[SAME_ROWS],
        unit: &[],
    },
    Mutation {
        name: "parallel-pieces-reversed",
        package: Package::Core,
        breaks: "a partition's pieces are merged last morsel first, each group numbered by a later row",
        file: "src/exec/parallel/grouped.rs",
        original: "for piece in chunks.into_iter().flatten() {",
        replacement: "for piece in chunks.into_iter().rev().flatten() {",
        integration: &[SAME_ROWS],
        unit: &[],
    },
    Mutation {
        name: "parallel-sort-ties-later",
        package: Package::Core,
        breaks: "the merge of a sort's runs takes a tie from the later run, peers out of their input order",
        file: "src/exec/parallel/sorted.rs",
        original: ".then_with(|| other.run.cmp(&self.run))",
        replacement: ".then_with(|| self.run.cmp(&other.run))",
        integration: &[SAME_ROWS],
        unit: &[],
    },
    Mutation {
        name: "parallel-probe-reversed",
        package: Package::Core,
        breaks: "a probe of a table built by morsels gives a row's matches last built first",
        file: "src/exec/parallel/probe.rs",
        original: "        for r in rows {",
        replacement: "        for r in rows.iter().rev() {",
        integration: &[SAME_ROWS, JOINS],
        unit: &[],
    },
    Mutation {
        name: "parallel-place-left",
        package: Package::Core,
        breaks: "a row of EXCEPT or INTERSECT run in parallel keeps the place it carried",
        file: "src/exec/parallel/partitioned.rs",
        original: "row.truncate(width);",
        replacement: "row.truncate(width.saturating_add(1));",
        integration: &[SAME_ROWS],
        unit: &[],
    },
    Mutation {
        name: "parallel-window-unmerged",
        package: Package::Core,
        breaks: "a window's partitions are given one after another, not merged by their sort keys",
        file: "src/exec/parallel/partitioned.rs",
        original: "merge_by(&outs, &keys)",
        replacement: "merge_by(&outs, &[])",
        integration: &[SAME_ROWS],
        unit: &[],
    },
    Mutation {
        name: "parallel-morsel-first-row-lost",
        package: Package::Core,
        breaks: "each morsel of the row store begins a row id past its cut",
        file: "src/engine/rows/split.rs",
        original: "Some((first, self.cuts.get(n).copied()))",
        replacement: "Some((first.saturating_add(1), self.cuts.get(n).copied()))",
        integration: &[SAME_ROWS],
        unit: &[],
    },
    Mutation {
        name: "parallel-index-last-part-lost",
        package: Package::Core,
        breaks: "an index range cut at its tree's keys loses its part past the last cut",
        file: "src/index/blink.rs",
        original: "parts.push(Range { lower, upper: range.upper, checked: range.checked });",
        replacement: "drop((lower, range.upper));",
        integration: &[INDEX_RANGES],
        unit: &[],
    },
    Mutation {
        name: "grant-whole-free-budget",
        package: Package::Core,
        breaks: "a work grant takes all the free budget, leaving none for the grants after it",
        file: "src/spill/grant.rs",
        original: "let wanted = ceiling.min(free >> 1_u32);",
        replacement: "let wanted = ceiling.min(free);",
        integration: &[],
        unit: &[GRANTS],
    },
];
