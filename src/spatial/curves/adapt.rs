//! Moves curves between the geometry model, whose points lie flat in one
//! vector of numbers, and the curve engines' plain coordinate types, and
//! gives the model its curve linearizer (ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §7.1.6 ST_CurveToLine).

use super::geodesy::Planar;
use super::linearize::Tolerance;
use super::observe::curve_to_line;
use super::types::{Clothoid, Coord, Curve, Dims, Elliptical, Knot, Measures, Nurbs, NurbsPoint, Placement, Spiral, Vector, without_m};
use crate::error::Result;
use crate::geometry::model;

/// The points of `flat`, `dims.stride()` numbers each.
pub fn coords(flat: &[f64], dims: Dims) -> Vec<Coord> {
    flat.chunks_exact(dims.stride()).map(|p| coord(p, dims)).collect()
}

/// One point from its numbers.
pub fn coord(numbers: &[f64], dims: Dims) -> Coord {
    let at = |i: Option<usize>| i.and_then(|i| numbers.get(i)).copied().unwrap_or_default();
    Coord { x: at(Some(0)), y: at(Some(1)), z: at(dims.z_at()), m: at(dims.m_at()) }
}

/// The numbers of `points`, laid flat.
pub fn flat(points: &[Coord], dims: Dims) -> Vec<f64> {
    let mut out = Vec::with_capacity(points.len().saturating_mul(dims.stride()));
    for p in points {
        push(&mut out, *p, dims);
    }
    out
}

/// Appends one point's numbers.
pub fn push(out: &mut Vec<f64>, p: Coord, dims: Dims) {
    out.extend([p.x, p.y]);
    if dims.has_z() {
        out.push(p.z);
    }
    if dims.has_m() {
        out.push(p.m);
    }
}

fn measures(m: Option<model::Measures>) -> Measures {
    m.map(|found| [found.start, found.end])
}

fn model_measures(m: Measures) -> Option<model::Measures> {
    m.map(|[start, end]| model::Measures { start, end })
}

fn placement(p: &model::Placement, dims: Dims) -> Placement {
    let plane = without_m(dims);
    let location = if p.location.is_empty() { coord(&[f64::NAN; 3], plane) } else { coord(&p.location, plane) };
    let directions = p.directions.chunks_exact(plane.stride()).map(|d| Vector::of(coord(d, plane).space())).collect();
    Placement { location, directions }
}

fn model_placement(p: &Placement, dims: Dims) -> model::Placement {
    let plane = without_m(dims);
    let directions = p.directions.iter().flat_map(|d| flat(&[Coord::xyz(d.x, d.y, d.z)], plane)).collect();
    model::Placement { location: flat(&[p.location], plane), directions }
}

/// The engines' form of a model curve.
pub fn from_model(c: &model::Curve, dims: Dims) -> Curve {
    let plane = without_m(dims);
    match c {
        model::Curve::LineString(p) => Curve::LineString(coords(p, dims)),
        model::Curve::CircularString(p) => Curve::CircularString(coords(p, dims)),
        model::Curve::Circle(p) => Curve::Circle(coords(p, dims)),
        model::Curve::GeodesicString(p) => Curve::GeodesicString(coords(p, dims)),
        model::Curve::Elliptical(e) => Curve::Elliptical(e.as_ref().map(|e| Elliptical {
            placement: placement(&e.placement, dims),
            u_axis: e.u_axis,
            v_axis: e.v_axis,
            start: e.start_angle,
            end: e.end_angle,
            measures: measures(e.measures),
        })),
        model::Curve::Nurbs(n) => Curve::Nurbs(n.as_ref().map(|n| Nurbs {
            degree: u32::from(n.degree),
            points: n.points.iter().map(|p| NurbsPoint { point: coord(&p.point, plane), weight: p.weight }).collect(),
            knots: n.knots.iter().map(|k| Knot { value: k.value, multiplicity: u32::from(k.multiplicity) }).collect(),
            measures: measures(n.measures),
        })),
        model::Curve::Clothoid(k) => Curve::Clothoid(k.as_ref().map(|k| Clothoid {
            placement: placement(&k.placement, dims),
            scale: k.scale_factor,
            start: k.start_distance,
            end: k.end_distance,
            measures: measures(k.measures),
        })),
        model::Curve::Spiral(s) => Curve::Spiral(s.as_ref().map(|s| Spiral {
            placement: placement(&s.placement, dims),
            length: s.length,
            start_curvature: s.start_curvature,
            end_curvature: s.end_curvature,
            kind: s.spiral_type,
            measures: measures(s.measures),
        })),
        model::Curve::Compound(parts) => Curve::Compound(parts.iter().map(|p| from_model(p, dims)).collect()),
    }
}

