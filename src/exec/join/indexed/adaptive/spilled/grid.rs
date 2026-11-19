//! \[DESIGN\] A grid of cells over the outer rows' hull, by which both
//! inputs of a partitioned spatial join are written (RULES.md D138). A
//! point's cell counts the boundaries at or below it on each axis, so a
//! point beyond the hull falls in an edge cell and the cell of a point
//! never decreases as it grows. A shape goes to every cell its envelope's
//! corners span. A pair is kept only in the cell of its reference point,
//! the greater lower corner of the two envelopes. It lies in both, so its
//! cell is one each was written to, and the pair is produced once.

use crate::spatial::Bounds;

/// The grid: the inner boundaries of its columns and of its rows.
pub(super) struct Grid {
    xs: Vec<f64>,
    ys: Vec<f64>,
}

impl Grid {
    /// At most `most` cells, at least one, over `hull`, as near square as
    /// their count allows.
    pub(super) fn new(hull: Bounds, most: usize) -> Grid {
        let mut across = 1_usize;
        while across.saturating_add(1).saturating_mul(across.saturating_add(1)) <= most {
            across = across.saturating_add(1);
        }
        let down = most.checked_div(across).unwrap_or(1).max(1);
        let ([lx, ly], [hx, hy]) = (hull.lower(), hull.upper());
        Grid { xs: boundaries(lx, hx, across), ys: boundaries(ly, hy, down) }
    }

    /// How many cells it has.
    pub(super) fn count(&self) -> usize {
        self.xs.len().saturating_add(1).saturating_mul(self.ys.len().saturating_add(1))
    }

    /// The cell of the point `[x, y]`.
    fn cell(&self, [x, y]: [f64; 2]) -> usize {
        let column = self.xs.partition_point(|b| *b <= x);
        let row = self.ys.partition_point(|b| *b <= y);
        row.saturating_mul(self.xs.len().saturating_add(1)).saturating_add(column)
    }

    /// The cells `bounds` is written to.
    pub(super) fn cells(&self, bounds: Bounds) -> impl Iterator<Item = usize> + '_ {
        let width = self.xs.len().saturating_add(1);
        let (low, high) = (self.cell(bounds.lower()), self.cell(bounds.upper()));
        let (columns, rows) = (
            low.checked_rem(width).unwrap_or(0)..=high.checked_rem(width).unwrap_or(0),
            low.checked_div(width).unwrap_or(0)..=high.checked_div(width).unwrap_or(0),
        );
        rows.flat_map(move |row| columns.clone().map(move |column| row.saturating_mul(width).saturating_add(column)))
    }

    /// Whether the pair of envelopes `a` and `b`, which meet, is kept in
    /// cell `cell`: the cell of its reference point.
    pub(super) fn owns(&self, cell: usize, a: Bounds, b: Bounds) -> bool {
        let ([ax, ay], [bx, by]) = (a.lower(), b.lower());
        self.cell([ax.max(bx), ay.max(by)]) == cell
    }
}

/// The `cells - 1` boundaries splitting `low` to `high` evenly.
fn boundaries(low: f64, high: f64, cells: usize) -> Vec<f64> {
    let count = u32::try_from(cells).unwrap_or(u32::MAX);
    (1..count).map(|at| low + (high - low) * f64::from(at) / f64::from(count)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, side: f64) -> Bounds {
        Bounds::new([x, y], [x + side, y + side]).unwrap()
    }

    /// Verifies: [DESIGN] D138: a grid of at most the cells asked over a
    /// hull writes a shape to every cell it spans, edge cells taking what
    /// lies beyond the hull, and each meeting pair is owned by exactly one
    /// cell both shapes were written to.
    #[test]
    fn every_meeting_pair_has_one_owning_cell_both_were_written_to() {
        for most in [1_usize, 2, 3, 4, 9, 10] {
            let grid = Grid::new(square(0.0, 0.0, 10.0), most);
            assert!(grid.count() <= most && grid.count() >= 1, "{most}");
            let shapes: Vec<_> =
                (0_i32..12_i32).map(|n| square(f64::from(n) - 2.0, f64::from(n % 5_i32) * 2.5 - 1.0, f64::from(n % 4_i32) * 1.5)).collect();
            for a in &shapes {
                for b in shapes.iter().filter(|b| a.intersects(**b)) {
                    let owners: Vec<_> = (0..grid.count()).filter(|&c| grid.owns(c, *a, *b)).collect();
                    assert_eq!(owners.len(), 1);
                    assert!(grid.cells(*a).any(|c| c == owners[0]) && grid.cells(*b).any(|c| c == owners[0]));
                    assert_eq!(owners, (0..grid.count()).filter(|&c| grid.owns(c, *b, *a)).collect::<Vec<_>>());
                }
            }
        }
        let grid = Grid::new(square(0.0, 0.0, 10.0), 4);
        assert_eq!(grid.cells(square(-50.0, -50.0, 1.0)).collect::<Vec<_>>(), [0]);
        assert_eq!(grid.cells(square(4.0, 4.0, 2.0)).count(), 4);
        assert_eq!(Grid::new(square(3.0, 3.0, 0.0), 4).cells(square(3.0, 3.0, 0.0)).count(), 1);
    }
}
