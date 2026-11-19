//! The mutations of a prepared polygon's filed edges (RULES.md R-TEST-9,
//! D85): a polygon inside the prepared one, its edges meeting none, taken
//! as apart.

use super::{Mutation, Package};

/// Mechanisms of the filed edges whose removal must fail their tests.
pub const FILED: [Mutation; 1] = [Mutation {
    name: "filed-containment",
    package: Package::Core,
    breaks: "a polygon inside a prepared polygon, whose edges meet none of its edges, is taken as apart",
    file: "src/geometry/prepared/shapes.rs",
    original: "Some(inside_prepared || inside_other)",
    replacement: "Some(inside_other)",
    integration: &[],
    unit: &["geometry::prepared::shapes::tests::filed_edges_decide_as_the_relationship_does"],
}];
