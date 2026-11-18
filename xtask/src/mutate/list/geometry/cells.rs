//! The mutations of a prepared polygon's cell certificates (RULES.md
//! R-TEST-9, D85): a row's crossings no longer flipping their rings'
//! parity, and holes no longer taking a point out of their polygon.

use super::{Mutation, Package};

/// The file of the certificates.
const CELLS_FILE: &str = "src/geometry/prepared/cells.rs";
/// The unit test comparing every decision with exact location.
const AGREE: &str = "geometry::prepared::cells::tests::certificates_agree_with_exact_location";

/// Mechanisms of the certificates whose removal must fail their tests.
pub const CELLS: [Mutation; 2] = [
    Mutation {
        name: "cells-crossing-parity",
        package: Package::Core,
        breaks: "a cell certificate labels every unmarked cell as if no crossing lay right of it",
        file: CELLS_FILE,
        original: "parity.flip(crossing.ring)?;",
        replacement: "let _unflipped = crossing;",
        integration: &[],
        unit: &[AGREE],
    },
    Mutation {
        name: "cells-holes-ignored",
        package: Package::Core,
        breaks: "a cell certificate labels a point inside a hole as inside its polygon",
        file: CELLS_FILE,
        original: "let is = state.0 && state.1 == 0;",
        replacement: "let is = state.0;",
        integration: &[],
        unit: &[AGREE],
    },
];
