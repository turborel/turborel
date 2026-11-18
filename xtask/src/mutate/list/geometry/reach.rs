//! The mutations of the expanding nearest-edge searches (RULES.md
//! R-TEST-9, D85): a search window that no longer reaches past the
//! searching edge's or point's own envelope, a point's search that
//! follows only the nearer child of each node, and one that prunes
//! envelopes nearer than its reach.

use super::{Mutation, Package};

/// Mechanisms of the searches whose removal must fail their tests.
pub const REACH: [Mutation; 3] = [
    Mutation {
        name: "distance-search-reach",
        package: Package::Core,
        breaks: "a nearest-edge search looks only at edges whose envelopes touch the searching edge or point",
        file: "src/geometry/distance.rs",
        original: "    best + best * SLACK + unit * UNITS\n",
        replacement: "    let _unwidened = best;\n    0.0_f64 * SLACK * UNITS * unit\n",
        integration: &[],
        unit: &[
            "geometry::distance::near::tests::searches_return_the_nested_minimum_to_the_bit",
            "geometry::prepared::tree::tests::nearest_edges_measure_as_the_scan_does_to_the_bit",
            "geometry::tests::prepared::prepared_point_distances_match_the_scan_to_the_bit",
        ],
    },
    Mutation {
        name: "distance-search-children",
        package: Package::Core,
        breaks: "a point's nearest-edge search follows only the nearer child of each node",
        file: "src/geometry/prepared/tree.rs",
        original: "for next in if nearer { [right, left] } else { [left, right] } {",
        replacement: "for next in if nearer { [left, left] } else { [right, right] } {",
        integration: &[],
        unit: &[
            "geometry::prepared::tree::tests::nearest_edges_measure_as_the_scan_does_to_the_bit",
            "geometry::tests::prepared::prepared_point_distances_match_the_scan_to_the_bit",
        ],
    },
    Mutation {
        name: "distance-search-euclidean",
        package: Package::Core,
        breaks: "a point's nearest-edge search prunes envelopes within its reach",
        file: "src/geometry/prepared/tree.rs",
        original: "gap(point, bounds) > reach * reach * SQUARED_SLACK",
        replacement: "gap(point, bounds) > reach * reach * SQUARED_SLACK * 0.25_f64",
        integration: &[],
        unit: &[
            "geometry::prepared::tree::tests::nearest_edges_measure_as_the_scan_does_to_the_bit",
            "geometry::tests::prepared::prepared_point_distances_match_the_scan_to_the_bit",
        ],
    },
];
