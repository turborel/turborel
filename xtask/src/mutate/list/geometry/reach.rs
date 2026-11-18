//! The mutations of the expanding nearest-edge search (RULES.md
//! R-TEST-9, D85): a search window that no longer reaches past an edge's
//! own envelope.

use super::{Mutation, Package};

/// Mechanisms of the search whose removal must fail its tests.
pub const REACH: [Mutation; 1] = [Mutation {
    name: "distance-search-reach",
    package: Package::Core,
    breaks: "a nearest-edge search looks only at edges whose envelopes touch the searching edge's",
    file: "src/geometry/distance/near.rs",
    original: "let reach = best + best * SLACK + unit * UNITS;",
    replacement: "let reach = 0.0_f64;",
    integration: &[],
    unit: &["geometry::distance::near::tests::searches_return_the_nested_minimum_to_the_bit"],
}];
