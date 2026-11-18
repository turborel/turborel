//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.2, §5.1.3, §5.1.11,
//! §5.1.12, §5.1.19, §5.1.20, §5.1.25 to §5.1.28, §6.1.5 to §6.1.7,
//! §8.1.10, §10.3.3] The observers of a value's dimensions and extent:
//! ST_Dimension, ST_CoordDim, ST_Is3D, ST_IsMeasured, ST_Envelope,
//! ST_EnvelopeAsPts, ST_MinZ, ST_MaxZ, ST_MinM and ST_MaxM of every
//! geometry; ST_Z, ST_M and ST_ExplicitPoint of a point; ST_IsWorld of a
//! surface; and ST_IsClosed of a multicurve. Extremes are of the value's
//! linear form, a curve of another interpolation approximated as
//! ST_CurveToLine approximates it. A surface of the model is bounded, so
//! its exterior is never empty and ST_IsWorld is 0.

use std::collections::HashMap;

use crate::error::Result;
use crate::geometry::model::{Body, Collection, Dims, Geom, Kind};
use crate::geometry::observers::{Observer, Parameter, Returns, Warned, double, int, truth};
use crate::spatial::curves::geodesy::Planar;
use crate::spatial::curves::observe;
use crate::spatial::curves::types::Curve;
use crate::spatial::formats::tree::{self, Geometry};
use crate::value::Value;

/// ETOL, the envelope tolerance, least [§5.1.19 Description 2c].
const ETOL: f64 = 1.0e-9_f64;
/// ETOL relative to a coordinate's magnitude, so a widened side is wider.
const ETOL_RELATIVE: f64 = 1.0e-12_f64;
/// The position of y in a point's ordinates.
const Y: usize = 1;

/// An observer of a value's dimensions or extent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Property {
    /// [§5.1.2] ST_Dimension().
    Dimension,
    /// [§5.1.3] ST_CoordDim().
    CoordDim,
    /// [§5.1.11] ST_Is3D().
    Is3D,
    /// [§5.1.12] ST_IsMeasured().
    IsMeasured,
    /// [§5.1.19] ST_Envelope().
    Envelope,
    /// [§5.1.20] ST_EnvelopeAsPts().
    EnvelopeAsPts,
    /// [§5.1.25] ST_MinZ().
    MinZ,
    /// [§5.1.26] ST_MaxZ().
    MaxZ,
    /// [§5.1.27] ST_MinM().
    MinM,
    /// [§5.1.28] ST_MaxM().
    MaxM,
    /// [§6.1.5] ST_Z() of a point.
    Z,
    /// [§6.1.6] ST_M() of a point.
    M,
    /// [§6.1.7] ST_ExplicitPoint().
    ExplicitPoint,
    /// [§8.1.10] ST_IsWorld().
    IsWorld,
    /// [§10.3.3] ST_IsClosed() of a multicurve.
    IsClosed,
}

/// [§5.1.2] The dimension of `body`: -1 of the empty set, else the most
/// of its parts.
fn dimension(body: &Body) -> i16 {
    if body.is_empty() {
        return -1;
    }
    match body {
        Body::Collection(Collection::Geometry(parts)) => parts.iter().map(dimension).max().unwrap_or(-1),
        Body::Collection(Collection::MultiPoint(_)) => 0,
        Body::Collection(Collection::MultiCurve(_) | Collection::MultiLineString(_)) => 1,
        Body::Collection(Collection::MultiSurface(_) | Collection::MultiPolygon(_)) => 2,
        Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) => body.kind().dimension().unwrap_or(-1),
    }
}

/// The least and greatest ordinate `at` of `g`, none if it has none.
fn extremes(g: &Geom, at: Option<usize>) -> Result<Option<(f64, f64)>> {
    let Some(at) = at.filter(|_| !g.is_empty()) else { return Ok(None) };
    let all = g.ordinates(at)?;
    Ok(all.iter().copied().reduce(f64::min).zip(all.iter().copied().reduce(f64::max)))
}

/// [§5.1.19 Description 2d, 2e] A side widened by ETOL if it is a point.
fn widened((low, high): (f64, f64)) -> (f64, f64) {
    if low < high {
        return (low, high);
    }
    let by = ETOL.max(low.abs() * ETOL_RELATIVE);
    (low - by, high + by)
}

/// [§5.1.19] The bounding rectangle of `g` in x and y, null when empty.
fn envelope(g: &Geom) -> Result<Value> {
    let (Some(x), Some(y)) = (extremes(g, Some(0))?, extremes(g, Some(Y))?) else { return Ok(Value::Null) };
    let ((x0, x1), (y0, y1)) = (widened(x), widened(y));
    let ring = vec![x0, y0, x1, y0, x1, y1, x0, y1, x0, y0];
    let body = Body::Surface(crate::geometry::model::Surface::Polygon(vec![ring]));
    Ok(Geom::new(g.srid, Dims::Xy, body)?.to_value())
}

