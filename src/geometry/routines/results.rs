//! The types of the results of ST_Intersection, ST_Union, ST_Difference
//! and ST_SymDifference [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.39,
//! Tables 6 to 11]. The engine gives the most specific value of what
//! remains (`engine::geom_of`), an empty set being the empty point of
//! R00, and that value is in every cell's set but those named here.
//!
//! With an empty operand, the union, the difference of the other and the
//! symmetric difference are the other operand itself, whose type is
//! its row's R01 to R07. A union with an ST_GeomCollection operand is an
//! ST_GeomCollection (R07). The union of point sets of which one is an
//! ST_MultiPoint is an ST_MultiPoint (R04). The difference of an
//! ST_MultiCurve and a point set is an ST_MultiCurve (R05), and of an
//! ST_MultiSurface and a point set or curves an ST_MultiSurface (R06).

use super::super::model::{self, Geom, Kind};
use crate::error::Result;
use crate::spatial::planar::Operation;

/// The parameter type code of Table 6 of a nonempty value of `kind`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Code {
    Point,
    Curve,
    Surface,
    Solid,
    MultiPoint,
    MultiCurve,
    MultiSurface,
    Collection,
}

fn code(kind: Kind) -> Code {
    let tested = [
        (Kind::Point, Code::Point),
        (Kind::Curve, Code::Curve),
        (Kind::Surface, Code::Surface),
        (Kind::Solid, Code::Solid),
        (Kind::MultiPoint, Code::MultiPoint),
        (Kind::MultiCurve, Code::MultiCurve),
        (Kind::MultiSurface, Code::MultiSurface),
    ];
    tested.into_iter().find(|(of, _)| kind.is_subtype_of(*of)).map_or(Code::Collection, |(_, code)| code)
}

/// What the operation of `first` and `second` gives when either is
/// empty: the other operand for a union or symmetric difference, the
/// first for a difference, the empty point for an intersection.
pub(super) fn of_empty(operation: Operation, first: &Geom, second: &Geom) -> Option<Geom> {
    let empty = || Geom { srid: first.srid, dims: model::Dims::Xy, body: model::Body::Point(Vec::new()) };
    match (first.is_empty(), second.is_empty()) {
        (false, false) => None,
        (true, true) => Some(empty()),
        (true, false) => Some(match operation {
            Operation::Union | Operation::SymDifference => second.xy().with_srid(first.srid),
            Operation::Intersection | Operation::Difference => empty(),
        }),
        (false, true) => Some(match operation {
            Operation::Union | Operation::SymDifference | Operation::Difference => first.xy(),
            Operation::Intersection => empty(),
        }),
    }
}

/// The type a result of nonempty operands of `first` and `second` must
/// be cast to, if its own is not in its cell's set.
fn required(operation: Operation, first: Kind, second: Kind) -> Option<Kind> {
    let (one, two) = (code(first), code(second));
    let points = |c: Code| matches!(c, Code::Point | Code::MultiPoint);
    match operation {
        Operation::Union => {
            if one == Code::Collection || two == Code::Collection {
                Some(Kind::GeomCollection)
            } else if points(one) && points(two) && (one == Code::MultiPoint || two == Code::MultiPoint) {
                Some(Kind::MultiPoint)
            } else {
                None
            }
        }
        Operation::Difference => match (one, two) {
            (Code::MultiCurve, Code::Point | Code::MultiPoint) => Some(Kind::MultiCurve),
            (Code::MultiSurface, Code::Point | Code::MultiPoint | Code::Curve | Code::MultiCurve) => Some(Kind::MultiSurface),
            (
                Code::Point
                | Code::Curve
                | Code::Surface
                | Code::Solid
                | Code::MultiPoint
                | Code::MultiCurve
                | Code::MultiSurface
                | Code::Collection,
                Code::Point
                | Code::Curve
                | Code::Surface
                | Code::Solid
                | Code::MultiPoint
                | Code::MultiCurve
                | Code::MultiSurface
                | Code::Collection,
            ) => None,
        },
        Operation::Intersection | Operation::SymDifference => None,
    }
}

/// The engine's result `result` of nonempty `first` and `second`, as a
/// value of its cell's set.
pub(super) fn typed(operation: Operation, first: &Geom, second: &Geom, result: Geom) -> Result<Geom> {
    let Some(kind) = required(operation, first.kind(), second.kind()) else { return Ok(result) };
    if result.kind().is_subtype_of(kind) {
        return Ok(result);
    }
    // The most specific collection of the result's type that is one.
    let specific = [Kind::MultiPoint, Kind::MultiLineString, Kind::MultiPolygon]
        .into_iter()
        .find(|multi| kind != Kind::GeomCollection && multi.is_subtype_of(kind) && model::cast(&result, *multi).is_ok());
    model::cast(&result, specific.unwrap_or(kind))
}
