//! The 3D engine of SQL/MM Spatial, ISO/IEC 13249-3:2016 (DIS, SC32 N2593):
//! plane-based exact geometry, on the planar engine's exact predicates,
//! for the 3D set operations and relationships of §5.1; the measures of
//! curves, surfaces and solids in space (§7.1, §8.1, §9.1); boundaries,
//! closedness, simplicity and shells; and the clipping and tables of the
//! TIN surfaces of §8.6, which the curve engine triangulates. It knows
//! the geometry model (`geometry::model`) and no SQL type;
//! `geometry/solids` adapts it to the routines.

pub mod assemble;
pub mod boolean;
pub mod bsp;
pub mod cells;
pub mod contact;
pub mod distance;
pub mod exact;
pub mod kernel;
pub mod measure;
pub mod ops;
pub mod pieces;
pub mod shape;
pub mod table;
pub mod tin;
pub mod triangulate;

use crate::error::{Code, SqlError, SqlState};

/// An SQL/MM Spatial exception of `state`, of the rule `clause` breaks.
pub(crate) fn fault(state: SqlState, clause: &str, detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) {clause}] {detail}")).with_state(state)
}

/// The index after `at` among `count`, wrapping to the first.
pub(crate) fn after(at: usize, count: usize) -> usize {
    ahead(at, 1, count)
}

/// The index before `at` among `count`, wrapping to the last.
pub(crate) fn before(at: usize, count: usize) -> usize {
    ahead(at, count.saturating_sub(1), count)
}

/// The index `steps` after `at` among `count`, wrapping.
pub(crate) fn ahead(at: usize, steps: usize, count: usize) -> usize {
    at.wrapping_add(steps).checked_rem(count).unwrap_or_default()
}

/// Element `at` of a triple, the last for any larger index.
pub(crate) fn pick<T: Copy>(triple: &[T; 3], at: usize) -> T {
    let [first, second, third] = *triple;
    match at {
        0 => first,
        1 => second,
        _ => third,
    }
}

#[cfg(test)]
mod tests;
