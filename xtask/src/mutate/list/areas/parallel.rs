//! The mutations of parallel queries and work grants (RULES.md R-TEST-9,
//! D290 to D298): a part marked whatever its rows, a filter's step lost,
//! groups given out of their first rows' order or merged out of morsel
//! order, a sort's ties taken from the later run, a probe's matches out of
//! build order, a set operation's place left on its rows, a window's
//! partitions not merged by their keys, a row lost at each morsel's
//! start, an index range's last part lost, a grant of the whole free
//! budget, a part's operators reserving the budget again, a column
//! store's scan by batch run in parallel, a hash join's operand built
//! of rows where it gives batches, one of the column store kept off the
//! workers, a morsel's tree rows read of zones not judged, and a built
//! operand's datetimes joined whatever their types or without their
//! nulls.

use super::super::{Mutation, Package};

/// The test of every operator run in parallel against it alone.
const SAME_ROWS: &str = "parallel::parallel_parts_give_the_rows_alone_gives";
/// The test of the planner's choice.
const PLANNER: &str = "parallel::the_planner_marks_only_large_parts";
/// The test of index ranges split.
const INDEX_RANGES: &str = "parallel::index_ranges_split_into_morsels";
/// The unit test of work grants.
const GRANTS: &str = "spill::grant::tests::grants_halve_the_free_budget";
/// The unit test of a hash join's operand built by batch.
const BUILT: &str = "exec::parallel::probe::tests::an_operand_that_gives_batches_is_built_of_its_vectors";
/// The test of hash joins' built operands of the column store.
const BUILDS: &str = "parallel::joins_build_by_batch";
/// The unit test of vectors of words joined.
const JOINED: &str = "storage::batch::joined::tests::datetimes_and_truth_values_stay_vectors_of_words";
/// The unit test of grants inside a statement's grant.
const COVERED: &str = "spill::grant::tests::covered_grants_reserve_nothing";
/// The unit test of partitions' results merged as read back from lists
/// that spilled.
const MERGED: &str = "exec::parallel::partitioned::merged::tests::lists_spilled_are_merged_as_read_back";
/// The test of windows and set operations at the smallest grant.
const SPILLED_WINDOWS: &str = "spilled_windows::partitions_past_the_grant_give_their_results_in_order";

