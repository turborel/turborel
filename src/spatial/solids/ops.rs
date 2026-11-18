//! \[DESIGN\] The 3D set operations and relationships of geometry values,
//! ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.30 to §5.1.33 and §5.1.40
//! to §5.1.44, considering z and not m. Both values are taken apart
//! ([`super::cells`]), scaled to integers of one power of two, and met
//! exactly ([`super::boolean`]); a result is rounded once, at the end,
//! and is of the first value's spatial reference system.

use super::assemble::body;
use super::boolean::{difference, intersection, is_empty, reduced, union};
use super::cells::{Cells, Pieces};
use super::contact::Operand;
use super::exact::Scale;
use crate::error::Result;
use crate::geometry::model::{Dims, Geom};

/// A 3D set operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetOp {
    /// §5.1.30 ST_3DIntersection.
    Intersection,
    /// §5.1.31 ST_3DUnion.
    Union,
    /// §5.1.32 ST_3DDifference.
    Difference,
    /// §5.1.33 ST_3DSymDifference.
    SymDifference,
}

/// Two values made ready to meet, and their scale.
struct Pair {
    first: Operand,
    second: Operand,
    scale: Scale,
}

/// `first` and `second` taken apart at one scale.
fn pair(first: &Geom, second: &Geom) -> Result<Pair> {
    let (a, b) = (Cells::of(first)?, Cells::of(second)?);
    let scale = Scale::of(a.numbers().chain(b.numbers()));
    Ok(Pair { first: Operand::new(a.exact(scale)), second: Operand::new(b.exact(scale)), scale })
}

/// The pieces of `op` of the pair.
fn pieces(op: SetOp, pair: &Pair) -> Pieces {
    match op {
        SetOp::Intersection => reduced(intersection(&pair.first, &pair.second)),
        SetOp::Union => reduced(union(&pair.first, &pair.second)),
        SetOp::Difference => reduced(difference(&pair.first, &pair.second)),
        SetOp::SymDifference => {
            let one = Operand::new(reduced(difference(&pair.first, &pair.second)));
            let other = Operand::new(reduced(difference(&pair.second, &pair.first)));
            reduced(union(&one, &other))
        }
    }
}

/// `first op second`, of `first`'s system, x, y and z.
///
/// # Errors
///
/// As [`Cells::of`] and `Geom::new`.
pub fn set_op(op: SetOp, first: &Geom, second: &Geom) -> Result<Geom> {
    let pair = pair(first, second)?;
    Geom::new(first.srid, Dims::Xyz, body(&pieces(op, &pair), pair.scale))
}

/// §5.1.44 ST_3DIntersects: whether the values share a point.
///
/// # Errors
///
/// As [`Cells::of`].
pub fn intersects(first: &Geom, second: &Geom) -> Result<bool> {
    let pair = pair(first, second)?;
    if pair.first.is_empty() || pair.second.is_empty() {
        return Ok(false);
    }
    Ok(!is_empty(&reduced(intersection(&pair.first, &pair.second))))
}

/// §5.1.40 ST_3DEquals: whether the values are the same point set.
///
/// # Errors
///
/// As [`Cells::of`].
pub fn equals(first: &Geom, second: &Geom) -> Result<bool> {
    let pair = pair(first, second)?;
    let one = reduced(difference(&pair.first, &pair.second));
    Ok(is_empty(&one) && is_empty(&reduced(difference(&pair.second, &pair.first))))
}
