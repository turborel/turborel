//! The mutations of a prepared polygon's cell certificates (RULES.md
//! R-TEST-9, D85): a row's crossings no longer flipping their rings'
//! parity, holes no longer taking a point out of their polygon, a
//! crossing placed in the first column it may touch, a boundary cell's
//! reference side inverted or shared by several rings, another polygon's
//! hold ignored, the run and the rise near a point ignored, edges
//! through the corners of its path misplaced, and the interval trees'
//! work no longer counted toward building them.

use super::{Mutation, Package};

/// The files of the certificates and of locating a point near its edges.
const CELLS_FILE: &str = "src/geometry/prepared/cells.rs";
const PATH_FILE: &str = "src/geometry/prepared/cells/path.rs";
/// The unit tests comparing every decision with exact location.
const AGREE: &str = "geometry::prepared::cells::tests::certificates_agree_with_exact_location";
const FINE: &str = "geometry::prepared::cells::tests::edges_near_a_point_decide_cells_finer_teeth_cross";
const TIES: &str = "geometry::prepared::cells::tests::ties_on_a_points_path_are_placed_by_slope";
const INVALID: &str = "geometry::prepared::cells::tests::invalid_topology_is_labelled_as_location_reads_it";

/// Mechanisms of the certificates whose removal must fail their tests.
pub const CELLS: [Mutation; 14] = [
    Mutation {
        name: "cells-crossing-parity",
        package: Package::Core,
        breaks: "a cell certificate labels every unmarked cell as if no crossing lay right of it",
        file: CELLS_FILE,
        original: "self.flip(ring)?;",
        replacement: "let _unflipped = ring;",
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
    Mutation {
        name: "cells-crossing-column",
        package: Package::Core,
        breaks: "a crossing of a centre line is placed in the first column its edge may touch, not its own",
        file: CELLS_FILE,
        original: "    Some(low)\n}",
        replacement: "    Some(first)\n}",
        integration: &[],
        unit: &[AGREE, FINE],
    },
    Mutation {
        name: "cells-near-reference",
        package: Package::Core,
        breaks: "a boundary cell of a shell takes the opposite side for its reference point",
        file: CELLS_FILE,
        original: "if odd_holes == 0 { near(odd) } else { BOUNDARY }",
        replacement: "if odd_holes == 0 { near(!odd) } else { BOUNDARY }",
        integration: &[],
        unit: &[AGREE, FINE],
    },
    Mutation {
        name: "cells-near-hole",
        package: Package::Core,
        breaks: "a boundary cell of a hole takes the hole's side for its polygon's",
        file: CELLS_FILE,
        original: "if odd_holes == usize::from(odd) { near(!odd) } else { BOUNDARY }",
        replacement: "if odd_holes == usize::from(odd) { near(odd) } else { BOUNDARY }",
        integration: &[],
        unit: &[AGREE, FINE],
    },
    Mutation {
        name: "cells-near-several-rings",
        package: Package::Core,
        breaks: "a cell several rings' edges touch is decided as if only the last ring's did",
        file: CELLS_FILE,
        original: "*owner = if *owner == UNTOUCHED || *owner == edge.ring { edge.ring } else { SEVERAL };",
        replacement: "*owner = if *owner == UNTOUCHED || *owner == edge.ring || SEVERAL > 0 { edge.ring } else { SEVERAL };",
        integration: &[],
        unit: &[INVALID],
    },
    Mutation {
        name: "cells-near-other-polygon",
        package: Package::Core,
        breaks: "a cell inside another part of a multipolygon is decided by its own ring alone",
        file: CELLS_FILE,
        original: "if self.inside > usize::from(shell_odd && odd_holes == 0) {",
        replacement: "if self.inside > usize::from(shell_odd && odd_holes == 0) && self.inside == 0 {",
        integration: &[],
        unit: &[INVALID],
    },
    Mutation {
        name: "cells-near-run",
        package: Package::Core,
        breaks: "a point near its edges ignores those crossing its run to the cell's right side",
        file: PATH_FILE,
        original: "let run = straddles(a, b, y) && across(a, b, self.point) == Ordering::Greater && !beyond(a, b, [self.right, y]);",
        replacement: "let run = straddles(a, b, y) && across(a, b, self.point) == Ordering::Greater && !beyond(a, b, [self.right, y]) && y > y;",
        integration: &[],
        unit: &[AGREE, FINE],
    },
    Mutation {
        name: "cells-near-rise",
        package: Package::Core,
        breaks: "a point near its edges ignores those crossing its rise to the centre line",
        file: PATH_FILE,
        original: "Event::Crosses(run != (!level && self.rise(a, b)))",
        replacement: "Event::Crosses(run != (!level && self.rise(a, b) && level))",
        integration: &[],
        unit: &[AGREE, FINE],
    },
    Mutation {
        name: "cells-corner-slope",
        package: Package::Core,
        breaks: "an edge through a corner of a point's path, or a cell's side on a centre line, is placed right of it whatever its slope",
        file: PATH_FILE,
        original: "Ordering::Equal => !leans_left(a, b),",
        replacement: "Ordering::Equal => true || leans_left(a, b),",
        integration: &[],
        unit: &[TIES],
    },
    Mutation {
        name: "cells-corner-lean",
        package: Package::Core,
        breaks: "an edge through a corner of a point's path is placed by the opposite slope",
        file: PATH_FILE,
        original: "if by > ay { bx < ax } else { bx > ax }",
        replacement: "if by > ay { bx > ax } else { bx < ax }",
        integration: &[],
        unit: &[TIES],
    },
    Mutation {
        name: "cells-rise-side",
        package: Package::Core,
        breaks: "an edge ending on a cell's right side counts as left of the rise beside it",
        file: PATH_FILE,
        original: "if (ax < x) == (bx < x) {",
        replacement: "if (ax <= x) == (bx <= x) {",
        integration: &[],
        unit: &[TIES],
    },
    Mutation {
        name: "cells-rise-height",
        package: Package::Core,
        breaks: "an edge meeting a cell's side exactly at the rise's end counts as above it",
        file: PATH_FILE,
        original: "let above = |h: f64| height(a, b, [x, h]) == Ordering::Greater;",
        replacement: "let above = |h: f64| height(a, b, [x, h]) != Ordering::Less;",
        integration: &[],
        unit: &[TIES],
    },
    Mutation {
        name: "cells-built-by-work",
        package: Package::Core,
        breaks: "a polygon whose rays cross many teeth waits for eight located points per edge before its certificates",
        file: "src/geometry/prepared/locator.rs",
        original: "*self = self.saturating_add(1);",
        replacement: "*self = self.saturating_add(0);",
        integration: &[],
        unit: &["geometry::tests::prepared::certificates_are_built_once_ray_work_repays_them"],
    },
];
