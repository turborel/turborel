//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.9.1 Description 15, 25 to
//! 30, §15.11.1, §15.12.1, §15.13.1] Units of measure along and off a
//! curve, and the offsets a distance expression applies to the place it
//! measures along.
//!
//! Implementation-defined choices, as the text gives the offsets' meaning
//! and not their arithmetic:
//!
//! - A lateral offset moves the place along the normal of the curve there,
//!   to the right of the curve's direction unless the LRM's positive
//!   lateral direction is 'left' [§15.9.1 Description 26].
//! - A vertical offset adds to the place's z, the station's z or 0 if the
//!   curve has none, upward unless the LRM's positive vertical direction
//!   is 'down' [§15.9.1 Description 27].
//! - An offset referent geometry moves the base of the offset to the
//!   place of that geometry nearest the station: a lateral offset is then
//!   measured from it, a vertical one from its z. An offset referent
//!   description names the base in words only and is not interpreted: the
//!   offset is measured from the station.
//! - A vector offset adds its vectors' sum to the place, in the LRM's
//!   offset unit along the coordinate axes [§15.13.1].
//! - An offset distance that is the null value moves nothing.

use super::lrm::{DOWN, LEFT};
use super::measure::{Foot, Station, Vertex, nearest, paths_of};
use super::model::{DistanceExp, LatOffsetExp, LrMeasure, Lrm, VectorOffsetExp, VerOffsetExp};
use super::{Catalog, failed, metres_in};
use crate::error::{Result, SqlState};
use crate::geometry::model::Geom;

/// [§15.9.1 Description 15] The length `m` gives, in the units of the
/// curve's coordinates: `m`'s unit, or `default` if it has none.
///
/// # Errors
///
/// SQLSTATE 2FF14 of a unit no `Unit` of the catalog names; 2FF13 of a
/// catalog of no positive `metres_per_unit`.
pub(crate) fn to_coords(m: &LrMeasure, default: &str, cat: &Catalog<'_>) -> Result<f64> {
    Ok(m.measure * metres_of(m.units.as_deref().unwrap_or(default), cat)? / coordinate_metres(cat)?)
}

/// The measure, in `unit`, of a length of `coords` units of the curve's
/// coordinates.
///
/// # Errors
///
/// As `to_coords`.
pub(crate) fn from_coords(coords: f64, unit: &str, cat: &Catalog<'_>) -> Result<f64> {
    Ok(coords * coordinate_metres(cat)? / metres_of(unit, cat)?)
}

/// The metres in one `unit`.
fn metres_of(unit: &str, cat: &Catalog<'_>) -> Result<f64> {
    metres_in(unit, cat.units).filter(|f| *f > 0.0_f64).ok_or_else(|| {
        failed(SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED, "§15.1.1 Description 16", &format!("unsupported unit specified: {unit}"))
    })
}

/// The metres in one unit of the curve's coordinates.
fn coordinate_metres(cat: &Catalog<'_>) -> Result<f64> {
    if cat.metres_per_unit > 0.0_f64 {
        return Ok(cat.metres_per_unit);
    }
    Err(failed(SqlState::SPATIAL_ATTEMPTED_DIVISION_BY_ZERO, "§15.9.1 Description 15", "attempted division by zero"))
}

/// [§15.9.1 Description 25 to 31] The place `d` gives: `station`, moved by
/// its offset expressions.
///
/// # Errors
///
/// SQLSTATE 2FF02 of an offset under an LRM that allows none [§15.9.1
/// Description 25]; as `to_coords`.
pub(crate) fn apply(station: &Station, d: &DistanceExp, lrm: &Lrm, cat: &Catalog<'_>) -> Result<Vertex> {
    if !d.has_offset() {
        return Ok(station.at);
    }
    let Some(unit) = lrm.offset_units.as_deref() else {
        return Err(failed(
            SqlState::SPATIAL_INVALID_ARGUMENT,
            "§15.9.1 Description 25",
            &format!("the LRM {} allows no offsets, but the distance expression has one", lrm.id),
        ));
    };
    let scale = Scale { unit, cat };
    let mut at = station.at;
    if let Some(lateral) = &d.lateral {
        at = scale.lateral(station, lateral, lrm)?;
    }
    if let Some(vertical) = &d.vertical {
        at.z = Some(scale.vertical(&at, vertical, lrm)?);
    }
    if let Some(vector) = &d.vector {
        at = scale.vector(at, vector)?;
    }
    Ok(at)
}

