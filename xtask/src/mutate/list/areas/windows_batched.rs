//! The mutations of windows over batches (RULES.md R-TEST-9, D560): a
//! boundary of keys alike, the peer groups found of the partitioning's
//! bytes alone, a partition's last peer group, a frame's prefix sum off
//! by one, and the place LAG takes under IGNORE NULLS.

use super::super::{Mutation, Package};

/// The kernels' contract, run against the portable ones.
const CONTRACT: &str = "kernels::portable_kernels_keep_the_contract";
/// Windows over batches against the same windows a row at a time.
const BOTH_WAYS: &str = "window_batches::windows_over_batches_are_those_of_rows";
/// Frames found at once against frames found row after row.
const FRAMES: &str = "exec::window::batched::frames::tests::frames_found_at_once_are_those_found_row_after_row";

/// Mechanisms whose removal must fail the requirements of windows over
/// batches.
pub const WINDOWS_BATCHED: [Mutation; 5] = [
    Mutation {
        name: "window-boundary-alike",
        package: Package::Core,
        breaks: "a key alike to the one before it is taken for a boundary, and one that differs is not",
        file: "src/kernels/portable/boundaries.rs",
        original: "        if before == after {",
        replacement: "        if before != after {",
        integration: &[CONTRACT, BOTH_WAYS],
        unit: &[],
    },
    Mutation {
        name: "window-peers-of-partitions",
        package: Package::Core,
        breaks: "peer groups are found of the partitioning's bytes, not of the ordering's too",
        file: "src/exec/window/batched/boundaries.rs",
        original: "        kernels.boundaries(&k.bytes, k.width, (0, k.width), &mut words);",
        replacement: "        kernels.boundaries(&k.bytes, k.width, (0, split), &mut words);",
        integration: &[BOTH_WAYS],
        unit: &[],
    },
    Mutation {
        name: "window-last-peer-group",
        package: Package::Core,
        breaks: "a partition's last peer group has no rows, its rows counted in the group before",
        file: "src/exec/window/batched/frames.rs",
        original: "        starts.push(len);\n        let mut group",
        replacement: "        let mut group",
        integration: &[BOTH_WAYS],
        unit: &[FRAMES],
    },
    Mutation {
        name: "window-prefix-off-by-one",
        package: Package::Core,
        breaks: "a frame's sum ends a row before its last",
        file: "src/exec/window/batched/sets.rs",
        original: "sums.get(to).copied().unwrap_or(zero)",
        replacement: "sums.get(to.saturating_sub(1)).copied().unwrap_or(zero)",
        integration: &[BOTH_WAYS],
        unit: &[],
    },
    Mutation {
        name: "window-lag-ignore-nulls",
        package: Package::Core,
        breaks: "LAG under IGNORE NULLS counts the current row's extent among those before it",
        file: "src/exec/window/batched/picks.rs",
        original: "                    n.before(row).checked_sub(offset)",
        replacement: "                    n.before(row.saturating_add(1)).checked_sub(offset)",
        integration: &[BOTH_WAYS],
        unit: &[],
    },
];