/// The model's form of an engine curve; a degree or multiplicity past a
/// byte saturates, which the model's checks then refuse.
pub fn to_model(c: &Curve, dims: Dims) -> model::Curve {
    let plane = without_m(dims);
    let byte = |v: u32| u8::try_from(v).unwrap_or(u8::MAX);
    match c {
        Curve::LineString(p) => model::Curve::LineString(flat(p, dims)),
        Curve::CircularString(p) => model::Curve::CircularString(flat(p, dims)),
        Curve::Circle(p) => model::Curve::Circle(flat(p, dims)),
        Curve::GeodesicString(p) => model::Curve::GeodesicString(flat(p, dims)),
        Curve::Elliptical(e) => model::Curve::Elliptical(e.as_ref().map(|e| model::Elliptical {
            placement: model_placement(&e.placement, dims),
            u_axis: e.u_axis,
            v_axis: e.v_axis,
            start_angle: e.start,
            end_angle: e.end,
            measures: model_measures(e.measures),
        })),
        Curve::Nurbs(n) => model::Curve::Nurbs(n.as_ref().map(|n| model::Nurbs {
            degree: byte(n.degree),
            points: n.points.iter().map(|p| model::NurbsPoint { point: flat(&[p.point], plane), weight: p.weight }).collect(),
            knots: n.knots.iter().map(|k| model::Knot { value: k.value, multiplicity: byte(k.multiplicity) }).collect(),
            measures: model_measures(n.measures),
        })),
        Curve::Clothoid(k) => model::Curve::Clothoid(k.as_ref().map(|k| model::Clothoid {
            placement: model_placement(&k.placement, dims),
            scale_factor: k.scale,
            start_distance: k.start,
            end_distance: k.end,
            measures: model_measures(k.measures),
        })),
        Curve::Spiral(s) => model::Curve::Spiral(s.as_ref().map(|s| model::Spiral {
            placement: model_placement(&s.placement, dims),
            length: s.length,
            start_curvature: s.start_curvature,
            end_curvature: s.end_curvature,
            spiral_type: s.kind,
            measures: model_measures(s.measures),
        })),
        Curve::Compound(parts) => model::Curve::Compound(parts.iter().map(|p| to_model(p, dims)).collect()),
    }
}

/// The model's curve linearizer: the curve's ST_CurveToLine at the
/// default tolerance, in the coordinate plane (a geodesic string's
/// geodesy needs its reference system, which the model's linear form
/// does not carry).
pub fn linearizer(c: &model::Curve, dims: Dims) -> Option<Result<model::Coords>> {
    Some(curve_to_line(&from_model(c, dims), &Tolerance::DEFAULT, &Planar).map(|points| flat(&points, dims)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spatial::formats::samples::{DIMS, curves};

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1] Every curve
    /// type in every dimension crosses to the model and back unchanged.
    #[test]
    fn curves_cross_both_ways() {
        for dims in DIMS {
            for curve in curves(dims) {
                assert_eq!(from_model(&to_model(&curve, dims), dims), curve, "{dims:?}");
            }
        }
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.6] The model's
    /// linear form of a circular arc runs from its start to its end
    /// through points on the circle.
    #[test]
    fn linear_form_of_an_arc() {
        let arc = model::Curve::CircularString(vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64, 2.0_f64, 0.0_f64]);
        let line = linearizer(&arc, Dims::Xy).unwrap().unwrap();
        let points = coords(&line, Dims::Xy);
        assert!(points.len() > 8);
        assert_eq!(points.first().copied(), Some(Coord::xy(0.0, 0.0)));
        assert_eq!(points.last().copied(), Some(Coord::xy(2.0, 0.0)));
        assert!(points.iter().all(|p| ((p.x - 1.0).hypot(p.y) - 1.0).abs() < 1e-9_f64));
        let g = model::Geom::new(0, Dims::Xy, model::Body::Curve(arc)).unwrap();
        assert_eq!(g.linear().unwrap().kind(), model::Kind::LineString);
    }
}
