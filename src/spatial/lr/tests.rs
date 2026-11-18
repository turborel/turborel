//! Unit tests of linear referencing [ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §15]: the types (`tests/types.rs`), the constructors, observers
//! and mutators (`tests/build.rs`), the stored form and the text and GML
//! representations (`tests/formats.rs`), and locating and translating
//! positions (`tests/locating.rs`), with the values they share.

use super::*;
use crate::error::SqlState;
use crate::geometry::model::{Body, Curve, Dims, Geom};

mod build;
mod formats;
mod locating;
mod types;

/// The unit every sample measures in.
const METRE: &str = "metre";
/// The unit of the conversion tests.
const KILOMETRE: &str = "kilometre";

/// The seeded linear units.
fn units() -> Vec<Unit<'static>> {
    linear_units()
}

/// The SQLSTATE of the error `r` holds.
fn state<T: std::fmt::Debug>(r: Result<T>) -> SqlState {
    r.unwrap_err().state.unwrap()
}

/// A measure of `v` metres.
fn m(v: f64) -> LrMeasure {
    LrMeasure { measure: v, units: Some(METRE.to_owned()) }
}

/// The LRM `id` of type `lrm_type`, in metres, of no offsets.
fn lrm(id: i32, lrm_type: &str) -> Lrm {
    let parts = LrmParts {
        id: Some(id),
        name: Some(format!("lrm {id}")),
        lrm_type: Some(lrm_type.to_owned()),
        units: Some(METRE.to_owned()),
        constraints: None,
    };
    Lrm::new(parts, &units()).unwrap()
}

/// The LRM `id` of type `lrm_type`, in metres, its offsets in metres
/// positive towards `lateral`.
fn lrm_offsets(id: i32, lrm_type: &str, lateral: &str) -> Lrm {
    let parts = LrmParts {
        id: Some(id),
        name: Some(format!("lrm {id}")),
        lrm_type: Some(lrm_type.to_owned()),
        units: Some(METRE.to_owned()),
        constraints: Some(vec!["one".to_owned()]),
    };
    let offsets = OffsetParts { units: Some(METRE.to_owned()), lateral: Some(lateral.to_owned()), vertical: None };
    Lrm::new_with_offsets(parts, offsets, &units()).unwrap()
}

/// A linestring of SRID 0 through `coords` of `dims`.
fn line(coords: &[f64], dims: Dims) -> Geom {
    Geom::new(0, dims, Body::Curve(Curve::LineString(coords.to_vec()))).unwrap()
}

/// The point (x, y) of SRID 0.
fn point(x: f64, y: f64) -> Geom {
    Geom::new(0, Dims::Xy, Body::Point(vec![x, y])).unwrap()
}

/// The ordinates of a point value.
fn ordinates(g: &Geom) -> Vec<f64> {
    let Body::Point(c) = &g.body else { panic!("not a point: {g:?}") };
    c.clone()
}

/// Whether `a` and `b` are the same ordinates, to rounding.
fn near(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9_f64)
}

/// Element `id`'s attributes, its default LRM `lrm_id`, its default
/// measure 100 metres, its type `road`.
fn parts(id: i32, lrm_id: i32) -> ElementParts {
    ElementParts {
        id: Some(id),
        default_lrm: Some(lrm_id),
        default_measure: Some(m(100.0_f64)),
        element_type: Some("road".to_owned()),
        start_values: None,
    }
}

/// The curve element `id` along `g`, of default LRM `lrm_id`.
fn curve(id: i32, lrm_id: i32, g: Geom) -> LrCurve {
    LrCurve::new(parts(id, lrm_id), Some(g)).unwrap()
}

/// The curve element 1 from (0 0) to (100 0), of default LRM 1.
fn road() -> LrCurve {
    curve(1, 1, line(&[0.0_f64, 0.0_f64, 100.0_f64, 0.0_f64], Dims::Xy))
}