/// An LRM's offset unit, with the catalog that converts it.
struct Scale<'u, 'c> {
    /// The offset unit.
    unit: &'u str,
    /// The catalog.
    cat: &'c Catalog<'c>,
}

impl Scale<'_, '_> {
    /// The length of an offset distance, in coordinate units; 0 of none.
    fn length(&self, m: Option<&LrMeasure>) -> Result<f64> {
        m.map_or(Ok(0.0_f64), |m| to_coords(m, self.unit, self.cat))
    }

    /// [§15.9.1 Description 26, 28, §15.11.1] The place a lateral offset
    /// gives.
    fn lateral(&self, station: &Station, o: &LatOffsetExp, lrm: &Lrm) -> Result<Vertex> {
        let base = based(station.at, o.geometry.as_ref())?;
        let toward_left = if is(lrm.lateral_direction.as_deref(), LEFT) { 1.0_f64 } else { -1.0_f64 };
        let d = toward_left * self.length(o.distance.as_ref())?;
        Ok(Vertex { x: base.x - d * station.dy, y: base.y + d * station.dx, ..base })
    }

    /// [§15.9.1 Description 27, 29, §15.12.1] The z a vertical offset
    /// gives the place `at`.
    fn vertical(&self, at: &Vertex, o: &VerOffsetExp, lrm: &Lrm) -> Result<f64> {
        let base = based(*at, o.geometry.as_ref())?;
        let upward = if is(lrm.vertical_direction.as_deref(), DOWN) { -1.0_f64 } else { 1.0_f64 };
        Ok(base.z.unwrap_or_default() + upward * self.length(o.distance.as_ref())?)
    }

    /// [§15.9.1 Description 30, §15.13.1] The place a vector offset gives
    /// `at`.
    fn vector(&self, at: Vertex, o: &VectorOffsetExp) -> Result<Vertex> {
        let per = to_coords(&LrMeasure { measure: 1.0_f64, units: None }, self.unit, self.cat)?;
        let mut moved = at;
        for v in &o.vectors {
            moved.x += v.x.unwrap_or_default() * per;
            moved.y += v.y.unwrap_or_default() * per;
            if let Some(z) = v.z {
                moved.z = Some(moved.z.unwrap_or_default() + z * per);
            }
        }
        Ok(moved)
    }
}

/// The base of an offset from `at`: the place of `geometry` nearest it,
/// if one is given and not empty.
fn based(at: Vertex, geometry: Option<&Geom>) -> Result<Vertex> {
    let Some(g) = geometry else { return Ok(at) };
    let paths = paths_of(g)?;
    Ok(nearest(&paths, at.x, at.y).map_or(at, |b| Vertex { z: b.z.or(at.z), m: at.m, ..b }))
}

/// Whether `direction` is `word`, in any case.
fn is(direction: Option<&str>, word: &str) -> bool {
    direction.is_some_and(|d| d.eq_ignore_ascii_case(word))
}

/// [§15.4.5 Description 4] The lateral offset of a point whose foot on
/// the curve is `foot`, in the LRM's offset unit and positive direction.
///
/// # Errors
///
/// As `from_coords`.
pub(crate) fn lateral_of(foot: &Foot, unit: &str, lrm: &Lrm, cat: &Catalog<'_>) -> Result<LatOffsetExp> {
    let left = foot.side > 0.0_f64;
    let positive = left == is(lrm.lateral_direction.as_deref(), LEFT);
    let signed = if positive { foot.distance } else { -foot.distance };
    Ok(LatOffsetExp::new(LrMeasure { measure: from_coords(signed, unit, cat)?, units: Some(unit.to_owned()) }))
}

/// [§15.4.5 Description 4] The vertical offset of a point `dz` above the
/// curve, in the LRM's offset unit and positive direction.
///
/// # Errors
///
/// As `from_coords`.
pub(crate) fn vertical_of(dz: f64, unit: &str, lrm: &Lrm, cat: &Catalog<'_>) -> Result<VerOffsetExp> {
    let signed = if is(lrm.vertical_direction.as_deref(), DOWN) { -dz } else { dz };
    Ok(VerOffsetExp::new(LrMeasure { measure: from_coords(signed, unit, cat)?, units: Some(unit.to_owned()) }))
}
