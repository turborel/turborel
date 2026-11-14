//! The mutations of framed COUNT DISTINCT (RULES.md R-TEST-9): the rows a
//! frame left not taken away, the rows after an exclusion's hole not
//! kept, the filter ignored, nulls not told of or never taken away, a
//! value's count raised or lowered but its distinct values miscounted,
//! and SR8d ii's refusal of the other DISTINCT set functions dropped.

use super::{Mutation, Package};

/// The file of the sliding count.
const SLIDING: &str = "src/exec/window/aggregate/sliding.rs";
/// The unit test of the sliding count against each frame's rows.
const UNIT: &str = "exec::window::aggregate::tests::count_distinct_over_frames_that_move_7_15";
/// The test of ROWS, RANGE and GROUPS frames.
const FRAMES: &str = "window_distinct::framed_count_distinct_counts_each_value_once_9_23";
/// The test of exclusions.
const EXCLUSIONS: &str = "window_distinct::exclusions_leave_values_out_7_15";
/// The test of filters, collations, nested window functions and nulls.
const FILTERS: &str = "window_distinct::filters_collations_nested_and_nulls_10_9";
/// The test past the work area.
const SPILLED: &str = "window_distinct::framed_count_distinct_past_the_work_area_9_23";

/// The mutations of framed COUNT DISTINCT, in the order they run.
pub const WINDOW_DISTINCT: [Mutation; 8] = [
    Mutation {
        name: "sliding-rows-left-in",
        package: Package::Core,
        breaks: "COUNT(DISTINCT ...) over a frame that moves counts the values of rows the frame has left [§10.9 GR4b]",
        file: SLIDING,
        original: "                    (false, true) => self.count((call, slot), at, part, false)?,",
        replacement: "                    (false, true) => {}",
        integration: &[FRAMES, EXCLUSIONS, SPILLED],
        unit: &[UNIT],
    },
    Mutation {
        name: "sliding-frame-cut-short",
        package: Package::Core,
        breaks: "of a frame that an exclusion splits, the rows after the hole are added again for each frame and never taken away [§7.15 GR5b ii, §10.9 GR4b]",
        file: SLIDING,
        original: "        self.held = *frame;",
        replacement: "        self.held = [frame[0], (0, 0), (0, 0)];",
        integration: &[EXCLUSIONS],
        unit: &[UNIT],
    },
    Mutation {
        name: "sliding-filter-ignored",
        package: Package::Core,
        breaks: "a filtered COUNT(DISTINCT ...) over a frame that moves counts the rows its filter is not true of [§10.9 GR4b]",
        file: SLIDING,
        original: "        if !admitted(call, slot, values)? {",
        replacement: "        if false && !admitted(call, slot, values)? {",
        integration: &[FILTERS],
        unit: &[],
    },
    Mutation {
        name: "sliding-nulls-untold",
        package: Package::Core,
        breaks: "a null left out of a COUNT(DISTINCT ...) over a frame that moves warns no 01003 [§10.9 GR4b]",
        file: SLIDING,
        original: "        Ok((cast(count, &call.result)?, self.nulls > 0))",
        replacement: "        Ok((cast(count, &call.result)?, false))",
        integration: &[FILTERS],
        unit: &[UNIT],
    },
    Mutation {
        name: "sliding-nulls-kept",
        package: Package::Core,
        breaks: "a frame that has left a row of a null value still tells of its null [§10.9 GR4b]",
        file: SLIDING,
        original: "if adding { self.nulls.saturating_add(1) } else { self.nulls.saturating_sub(1) }",
        replacement: "if adding { self.nulls.saturating_add(1) } else { self.nulls }",
        integration: &[],
        unit: &[UNIT],
    },
    Mutation {
        name: "bag-raise-always-new",
        package: Package::Core,
        breaks: "each row of a value already counted is counted as a value of its own [§10.9 GR4b]",
        file: "src/exec/work.rs",
        original: "        self.set(key, (n, n.saturating_add(1)))?;\n        Ok(n == 0)",
        replacement: "        self.set(key, (n, n.saturating_add(1)))?;\n        Ok(true)",
        integration: &[FRAMES, EXCLUSIONS, FILTERS, SPILLED],
        unit: &[UNIT],
    },
    Mutation {
        name: "bag-lower-always-gone",
        package: Package::Core,
        breaks: "a value is no longer counted once any of its rows has left the frame, though others are in it [§10.9 GR4b]",
        file: "src/exec/work.rs",
        original: "        self.set(key, (n, n.saturating_sub(1)))?;\n        Ok(n == 1)",
        replacement: "        self.set(key, (n, n.saturating_sub(1)))?;\n        Ok(true)",
        integration: &[FRAMES, EXCLUSIONS, SPILLED],
        unit: &[UNIT],
    },
    Mutation {
        name: "framed-distinct-all-taken",
        package: Package::Core,
        breaks: "a DISTINCT set function but COUNT is taken over a window of an ordering or a frame [§9.23 SR8d ii]",
        file: "src/binder/window/spec.rs",
        original: "    if !matches!(sf.arg, SetArg::Distinct(_)) || sf.name == kw::COUNT {",
        replacement: "    if true || !matches!(sf.arg, SetArg::Distinct(_)) || sf.name == kw::COUNT {",
        integration: &["window_distinct::other_distinct_set_functions_stay_refused_9_23"],
        unit: &[],
    },
];
