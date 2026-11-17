//! Immutable closed-bound searches over at most one machine word of
//! rectangles. Four inclusive prefix masks identify exactly the overlapping
//! bounds; callers still refine geometry candidates (D82).

use super::Bounds;

/// One bit per retained row, including rows with no bounds.
pub(crate) const ROWS: usize = 64;
/// Two lower and two upper coordinate comparisons define overlap.
pub(crate) const AXES: usize = 4;

#[derive(Clone, Copy)]
struct Boundary {
    coordinate: f64,
    rows: u64,
}

pub(crate) struct Masks {
    axes: [Vec<Boundary>; AXES],
}

impl Masks {
    /// Four sorted boundaries retained for each rectangle.
    pub(crate) const ENTRY_BYTES: usize = AXES.saturating_mul(size_of::<Boundary>());
    pub(crate) fn new(rectangles: &[Option<Bounds>]) -> Option<Self> {
        if rectangles.len() > ROWS {
            return None;
        }
        let mut axes: [Vec<Boundary>; AXES] = std::array::from_fn(|_| Vec::with_capacity(rectangles.len()));
        for (at, bounds) in rectangles.iter().enumerate() {
            let Some(bounds) = bounds else { continue };
            let rows = 1_u64.checked_shl(u32::try_from(at).ok()?)?;
            let [lx, ly] = bounds.lower();
            let [ux, uy] = bounds.upper();
            for (axis, coordinate) in axes.iter_mut().zip([lx, ly, -ux, -uy]) {
                axis.push(Boundary { coordinate, rows });
            }
        }
        for axis in &mut axes {
            axis.sort_unstable_by(|a, b| a.coordinate.total_cmp(&b.coordinate));
            let mut rows = 0_u64;
            for boundary in axis {
                rows |= boundary.rows;
                boundary.rows = rows;
            }
        }
        Some(Self { axes })
    }

    pub(crate) fn overlapping(&self, bounds: Bounds) -> u64 {
        let [lx, ly] = bounds.lower();
        let [ux, uy] = bounds.upper();
        self.axes.iter().zip([ux, uy, -lx, -ly]).fold(u64::MAX, |rows, (axis, coordinate)| {
            let at = axis.partition_point(|boundary| boundary.coordinate <= coordinate);
            rows & at.checked_sub(1).and_then(|at| axis.get(at)).map_or(0, |boundary| boundary.rows)
        })
    }
}

#[cfg(test)]
mod tests;