/// Mechanisms of parallel queries and grants whose removal must fail their tests.
pub const PARALLEL: [Mutation; 27] = [
    Mutation {
        name: "parallel-least-rows-ignored",
        package: Package::Core,
        breaks: "a part reading fewer rows than EXECUTOR.PARALLEL_MIN_ROWS is run in parallel",
        file: "src/physical/cost/parallel.rs",
        original: "        if rows < self.least {",
        replacement: "        if rows < self.least.min(0) {",
        integration: &[PLANNER],
        unit: &[],
    },
    Mutation {
        name: "parallel-filter-lost",
        package: Package::Core,
        breaks: "a filter over a table split into morsels keeps every row its scan reads",
        file: "src/exec/parallel/pipeline.rs",
        original: "steps: cond.map(|c| Step::Filter(c.clone())).into_iter().collect()",
        replacement: "steps: Vec::new()",
        integration: &[SAME_ROWS],
        unit: &[],
    },
    Mutation {
        name: "parallel-groups-out-of-order",
        package: Package::Core,
        breaks: "groups made in parallel are given last first, not in their first rows' order",
        file: "src/exec/parallel/grouped.rs",
        original: "chunks)?;\n    groups.sort_unstable_by_key(|g| g.0);",
        replacement: "chunks)?;\n    groups.sort_unstable_by_key(|g| std::cmp::Reverse(g.0));",
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
        breaks: "the merge of partitions' results takes a tie from the later partition, peers out of their input order (RULES.md D1227)",
        file: "src/exec/parallel/partitioned/merged.rs",
        original: ".then_with(|| other.run.cmp(&self.run))",
        replacement: ".then_with(|| self.run.cmp(&other.run))",
        integration: &[],
        unit: &[MERGED],
    },
    Mutation {
        name: "parallel-probe-reversed",
        package: Package::Core,
        breaks: "a probe of a table built by morsels gives a row's matches last built first",
        file: "src/exec/batch/joined/shared/probing.rs",
        original: "        for p in candidates {",
        replacement: "        for p in candidates.collect::<Vec<usize>>().into_iter().rev() {",
        integration: &["join_tables::matches_come_in_build_order_in_parallel"],
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
        original: "Merged::of(outs, &keys)",
        replacement: "Merged::of(outs, &[])",
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
        file: "src/index/blink/range.rs",
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
        original: "ceiling.min(share.saturating_sub(held) >> 1_u32)",
        replacement: "ceiling.min(share.saturating_sub(held))",
        integration: &[],
        unit: &[GRANTS],
    },
    Mutation {
        name: "parallel-column-store-by-morsels",
        package: Package::Core,
        breaks: "a column store's scan, which a serial scan by batch serves better, is run in parallel",
        file: "src/physical/parallel.rs",
        original: "db.storage_of(table).is_ok_and(|s| s.keeps_columns())",
        replacement: "db.storage_of(table).is_ok_and(|s| s.keeps_columns() && false)",
        integration: &[PLANNER],
        unit: &[],
    },
    Mutation {
        name: "grant-covered-reserved-again",
        package: Package::Core,
        breaks: "a parallel part's operators reserve the budget again beside the statement's grant",
        file: "src/spill/grant.rs",
        original: "if self.covered {",
        replacement: "if false {",
        integration: &[],
        unit: &[COVERED],
    },
    Mutation {
        name: "parallel-built-row-by-row",
        package: Package::Core,
        breaks: "a hash join's operand that gives batches is built of rows made one by one (D1050)",
        file: "src/exec/parallel/probe.rs",
        original: "layout(right, db).filter(|_| crate::exec::batched(db) && built_by_batch(right, db))",
        replacement: "layout(right, db).filter(|_| crate::exec::batched(db) && !built_by_batch(right, db))",
        integration: &[],
        unit: &[BUILT],
    },
    Mutation {
        name: "parallel-built-column-store-serial",
        package: Package::Core,
        breaks: "a hash join's built operand reading the column store stays on the statement's thread (D1051)",
        file: "src/physical/parallel.rs",
        original: "|rows| asked.by_rows(rows)",
        replacement: "|rows| asked.degree(plan, rows)",
        integration: &[BUILDS],
        unit: &[],
    },
    Mutation {
        name: "parallel-morsel-zones-unjudged",
        package: Package::Core,
        breaks: "a column store's morsel reads its tree's rows of every zone, not those its split judged (D1052)",
        file: "src/engine/rows/split.rs",
        original: "(self.reader, part), what, self.zones_within(range))",
        replacement: "(self.reader, part), what, vec![range])",
        integration: &[],
        unit: &["engine::rows::frozen::tests::morsels::a_split_judges_its_zones_once_for_every_morsel"],
    },
    Mutation {
        name: "joined-datetimes-of-two-types",
        package: Package::Core,
        breaks: "datetimes of two types are joined as one vector of the first's type (D1053)",
        file: "src/storage/batch/joined.rs",
        original: "parts.iter().all(|&(column, _)| kind(column) == Some(first)).then_some(first)",
        replacement: "Some(first)",
        integration: &[],
        unit: &[JOINED],
    },
    Mutation {
        name: "joined-nulls-not-looked-for",
        package: Package::Core,
        breaks: "a joined vector of datetimes loses the nulls of a part that holds some (D1053)",
        file: "src/storage/batch/joined.rs",
        original: "Column::Datetimes(datetimes) => datetimes.nulls.is_some(),",
        replacement: "Column::Datetimes(_) => false,",
        integration: &[],
        unit: &[JOINED],
    },
    Mutation {
        name: "looped-full-join-takes-part",
        package: Package::Core,
        breaks: "a FULL join by an index, whose unmatched rows come last, probes on a part's threads (D1114)",
        file: "src/exec/parallel/looped.rs",
        original: "let gives_as_read = !matches!(outer, Preserved::Both(_));",
        replacement: "let gives_as_read = true;",
        integration: &[],
        unit: &["exec::parallel::looped::tests::an_index_join_over_a_part_probes_on_its_threads"],
    },
    Mutation {
        name: "looped-chain-cut",
        package: Package::Core,
        breaks: "of a chain of index joins over a part, the joins below the first stay on the statement's thread (D1114)",
        file: "src/exec/parallel/looped.rs",
        original: "if depth < MAX_DEPTH && joins_alone(left) {",
        replacement: "if depth > MAX_DEPTH && joins_alone(left) {",
        integration: &[],
        unit: &["exec::parallel::looped::tests::the_left_operand_of_the_lowest_join_is_taken_out"],
    },
    Mutation {
        name: "looped-left-error-lost",
        package: Package::Core,
        breaks: "an index join over a part drops the error its left operand raised (D1114)",
        file: "src/exec/parallel/looped.rs",
        original: "return self.failed.take().map(Err);",
        replacement: "return None;",
        integration: &["looped_parts::an_index_join_over_a_part_raises_as_alone"],
        unit: &[],
    },
    Mutation {
        name: "held-left-out-of-order",
        package: Package::Core,
        breaks: "a semi join's left held by its parts holds each morsel's rows before those of the morsels before it (D1115)",
        file: "src/exec/parallel/held.rs",
        original: "        parts.push(batch);\n        bytes = bytes.saturating_add(held);",
        replacement: "        parts.insert(0, batch);\n        bytes = bytes.saturating_add(held);",
        integration: &["held_left::a_left_held_by_its_parts_gives_its_rows_in_order"],
        unit: &[],
    },
    Mutation {
        name: "held-left-read-dropped",
        package: Package::Core,
        breaks: "a semi join's left held by its parts past the work area drops the batches it read before (D1115)",
        file: "src/exec/parallel/held.rs",
        original: "Box::new(given.into_iter().chain(left))",
        replacement: "Box::new(given.into_iter().take(0).chain(left))",
        integration: &["held_left::a_left_held_by_its_parts_past_the_work_area_is_read_once"],
        unit: &[],
    },
    Mutation {
        name: "held-left-error-lost",
        package: Package::Core,
        breaks: "a semi join's left held by its parts past the work area drops the error of a morsel taken after (D1115)",
        file: "src/exec/parallel/held.rs",
        original: "            Ok(Taken::Past) => again(m),\n            Err(e) => Box::new(std::iter::once(Err(e))),",
        replacement: "            Ok(Taken::Past) => again(m),\n            Err(_) => Box::new(std::iter::empty()),",
        integration: &["held_left::a_left_held_by_its_parts_raises_as_alone"],
        unit: &[],
    },
    Mutation {
        name: "held-left-past-morsel-dropped",
        package: Package::Core,
        breaks: "a semi join's left held by its parts drops a morsel past the work area, not reading it again (D1115)",
        file: "src/exec/parallel/held.rs",
        original: "Ok(Taken::Past) => again(m),",
        replacement: "Ok(Taken::Past) => Box::new(std::iter::empty()),",
        integration: &["held_left::a_left_held_by_its_parts_past_the_work_area_is_read_once"],
        unit: &[],
    },
    Mutation {
        name: "parallel-merged-first-list-skipped",
        package: Package::Core,
        breaks: "the first partition's result is left out of the merge (RULES.md D1227)",
        file: "src/exec/parallel/partitioned/merged.rs",
        original: "for run in 0..merged.runs.len() {",
        replacement: "for run in 1..merged.runs.len() {",
        integration: &[SPILLED_WINDOWS],
        unit: &[],
    },
    Mutation {
        name: "parallel-sort-ties-dropped",
        package: Package::Core,
        breaks: "a sort run in parallel that asks for its last row's peers gives its first rows alone (RULES.md D1229)",
        file: "src/exec/parallel/sorted.rs",
        original: "Box::new(WithTies { rows, left: n, keys, last: None })",
        replacement: "Box::new(rows.take(n))",
        integration: &[SAME_ROWS],
        unit: &[],
    },
];
