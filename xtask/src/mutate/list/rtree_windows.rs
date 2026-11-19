//! The mutations of geometry windows beyond intersection (RULES.md
//! R-TEST-9, D135): relate patterns read as meeting, distance windows
//! unscaled, pruned without a certificate or kept past their circle, read
//! regions not widened, outer joins that lose their nulls or a FULL join's
//! matches or filter, and null calls of shared names taken by §15.

use super::{Mutation, Package};

const DISTANCE: &str = "index::rtree::tests::within::distance_windows_find_within_their_limits";
const OUTER: &str = "geometry::index::outer::geometry_outer_joins_keep_unmatched_rows_and_equal_products";

/// The mutations of geometry windows, in the order they run.
pub const RTREE_WINDOWS: [Mutation; 9] = [
    Mutation {
        name: "relate-pattern-always-meets",
        package: Package::Core,
        breaks: "a relate pattern disjoint values may match reads a window and drops their rows",
        file: "src/index/rtree/geometry.rs",
        original: "    if !window::pattern_meets(pattern) {",
        replacement: "    if !window::pattern_meets(pattern) && false {",
        integration: &[],
        unit: &["index::rtree::tests::within::relate_windows_read_their_patterns"],
    },
    Mutation {
        name: "distance-radius-unscaled",
        package: Package::Core,
        breaks: "a distance in another unit is read as the column's unit",
        file: "src/spatial/window.rs",
        original: "let units = limit.max(RADIUS_FLOOR) / scale * (1.0_f64 + RELATIVE_SLACK);",
        replacement: "let units = limit.max(RADIUS_FLOOR) / 1.0_f64.max(*scale) * (1.0_f64 + RELATIVE_SLACK);",
        integration: &[],
        unit: &[DISTANCE],
    },
    Mutation {
        name: "distance-uncertified-pruned",
        package: Package::Core,
        breaks: "a tree whose distances may overflow is pruned, hiding 22003",
        file: "src/index/rtree/geometry/within.rs",
        original: "tree.within_domain(bounds, domain))?;",
        replacement: "tree.within_domain(bounds, domain).map(|_| true))?;",
        integration: &[],
        unit: &["index::rtree::tests::within::uncertified_distance_windows_read_every_entry"],
    },
    Mutation {
        name: "distance-corner-kept",
        package: Package::Core,
        breaks: "a row in the window's corner, beyond the limit, is a candidate",
        file: "src/index/rtree/geometry/within.rs",
        original: "found.distance_lower_bound(bounds) > radius",
        replacement: "found.distance_lower_bound(bounds) > radius * 2.0_f64",
        integration: &[],
        unit: &[DISTANCE],
    },
    Mutation {
        name: "distance-region-not-widened",
        package: Package::Core,
        breaks: "a distance read records the value's envelope alone, missing writes within the limit",
        file: "src/index/rtree/geometry/within.rs",
        original: "Read::Window(window, _) => Some(vec![window]),",
        replacement: "Read::Window(_, _) => Some(vec![bounds]),",
        integration: &["geometry::index::widened::distance_reads_record_their_widened_window"],
        unit: &[],
    },
    Mutation {
        name: "outer-join-no-nulls",
        package: Package::Core,
        breaks: "an outer row no table row matches is dropped, as an inner join drops it",
        file: "src/exec/join/indexed.rs",
        original: "nulls: (!inner).then(|| vec![Value::Null; width]),",
        replacement: "nulls: false.then(|| vec![Value::Null; width]),",
        integration: &[OUTER],
        unit: &[],
    },
    Mutation {
        name: "full-join-forgets-matches",
        package: Package::Core,
        breaks: "a FULL join keeps with nulls the table rows its probes matched",
        file: "src/exec/join/indexed/preserved.rs",
        original: "if self.rows.contains(&id) {",
        replacement: "if self.rows.contains(&id) && false {",
        integration: &[OUTER],
        unit: &[],
    },
    Mutation {
        name: "full-join-filter-ignored",
        package: Package::Core,
        breaks: "a FULL join keeps the table rows its derived table's filter drops",
        file: "src/exec/join/indexed/preserved.rs",
        original: "Some(filter) => eval_cond(filter, &joined, runner)? == Truth::True,",
        replacement: "Some(filter) => eval_cond(filter, &joined, runner)? != Truth::Unknown,",
        integration: &[OUTER],
        unit: &[],
    },
    Mutation {
        name: "shared-name-null-call",
        package: Package::Core,
        breaks: "a call of nulls alone of ST_EQUALS raises a §15 routine's 2FF03",
        file: "src/spatial/lr/sql/dispatch.rs",
        original: "if self.fallback.is_some() && values.iter().all(|v| v.borrow().is_null()) {",
        replacement: "if self.fallback.is_some() && values.iter().all(|v| v.borrow().is_null()) && false {",
        integration: &["geometry::index::outer::null_calls_of_shared_names_are_null"],
        unit: &[],
    },
];
