//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §6.1.3 to §6.1.6] The
//! mutators of a point's coordinates. A point is well formed with x and y
//! together [§6.1.1 Description 12], so ST_X or ST_Y of the empty point,
//! which would give it one alone, raises 2FF18. ST_Z and ST_M of a
//! number give a point without z or m the coordinate; of the empty point
//! they raise 2FF03, and the null value of a point that is not empty
//! raises 2FF16 [§6.1.5 Description 4a, 4b].

use super::null_argument;
use crate::error::{Result, SqlState};
use crate::geometry::model::{Body, Dims, Geom};
use crate::geometry::observers::signature;
use crate::geometry::representations::spatial;
use crate::value::Value;

/// The ordinates of a point, by name.
#[derive(Clone, Copy)]
struct Ordinates {
    x: f64,
    y: f64,
    z: Option<f64>,
    m: Option<f64>,
}

impl Ordinates {
    fn of(coords: &[f64], dims: Dims) -> Option<Ordinates> {
        let (&x, &y) = (coords.first()?, coords.get(1)?);
        let z = dims.z_at().and_then(|at| coords.get(at).copied());
        let m = dims.m_at().and_then(|at| coords.get(at).copied());
        Some(Ordinates { x, y, z, m })
    }

    /// Its dimensions and coordinates.
    fn point(self) -> (Dims, Vec<f64>) {
        let dims = Dims::Xy.with_z(self.z.is_some()).with_m(self.m.is_some());
        let coords = [Some(self.x), Some(self.y), self.z, self.m].into_iter().flatten().collect();
        (dims, coords)
    }
}

/// A coordinate of a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Axis {
    X,
    Y,
    Z,
    M,
}

impl Axis {
    const fn name(self) -> &'static str {
        match self {
            Self::X => "ST_X",
            Self::Y => "ST_Y",
            Self::Z => "ST_Z",
            Self::M => "ST_M",
        }
    }
}

/// [§6.1.3 to §6.1.6] `g`, a point, with the coordinate `axis` set to
/// `arg`.
pub(super) fn coordinate(axis: Axis, g: &Geom, arg: &Value) -> Result<Geom> {
    let Body::Point(coords) = &g.body else { return Err(signature(axis.name())) };
    let given = if let Value::Approx(v) = arg {
        Some(*v)
    } else if arg.is_null() {
        None
    } else {
        return Err(signature(axis.name()));
    };
    let Some(mut at) = Ordinates::of(coords, g.dims) else { return empty(axis, g, given) };
    let Some(v) = given else {
        return Err(spatial(SqlState::SPATIAL_NOT_AN_EMPTY_SET, "§6.1.5 Description 4b", "not an empty set"));
    };
    match axis {
        Axis::X => at.x = v,
        Axis::Y => at.y = v,
        Axis::Z => at.z = Some(v),
        Axis::M => at.m = Some(v),
    }
    let (dims, point) = at.point();
    Geom::new(g.srid, dims, Body::Point(point))
}

/// The mutator of `axis` of the empty point `g` and `given`.
fn empty(axis: Axis, g: &Geom, given: Option<f64>) -> Result<Geom> {
    if matches!(axis, Axis::X | Axis::Y) {
        return Err(spatial(SqlState::SPATIAL_POINT_VALUE_NOT_WELL_FORMED, "§6.1.1 Description 12a", "point value not well formed"));
    }
    if given.is_some() {
        return Err(null_argument(axis.name()));
    }
    Ok(g.clone())
}
