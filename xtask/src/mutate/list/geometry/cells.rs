//! The mutations of a prepared polygon's cell certificates (RULES.md
//! R-TEST-9, D85): a row's crossings no longer flipping their rings'
//! parity, holes no longer taking a point out of their polygon, a
//! crossing placed in the first column it may touch, a boundary cell's
//! reference side inverted or shared by several rings, another polygon's
//! hold ignored, the run and the rise near a point ignored, edges
//! through the corners of its path misplaced, the interval trees' work no
//! longer counted toward building them, a cell number's floor left
//! unchecked where subtracting the shift rounds, and the records of cells
//! several rings touch losing a point's edges, crossings, shell, hole
//! order or another part's hold.

use super::{Mutation, Package};

/// The files of the certificates and of locating a point near its edges.
const CELLS_FILE: &str = "src/geometry/prepared/cells.rs";
const PATH_FILE: &str = "src/geometry/prepared/cells/path.rs";
/// The unit tests comparing every decision with exact location.
const AGREE: &str = "geometry::prepared::cells::tests::certificates_agree_with_exact_location";
const FINE: &str = "geometry::prepared::cells::tests::edges_near_a_point_decide_cells_finer_teeth_cross";
const TIES: &str = "geometry::prepared::cells::tests::ties_on_a_points_path_are_placed_by_slope";
const INVALID: &str = "geometry::prepared::cells::tests::invalid_topology_is_labelled_as_location_reads_it";
const SEVERAL: &str = "geometry::prepared::cells::tests::cells_several_rings_touch_are_decided_by_their_rings";
/// The files of the records of cells several rings touch.
const SEVERAL_FILE: &str = "src/geometry/prepared/cells/several.rs";

/// Mechanisms of the certificates whose removal must fail their tests.
pub const CELLS: [Mutation; 20] = [
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
        original: "near(odd == shell)",
        replacement: "near(odd != shell)",
        integration: &[],
        unit: &[AGREE, FINE],
    },
    Mutation {
        name: "cells-near-hole",
        package: Package::Core,
        breaks: "a boundary cell of a hole takes the hole's side for its polygon's",
        file: CELLS_FILE,
        original: "near(odd == shell)",
        replacement: "near(odd)",
        integration: &[],
        unit: &[AGREE, FINE],
    },
    Mutation {
        name: "cells-near-several-rings",
        package: Package::Core,
        breaks: "a cell several rings' edges touch is decided as if only the last ring's did",
        file: CELLS_FILE,
        original: "if was == UNTOUCHED || was == ring {",
        replacement: "if was == UNTOUCHED || was == ring || ring < usize::MAX {",
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
    Mutation {
        name: "grid-checked-floor",
        package: Package::Core,
        breaks: "a point just below a cell side at a negative coordinate is placed in the cell above",
        file: "src/geometry/grid.rs",
        original: "Some(if floor + SHIFT > scaled { floor - 1.0_f64 } else { floor })",
        replacement: "Some(if floor + SHIFT > scaled { floor } else { floor })",
        integration: &[],
        unit: &["geometry::prepared::cells::tests::cells_are_found_exactly_below_negative_sides"],
    },
    Mutation {
        name: "cells-several-on",
        package: Package::Core,
        breaks: "a point on an edge of a cell several rings touch is placed by its parities alone",
        file: PATH_FILE,
        original: "Event::On => slots.on |= bit,",
        replacement: "Event::On => slots.on |= 0,",
        integration: &[],
        unit: &[SEVERAL],
    },
    Mutation {
        name: "cells-several-flip",
        package: Package::Core,
        breaks: "a point in a cell several rings touch keeps every ring's side at the reference point",
        file: PATH_FILE,
        original: "Event::Crosses(true) => slots.odd ^= bit,",
        replacement: "Event::Crosses(true) => slots.odd ^= 0,",
        integration: &[],
        unit: &[SEVERAL],
    },
    Mutation {
        name: "cells-several-shell-holds",
        package: Package::Core,
        breaks: "a polygon whose shell holds a cell without touching it takes its first touching hole for its shell",
        file: SEVERAL_FILE,
        original: "let shell = if touches { SHELL_TOUCHES } else { SHELL_HOLDS };",
        replacement: "let shell = if touches || !touches { SHELL_TOUCHES } else { SHELL_HOLDS };",
        integration: &[],
        unit: &[SEVERAL],
    },
    Mutation {
        name: "cells-several-hole-order",
        package: Package::Core,
        breaks: "a hole listed after one that holds the cell still decides its points",
        file: PATH_FILE,
        original: "let passed = hole && (ring > holding || found == Location::Exterior);",
        replacement: "let passed = hole && (ring > u32::MAX || found == Location::Exterior);",
        integration: &[],
        unit: &[SEVERAL],
    },
    Mutation {
        name: "cells-several-other-polygon",
        package: Package::Core,
        breaks: "a cell several rings touch, held whole by another part, is decided by its own rings alone",
        file: SEVERAL_FILE,
        original: "    if parity.inside > holding {\n        return Some(INTERIOR);",
        replacement: "    if parity.inside > holding && holding > usize::MAX.saturating_sub(1) {\n        return Some(INTERIOR);",
        integration: &[],
        unit: &[SEVERAL],
    },
];
