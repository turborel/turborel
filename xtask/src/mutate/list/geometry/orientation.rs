//! The mutations of exact orientation by floating-point expansions
//! (RULES.md R-TEST-9, D85): a sign taken from a leading term that
//! cancelled, a zero factor no longer deciding its product, and a rounded
//! difference accepted as exact.

use super::{Mutation, Package};

/// The file of the expansion stage.
const EXPANSION: &str = "src/geometry/robust/expansion.rs";
/// The unit test of signs given or withheld for any coordinates.
const EXACT_OR_ABSENT: &str = "geometry::robust::expansion::tests::expansion_signs_are_exact_or_absent";

/// Mechanisms of the expansion stage whose removal must fail its tests.
pub const ORIENTATION: [Mutation; 3] = [
    Mutation {
        name: "geometry-expansion-largest-term",
        package: Package::Core,
        breaks: "an orientation expansion takes its sign from its leading term even when that term cancelled to zero",
        file: EXPANSION,
        original: "let largest = [x3, x2, x1, x0].into_iter().find(|x| x.abs() > 0.0_f64);",
        replacement: "let largest = Some(x3);",
        integration: &[],
        unit: &["geometry::robust::expansion::tests::cancelled_products_take_their_errors_sign"],
    },
    Mutation {
        name: "geometry-expansion-zero-factor",
        package: Package::Core,
        breaks: "an exact zero difference no longer makes its product zero when the other difference is inexact",
        file: EXPANSION,
        original: "if zero(a) || zero(b) {",
        replacement: "if zero(a) && zero(b) {",
        integration: &[],
        unit: &[EXACT_OR_ABSENT],
    },
    Mutation {
        name: "geometry-expansion-inexact-difference",
        package: Package::Core,
        breaks: "an orientation expansion accepts a coordinate difference that was rounded",
        file: EXPANSION,
        original: "(x.is_finite() && error.abs() <= 0.0_f64).then_some(x)",
        replacement: "x.is_finite().then_some(x)",
        integration: &[],
        unit: &[EXACT_OR_ABSENT],
    },
];
