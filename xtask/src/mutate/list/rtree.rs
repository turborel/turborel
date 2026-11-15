//! Mutations of spatial bounds, split traversal and MVCC reference counts
//! (RULES.md R-TEST-9, D72).

use super::{Mutation, Package};

/// Independent mechanisms whose removal must fail spatial requirements.
pub const RTREE: [Mutation; 5] = [
    Mutation {
        name: "rtree-split-stamp",
        package: Package::Core,
        breaks: "a scan ignores splits after queuing a child and misses entries moved to its right sibling",
        file: "src/engine/rtree/scan.rs",
        original: "if node.stamp != visit.stamp {",
        replacement: "if node.stamp == visit.stamp && node.stamp != visit.stamp {",
        integration: &[],
        unit: &["engine::rtree::tests::a_scan_survives_splits_without_missing_or_repeating_old_entries"],
    },
    Mutation {
        name: "rtree-closed-boundary",
        package: Package::Core,
        breaks: "an intersection excludes points touching its lower X boundary",
        file: "src/spatial.rs",
        original: "self.min_x <= other.max_x && other.min_x <= self.max_x",
        replacement: "self.min_x < other.max_x && other.min_x <= self.max_x",
        integration: &[],
        unit: &["spatial::tests::touching_points_and_edges_intersect"],
    },
    Mutation {
        name: "rtree-version-count",
        package: Package::Core,
        breaks: "removing one version deletes an entry still held by another version",
        file: "src/engine/rtree/write.rs",
        original: "} else if entry.value > 1 {",
        replacement: "} else if entry.value > u64::MAX {",
        integration: &[],
        unit: &["engine::rtree::tests::reference_counts_preserve_versions_and_absent_removes_do_nothing"],
    },
    Mutation {
        name: "rtree-nearest-priority",
        package: Package::Core,
        breaks: "nearest traversal expands the most distant frontier item first",
        file: "src/engine/rtree/nearest.rs",
        original: "other.distance.total_cmp(&self.distance)",
        replacement: "self.distance.total_cmp(&other.distance)",
        integration: &[],
        unit: &["engine::rtree::tests::nearest::nearest_points_match_exhaustive_sorting"],
    },
    Mutation {
        name: "rtree-distance-lower-bound",
        package: Package::Core,
        breaks: "a branch distance bound overestimates the distance of a nearer child",
        file: "src/spatial.rs",
        original: "down(down(squared.sqrt()) * scale)",
        replacement: "down(down(squared.sqrt()) * scale) + scale",
        integration: &[],
        unit: &["spatial::tests::distances_bound_the_geometry_at_extreme_scales"],
    },
];
