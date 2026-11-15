//! Mutations of spatial bounds, split traversal and MVCC reference counts
//! (RULES.md R-TEST-9, D72).

use super::{Mutation, Package};

/// Independent mechanisms whose removal must fail spatial requirements.
pub const RTREE: [Mutation; 3] = [
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
];
