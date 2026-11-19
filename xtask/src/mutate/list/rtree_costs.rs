//! The mutations of spatial probes priced by geometry statistics
//! (RULES.md R-TEST-9, D137): envelopes never gathered, a grid ignoring
//! extents, clustering measured backwards, the statistics' rows ignored,
//! evaluation left unpriced for probes or scans, and clustering ignored.

use super::{Mutation, Package};

const BROAD: &str = "geometry::index::costs::broad_windows_are_estimated_from_the_envelope_grid";
const CLUSTERED: &str = "geometry::index::costs::clustered_rows_keep_a_broad_probe_and_scattered_rows_scan";
const VERTICES: &str = "geometry::index::costs::many_vertices_turn_a_scan_into_a_probe";

/// The mutations of spatial pricing by statistics, in the order they run.
pub const RTREE_COSTS: [Mutation; 7] = [
    Mutation {
        name: "envelopes-not-gathered",
        package: Package::Core,
        breaks: "a geometry column's values go to the value sample, so it keeps no envelope statistics",
        file: "src/stats.rs",
        original: "            self.add_geometry(value);",
        replacement: "            self.sample.add(value);",
        integration: &[BROAD, CLUSTERED, VERTICES],
        unit: &[],
    },
    Mutation {
        name: "grid-without-extents",
        package: Package::Core,
        breaks: "the grid takes every envelope for its centre, so windows beside a polygon miss it",
        file: "src/stats/envelopes.rs",
        original: "let (half_x, half_y) = (cell.width / 2.0_f64, cell.height / 2.0_f64);",
        replacement: "let (half_x, half_y) = (cell.width * 0.0_f64, cell.height * 0.0_f64);",
        integration: &[],
        unit: &["stats::envelopes::tests::extents_widen_the_window_and_empties_meet_none"],
    },
    Mutation {
        name: "clustering-backwards",
        package: Package::Core,
        breaks: "clustering measures scattered rows as laid out in space and laid out ones as scattered",
        file: "src/stats/envelopes.rs",
        original: "let ratio = 1.0_f64 - step / pair;",
        replacement: "let ratio = step / pair;",
        integration: &[CLUSTERED],
        unit: &["stats::envelopes::tests::clustering_tells_a_table_laid_out_in_space"],
    },
    Mutation {
        name: "stated-rows-ignored",
        package: Package::Core,
        breaks: "a broad window is estimated by sampling the tree though the statistics know it",
        file: "src/physical/access/geometric.rs",
        original: "stated.map(|s| s.rows).filter(|rows| *rows > COUNTED_ROWS)",
        replacement: "stated.map(|s| s.rows).filter(|_| false)",
        integration: &[BROAD],
        unit: &[],
    },
    Mutation {
        name: "evaluation-unpriced",
        package: Package::Core,
        breaks: "a spatial call costs nothing however many vertices its rows hold",
        file: "src/physical/access/geometric.rs",
        original: "evaluation.per_row += CALL_ROWS + VERTEX_ROWS * vertices;",
        replacement: "evaluation.per_row += 0.0_f64;",
        integration: &[VERTICES],
        unit: &[],
    },
    Mutation {
        name: "scan-evaluation-unpriced",
        package: Package::Core,
        breaks: "a scan pays nothing for the spatial calls it evaluates of every row",
        file: "src/physical/access.rs",
        original: "let seq_cost = seq_cost + scanned * evaluation.per_row;",
        replacement: "let seq_cost = seq_cost + 0.0_f64 * scanned;",
        integration: &[VERTICES],
        unit: &[],
    },
    Mutation {
        name: "clustering-unpriced",
        package: Package::Core,
        breaks: "a probe's rows laid out in space cost a fetch each, as scattered ones do",
        file: "src/physical/access.rs",
        original: "let clustered = geometric::clustered_saving(est, clustering, (storage.fetch_cost(), row));",
        replacement: "let clustered = geometric::clustered_saving(est, 0.0_f64, (storage.fetch_cost(), row));",
        integration: &[CLUSTERED],
        unit: &[],
    },
];