/// [§5.1.20] The points of the least and the greatest coordinates of `g`.
fn envelope_points(g: &Geom) -> Result<Value> {
    let (Some(xs), Some(ys)) = (extremes(g, Some(0))?, extremes(g, Some(Y))?) else { return Ok(Value::Null) };
    let zs = extremes(g, g.dims.z_at())?;
    let ms = extremes(g, g.dims.m_at())?;
    let point = |pick: fn((f64, f64)) -> f64| -> Result<Value> {
        let coords = [Some(pick(xs)), Some(pick(ys)), zs.map(pick), ms.map(pick)].into_iter().flatten().collect();
        Ok(Geom::new(g.srid, g.dims, Body::Point(coords))?.to_value())
    };
    Ok(Value::Array(vec![point(|(low, _)| low)?, point(|(_, high)| high)?].into()))
}

/// [§10.3.3] Whether a multicurve's boundary is empty: each end point is
/// the end of an even number of its curves, x and y read.
fn closed(value: &Geometry) -> Result<bool> {
    let curves: Vec<Curve> = match &value.body {
        tree::Body::Collection(tree::Collection::MultiCurve(curves)) => curves.clone(),
        tree::Body::Collection(tree::Collection::MultiLineString(lines)) => lines.iter().map(|l| Curve::LineString(l.clone())).collect(),
        tree::Body::Point(_) | tree::Body::Curve(_) | tree::Body::Surface(_) | tree::Body::Solid(_) | tree::Body::Collection(_) => {
            Vec::new()
        }
    };
    let mut ends: HashMap<(u64, u64), usize> = HashMap::new();
    for c in &curves {
        for p in [observe::start_point(c, &Planar)?, observe::end_point(c, &Planar)?].into_iter().flatten() {
            let key = ((p.x + 0.0_f64).to_bits(), (p.y + 0.0_f64).to_bits());
            let count = ends.entry(key).or_default();
            *count = count.saturating_add(1);
        }
    }
    Ok(!curves.is_empty() && ends.values().all(|n| n.is_multiple_of(2)))
}

/// [§6.1.7] A point's coordinates, null when empty.
fn explicit(g: &Geom) -> Value {
    let Body::Point(coords) = &g.body else { return Value::Null };
    if coords.is_empty() {
        return Value::Null;
    }
    Value::Array(coords.iter().map(|c| Value::Approx(*c)).collect::<Vec<_>>().into())
}

/// [§6.1.5, §6.1.6] A point's ordinate `at`, null if it has none.
fn ordinate(g: &Geom, at: Option<usize>) -> Value {
    let Body::Point(coords) = &g.body else { return Value::Null };
    double(at.and_then(|at| coords.get(at).copied()))
}

impl Observer for Property {
    fn name(self) -> &'static str {
        match self {
            Self::Dimension => "ST_DIMENSION",
            Self::CoordDim => "ST_COORDDIM",
            Self::Is3D => "ST_IS3D",
            Self::IsMeasured => "ST_ISMEASURED",
            Self::Envelope => "ST_ENVELOPE",
            Self::EnvelopeAsPts => "ST_ENVELOPEASPTS",
            Self::MinZ => "ST_MINZ",
            Self::MaxZ => "ST_MAXZ",
            Self::MinM => "ST_MINM",
            Self::MaxM => "ST_MAXM",
            Self::Z => "ST_Z",
            Self::M => "ST_M",
            Self::ExplicitPoint => "ST_EXPLICITPOINT",
            Self::IsWorld => "ST_ISWORLD",
            Self::IsClosed => "ST_ISCLOSED",
        }
    }

    fn parameters(self) -> &'static [Parameter] {
        &[]
    }

    fn returns(self, _: Kind) -> Returns {
        match self {
            Self::Dimension | Self::CoordDim | Self::Is3D | Self::IsMeasured | Self::IsWorld | Self::IsClosed => Returns::Integer,
            Self::Envelope => Returns::Of(Kind::Polygon),
            Self::EnvelopeAsPts => Returns::Array(Kind::Point),
            Self::MinZ | Self::MaxZ | Self::MinM | Self::MaxM | Self::Z | Self::M => Returns::Double,
            Self::ExplicitPoint => Returns::Doubles,
        }
    }

    fn warns(self) -> bool {
        false
    }

    fn of(self, g: &Geom, value: &Geometry, _: &[Value]) -> Result<Warned> {
        let low = |pair: Option<(f64, f64)>| double(pair.map(|(low, _)| low));
        let high = |pair: Option<(f64, f64)>| double(pair.map(|(_, high)| high));
        let result = match self {
            Self::Dimension => Value::Exact(crate::value::Decimal::new(i128::from(dimension(&g.body)), 0)),
            Self::CoordDim => int(g.dims.stride()),
            Self::Is3D => truth(g.dims.has_z()),
            Self::IsMeasured => truth(g.dims.has_m()),
            Self::Envelope => envelope(g)?,
            Self::EnvelopeAsPts => envelope_points(g)?,
            Self::MinZ => low(extremes(g, g.dims.z_at())?),
            Self::MaxZ => high(extremes(g, g.dims.z_at())?),
            Self::MinM => low(extremes(g, g.dims.m_at())?),
            Self::MaxM => high(extremes(g, g.dims.m_at())?),
            Self::Z => ordinate(g, g.dims.z_at()),
            Self::M => ordinate(g, g.dims.m_at()),
            Self::ExplicitPoint => explicit(g),
            Self::IsWorld => truth(false),
            Self::IsClosed => truth(closed(value)?),
        };
        Ok((result, None))
    }
}
