//! The mutations of the `<filter clause>` (RULES.md R-TEST-9): the filter
//! ignored by groups row by row, by a window's frames and by a nested
//! window function's walk, a row the filter is unknown of taken, a
//! filtered set function read by batches, a column a grouping set leaves
//! out read as it is, and SR8's outer reference taken.

use super::{Mutation, Package};

/// The test of grouped filtered set functions against CASE.
const GROUPED: &str = "filter::grouped_set_functions_take_the_rows_filtered_10_9";
/// The test of window filtered set functions against CASE.
const WINDOWED: &str = "filter::window_set_functions_take_the_rows_filtered_10_9";

/// The mutations of the `<filter clause>`, in the order they run.
pub const FILTER: [Mutation; 7] = [
    Mutation {
        name: "filter-ignored-by-rows",
        package: Package::Core,
        breaks: "a group's set function read row by row is fed every row, not those its filter is true of [§10.9 GR4]",
        file: "src/exec/group.rs",
        original: "        if !admitted(a.filter.as_ref(), row, runner)? {",
        replacement: "        if !admitted(None, row, runner)? {",
        integration: &[GROUPED],
        unit: &[],
    },
    Mutation {
        name: "filter-ignored-by-frames",
        package: Package::Core,
        breaks: "a window's set function is fed every row of its frame, not those its filter is true of [§10.9 GR4]",
        file: "src/exec/window/aggregate.rs",
        original: "    if call.filter.is_none() {",
        replacement: "    if call.filter.is_some() || call.filter.is_none() {",
        integration: &[WINDOWED],
        unit: &[],
    },
    Mutation {
        name: "filter-ignored-by-nested-walk",
        package: Package::Core,
        breaks: "a set function of nested window functions computed for each frame is fed every row of it, not those its filter is true of [§10.9 GR4]",
        file: "src/exec/window/nested.rs",
        original: "                if !context.admits(self.call.filter.as_ref(), r2, part, runner)? {",
        replacement: "                if !context.admits(None, r2, part, runner)? {",
        integration: &[WINDOWED],
        unit: &[],
    },
    Mutation {
        name: "filter-unknown-taken",
        package: Package::Core,
        breaks: "a row a filter is unknown of is aggregated, though only one it is true of is [§10.9 GR4]",
        file: "src/exec/group.rs",
        original: "        Some(c) => Ok(eval_cond(c, row, runner)? == Truth::True),",
        replacement: "        Some(c) => Ok(eval_cond(c, row, runner)? != Truth::False),",
        integration: &[GROUPED, WINDOWED],
        unit: &[],
    },
    Mutation {
        name: "filter-read-by-batches",
        package: Package::Core,
        breaks: "a filtered set function is read by batches, which feed it every row [§10.9 GR4]",
        file: "src/exec/batch.rs",
        original: "        .map(|a| match (&a.arg, a.distinct || a.filter.is_some() || a.distribution.is_some()) {",
        replacement: "        .map(|a| match (&a.arg, a.distinct || a.distribution.is_some()) {",
        integration: &[GROUPED],
        unit: &[],
    },
    Mutation {
        name: "filter-grouping-unmasked",
        package: Package::Core,
        breaks: "a filter reads a column its grouping set leaves out as it is, not null [§7.13 SR17 m iii 1 A II]",
        file: "src/binder/grouping_sets.rs",
        original: "        filter: a.filter.map(|c| map_cond_columns(c, &value)),",
        replacement: "        filter: a.filter.map(|c| map_cond_columns(c, &|c| c)),",
        integration: &["filter::filters_in_grouping_sets_statements_and_views_10_9"],
        unit: &[],
    },
    Mutation {
        name: "filter-outer-reference-taken",
        package: Package::Core,
        breaks: "a <filter clause> with an outer reference is taken [§10.9 SR8]",
        file: "src/binder/filter.rs",
        original: "        if self.filter_columns(c)?.1 {",
        replacement: "        if false && self.filter_columns(c)?.1 {",
        integration: &["filter::filters_misused_are_refused_10_9"],
        unit: &[],
    },
];
