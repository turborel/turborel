//! The mutations of geometry windows beyond intersection (RULES.md
//! R-TEST-9, D135): relate patterns read as meeting, distance windows
//! unscaled, pruned without a certificate or kept past their circle, read
//! regions not widened, outer joins that lose their nulls or a FULL join's
//! matches or filter, null calls of shared names taken by §15, and a
//! nearest read in a unit cut off by bounds left unconverted, farthest
//! reads unplanned or by lower bounds, and WITH TIES uncounted or priced
//! as ONLY.

use super::{Mutation, Package};

const DISTANCE: &str = "index::rtree::tests::within::distance_windows_find_within_their_limits";
const FARTHEST: &str = "geometry::index::nearest_orders::farthest_first_reads_use_upper_bounds";
const ORDERS: &str = "geometry::index::nearest_orders::nearest_reads_serve_ties_offsets_keys_and_filters";
const OUTER: &str = "geometry::index::outer::geometry_outer_joins_keep_unmatched_rows_and_equal_products";

/// The mutations of geometry windows, in the order they run.
pub const RTREE_WINDOWS: [Mutation; 15] = [
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
        original: "let nulls = (first.is_none() && !inner).then(|| vec![Value::Null; width]);",
        replacement: "let nulls = false.then(|| vec![Value::Null; width]);",
        integration: &[OUTER],
        unit: &[],
    },
    Mutation {
        name: "full-join-forgets-matches",
        package: Package::Core,
        breaks: "a FULL join keeps with nulls the table rows its probes matched",
        file: "src/exec/join/indexed/preserved.rs",
        original: "if self.rows.contains(id) {",
        replacement: "if self.rows.contains(id) && false {",
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
    Mutation {
        name: "nearest-bound-unscaled",
        package: Package::Core,
        breaks: "a nearest read in a unit compares the column's bounds with measures in the unit, losing rows tied with the last kept",
        file: "src/exec/nearest.rs",
        original: "if self.threshold.beyond(self.read.bound(bound))? {",
        replacement: "if self.threshold.beyond(bound)? {",
        integration: &["geometry::index::nearest_units::nearest_unit_forms_use_the_index_and_equal_scans"],
        unit: &[],
    },
    Mutation {
        name: "farthest-not-planned",
        package: Package::Core,
        breaks: "ORDER BY a geometry distance DESC reads and sorts the whole table",
        file: "src/physical/nearest.rs",
        original: "descends(measure) || farthest_first(measure)",
        replacement: "descends(measure) || farthest_first(measure) && false",
        integration: &[FARTHEST],
        unit: &[],
    },
    Mutation {
        name: "farthest-read-nearest",
        package: Package::Core,
        breaks: "a farthest-first query reads the tree nearest first",
        file: "src/index/rtree/geometry.rs",
        original: "if query.descending { tree.farthest_candidates",
        replacement: "if query.descending && false { tree.farthest_candidates",
        integration: &[FARTHEST],
        unit: &[],
    },
    Mutation {
        name: "farthest-by-lower-bounds",
        package: Package::Core,
        breaks: "a farthest-first traversal orders by lower bounds, below the distances it bounds",
        file: "src/engine/rtree/nearest.rs",
        original: "Self::UpperBound => -entry.distance_upper_bound(query),",
        replacement: "Self::UpperBound => -entry.distance_lower_bound(query),",
        integration: &[],
        unit: &["engine::rtree::tests::nearest::farthest_candidates_come_in_decreasing_distance"],
    },
    Mutation {
        name: "ties-not-counted",
        package: Package::Core,
        breaks: "FETCH FIRST WITH TIES gives no count to its input, so a nearest read is not planned",
        file: "src/logical/limit.rs",
        original: "let Some(Fetch { count: Expr::Const(Value::Exact(d)), percent: false, .. }) = &self.fetch else { return None };\n        if d.scale != 0 {\n            return None;\n        }\n        usize::try_from(d.units).ok()\n    }\n\n    /// The first",
        replacement: "let Some(Fetch { count: Expr::Const(Value::Exact(d)), percent: false, ties: None, .. }) = &self.fetch else { return None };\n        if d.scale != 0 {\n            return None;\n        }\n        usize::try_from(d.units).ok()\n    }\n\n    /// The first",
        integration: &[ORDERS],
        unit: &[],
    },
    Mutation {
        name: "ties-priced-as-only",
        package: Package::Core,
        breaks: "a path in order below WITH TIES is priced by its count, though its peers are read too",
        file: "src/physical/access.rs",
        original: "let asked = self.first.filter(|_| !want.is_empty() && !tied);",
        replacement: "let asked = self.first.filter(|_| !want.is_empty() || tied);",
        integration: &["fetch_first::fetch_first_prices_a_path_in_order_by_the_rows_it_reads"],
        unit: &[],
    },
];
