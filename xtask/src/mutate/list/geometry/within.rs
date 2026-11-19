//! The mutations of scans deciding spatial conjuncts of their entries
//! and of the cells' bounds on a point's distance (RULES.md R-TEST-9,
//! D85): a cell's bounds drawn too tight, below by a cell, or above by
//! the diagonal; a scan yielding entries decided false, or rows a reader
//! may not see; and a planner reading no column of a decided entry
//! though the projection, or another conjunct, reads it.

use super::{Mutation, Package};

/// Mechanisms of decided scans and distance bounds whose removal must
/// fail their tests.
pub const WITHIN: [Mutation; 6] = [
    Mutation {
        name: "within-cells-lower",
        package: Package::Core,
        breaks: "a cell's lower bound on a point's distance counts the cell holding the edge as a whole cell away",
        file: "src/geometry/prepared/cells/within.rs",
        original: "let lower = ((cells + near).max(far) - 1.0_f64).max(0.0_f64) * grid.side();",
        replacement: "let lower = (cells + near).max(far) * grid.side();",
        integration: &[],
        unit: &[
            "geometry::prepared::cells::tests::distance_bounds_hold_within_and_beyond_the_grid",
            "geometry::tests::prepared::prepared_comparisons_decide_as_the_measures_compare",
        ],
    },
    Mutation {
        name: "within-cells-upper",
        package: Package::Core,
        breaks: "a cell's upper bound on a point's distance spans cells along an axis, not a diagonal",
        file: "src/geometry/prepared/cells/within.rs",
        original: "((cells + far + 2.0_f64) * grid.side() * std::f64::consts::SQRT_2).next_up()",
        replacement: "((cells + far + 2.0_f64) * grid.side()).next_up()",
        integration: &[],
        unit: &[
            "geometry::prepared::cells::tests::distance_bounds_hold_within_and_beyond_the_grid",
            "geometry::tests::prepared::prepared_comparisons_decide_as_the_measures_compare",
        ],
    },
    Mutation {
        name: "scan-decided-false",
        package: Package::Core,
        breaks: "a scan deciding a conjunct yields the entries it decides false, as rows of nulls",
        file: "src/exec/scan/decided.rs",
        original: "Some(Ok(Some(false) | None)) => Ok(None),",
        replacement: "Some(Ok(Some(false) | None)) => Ok(self.width.map(|width| vec![Value::Null; width])),",
        integration: &[],
        unit: &["db::prepared::plans::tests::scans_decide_spatial_conjuncts_of_their_entries"],
    },
    Mutation {
        name: "scan-decided-visible",
        package: Package::Core,
        breaks: "a scan deciding a conjunct makes a row of an entry decided true whether or not every reader sees it",
        file: "src/exec/scan/decided.rs",
        original: "self.width.filter(|_| self.unread && storage.visible_to_all(id))",
        replacement: "self.width.filter(|_| self.unread || storage.visible_to_all(id))",
        integration: &[],
        unit: &["db::prepared::plans::tests::scans_decide_spatial_conjuncts_of_their_entries"],
    },
    Mutation {
        name: "planner-decided-above",
        package: Package::Core,
        breaks: "a scan deciding a conjunct reads no column of an entry decided though the projection reads the call's column",
        file: "src/physical/access/spatial.rs",
        original: "unread: !others && !above.contains(&column)",
        replacement: "unread: !others && above.contains(&column) == above.contains(&column)",
        integration: &[],
        unit: &["db::prepared::plans::tests::scans_decide_spatial_conjuncts_of_their_entries"],
    },
    Mutation {
        name: "planner-decided-others",
        package: Package::Core,
        breaks: "a scan deciding a conjunct reads no column of an entry decided though another conjunct reads the call's column",
        file: "src/physical/access/spatial.rs",
        original: "let others = conds.iter().enumerate().any(|(other, cond)| other != *at && reads(cond));",
        replacement: "let others = conds.iter().enumerate().any(|(other, cond)| other != *at && reads(cond) && other == *at);",
        integration: &[],
        unit: &["db::prepared::plans::tests::scans_decide_spatial_conjuncts_of_their_entries"],
    },
];
