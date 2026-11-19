//! The mutations of bucketed edge intersection (RULES.md R-TEST-9, D85):
//! a probe finding no filed edges in the cells it covers, and an edge
//! touching the overlap's border left unfiled.

use super::{Mutation, Package};

/// The file of the buckets.
const BUCKETS_FILE: &str = "src/geometry/crossing/buckets.rs";

/// Mechanisms of the buckets whose removal must fail their tests.
pub const BUCKETS: [Mutation; 2] = [
    Mutation {
        name: "buckets-cell-range",
        package: Package::Core,
        breaks: "a probe edge finds none of the edges filed under the cells it covers",
        file: BUCKETS_FILE,
        original: "let to = *self.starts.get(base.checked_add(last)?.checked_add(1)?)?;",
        replacement: "let to = from;",
        integration: &[],
        unit: &["geometry::crossing::buckets::tests::buckets_agree_with_nested_loops"],
    },
    Mutation {
        name: "buckets-closed-overlap",
        package: Package::Core,
        breaks: "an edge lying on the border of the envelopes' overlap is not filed or probed",
        file: BUCKETS_FILE,
        original: "ax.min(bx) <= x1 && ax.max(bx) >= x0 && ay.min(by) <= y1 && ay.max(by) >= y0",
        replacement: "ax.min(bx) < x1 && ax.max(bx) > x0 && ay.min(by) <= y1 && ay.max(by) >= y0",
        integration: &[],
        unit: &["geometry::crossing::buckets::tests::touching_edges_meet_and_interleaved_combs_do_not"],
    },
];
