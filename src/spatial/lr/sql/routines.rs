//! The routines of §15, a table of each type's: its constructors,
//! observers, mutators and methods, and the functions of its text and GML.

mod distance;
mod element;
mod lrm;
mod position;
mod referent;

use super::args::P;
use super::routine::Routine;
use crate::spatial::lr::LrKind;

/// ST_LRM.
pub const LRM: P = P::Lr(LrKind::Lrm);
/// ST_LinearElement.
pub const ELEMENT: P = P::Lr(LrKind::LinearElement);
/// ST_LRFeature.
pub const FEATURE: P = P::Lr(LrKind::LrFeature);
/// ST_LRCurve.
pub const CURVE: P = P::Lr(LrKind::LrCurve);
/// ST_LRDirectedEdge.
pub const EDGE: P = P::Lr(LrKind::LrDirectedEdge);
/// ST_PositionExp.
pub const POSITION: P = P::Lr(LrKind::PositionExp);
/// ST_LRMeasure.
pub const MEASURE: P = P::Lr(LrKind::LrMeasure);
/// ST_StartValue.
pub const START: P = P::Lr(LrKind::StartValue);
/// ST_DistanceExp.
pub const DISTANCE: P = P::Lr(LrKind::DistanceExp);
/// ST_Referent.
pub const REFERENT: P = P::Lr(LrKind::Referent);
/// ST_LatOffsetExp.
pub const LATERAL: P = P::Lr(LrKind::LatOffsetExp);
/// ST_VerOffsetExp.
pub const VERTICAL: P = P::Lr(LrKind::VerOffsetExp);
/// ST_VectorOffsetExp.
pub const VECTOR: P = P::Lr(LrKind::VectorOffsetExp);
/// [§15.1.1] The length of an offset direction: CHARACTER VARYING(10).
pub const DIRECTION_LENGTH: u32 = 10;

/// Every routine, each type's in the order §15 defines them.
pub fn all() -> impl Iterator<Item = &'static Routine> {
    lrm::ROUTINES.iter().chain(element::ROUTINES).chain(position::ROUTINES).chain(distance::ROUTINES).chain(referent::ROUTINES)
}
