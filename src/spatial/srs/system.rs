//! The ST_SpatialRefSys type of ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §14.1: a spatial reference system made from its `<spatial reference
//! system>` text or its identifier, its canonical text, its identifier,
//! and equality.
//!
//! These are the implementation-defined parts.
//!
//! - SRID 0 (§14.1.5 Description 2) is an unspecified Cartesian plane,
//!   the system of geometries made without one. It has no definition, so
//!   ST_AsWKTSRS of it is the null value and no transform reaches or
//!   leaves it (F15, §5.1.6 Description 3c).
//! - A system made from text takes the EPSG code of its own AUTHORITY as
//!   its SRID when it has one; otherwise it has none until a catalog row
//!   gives it one.
//! - ST_Equals (§14.1.6 Description 3) holds when the two systems place
//!   every coordinate at the same point: the same kind of system, datum
//!   (by normalized name, ellipsoid and TOWGS84 shift), prime meridian,
//!   units by factor, axes, and projection method and parameters in
//!   metres and radians. Names of the systems, units, ellipsoids, prime
//!   meridians and axes, and every AUTHORITY, do not matter.

use super::fault::{Condition, Fault, Outcome};
use super::model::{Axis, CoordinateSystem, Datum, Direction, Geographic, PrimeMeridian, Unit};
use super::projection::{normalize, resolve};
use super::transform::Transform;
use super::wkt;

/// The identifier of the unspecified plane.
pub const UNSPECIFIED: i32 = 0;

/// An ST_SpatialRefSys value.
#[derive(Clone, Debug, PartialEq)]
pub struct SpatialRefSys {
    srid: Option<i32>,
    definition: Option<CoordinateSystem>,
}

/// The EPSG code of `system`'s own AUTHORITY, if it is a positive integer.
fn epsg_code(system: &CoordinateSystem) -> Option<i32> {
    let authority = system.authority().filter(|authority| authority.name.eq_ignore_ascii_case("EPSG"))?;
    authority.code.trim().parse::<i32>().ok().filter(|code| *code > 0_i32)
}

impl SpatialRefSys {
    /// The unspecified plane, SRID 0.
    pub fn unspecified() -> SpatialRefSys {
        SpatialRefSys { srid: Some(UNSPECIFIED), definition: None }
    }

    /// ST_SpatialRefSys(CHARACTER LARGE OBJECT) and ST_WKTSRSToSQL.
    ///
    /// # Errors
    ///
    /// F22 if `text` is not a `<spatial reference system>`.
    pub fn from_text(text: &str) -> Outcome<SpatialRefSys> {
        let definition = wkt::read(text)?;
        Ok(SpatialRefSys { srid: epsg_code(&definition), definition: Some(definition) })
    }

    /// ST_SpatialRefSys(INTEGER): the system the catalog supplies as
    /// `srid` (`crate::system::SpatialRefSys::by_id`), or SRID 0.
    ///
    /// # Errors
    ///
    /// F59 unknown spatial reference system for any other identifier.
    pub fn from_srid(srid: i32) -> Outcome<SpatialRefSys> {
        if srid == UNSPECIFIED {
            return Ok(SpatialRefSys::unspecified());
        }
        let row = crate::system::SpatialRefSys::by_id(srid).ok_or_else(|| {
            Fault::new(
                Condition::UnknownSystem,
                format!("[ISO/IEC 13249-3:2016 §14.1.2 Description 5] no spatial reference system has SRID {srid}"),
            )
        })?;
        let definition = wkt::read(&row.definition)?;
        Ok(SpatialRefSys { srid: Some(srid), definition: Some(definition) })
    }

    /// The system with the catalog identifier `srid` instead of its own.
    #[must_use]
    pub fn with_srid(self, srid: i32) -> SpatialRefSys {
        SpatialRefSys { srid: Some(srid), ..self }
    }

    /// ST_SRID(), if the system has one.
    pub fn srid(&self) -> Option<i32> {
        self.srid
    }

    /// The parsed definition; none for the unspecified plane.
    pub fn definition(&self) -> Option<&CoordinateSystem> {
        self.definition.as_ref()
    }

    /// ST_AsWKTSRS(): the canonical text; none for the unspecified plane.
    pub fn as_wkt(&self) -> Option<String> {
        self.definition.as_ref().map(wkt::write)
    }

    /// ST_Equals(ST_SpatialRefSys), by the reading of the module notes.
    pub fn equals(&self, other: &SpatialRefSys) -> bool {
        match (&self.definition, &other.definition) {
            (None, None) => true,
            (Some(a), Some(b)) => same_system(a, b),
            (Some(_), None) | (None, Some(_)) => false,
        }
    }

    /// ST_OrderingEquals: 0 if equal, else 1.
    pub fn ordering_equals(&self, other: &SpatialRefSys) -> i32 {
        i32::from(!self.equals(other))
    }

    /// The transform of ST_Transform from this system to `target`.
    ///
    /// # Errors
    ///
    /// F15 if either system is the unspecified plane or the transform
    /// cannot be made.
    pub fn transform_to(&self, target: &SpatialRefSys) -> Outcome<Transform> {
        match (&self.definition, &target.definition) {
            (Some(from), Some(to)) => Transform::new(from, to),
            (Some(_) | None, None) | (None, Some(_)) => Err(Fault::transform(
                "[ISO/IEC 13249-3:2016 §5.1.6 Description 3c] the unspecified plane, SRID 0, has no transform to or from another system",
            )),
        }
    }
}

fn same_system(a: &CoordinateSystem, b: &CoordinateSystem) -> bool {
    match (a, b) {
        (CoordinateSystem::Geographic(a), CoordinateSystem::Geographic(b)) => same_geographic(a, b),
        (CoordinateSystem::Geocentric(a), CoordinateSystem::Geocentric(b)) => {
            same_datum(&a.datum, &b.datum)
                && same_meridian(&a.prime_meridian, 1.0_f64, &b.prime_meridian, 1.0_f64)
                && same_unit(&a.linear_unit, &b.linear_unit)
                && same_axes(&a.axes, &b.axes, &[Direction::Other, Direction::East, Direction::North])
        }
        (CoordinateSystem::Projected(a), CoordinateSystem::Projected(b)) => {
            let (a_angular, b_angular) = (a.geographic.angular_unit.factor, b.geographic.angular_unit.factor);
            let a_method = resolve(&a.projection.name, &a.parameters, a_angular, a.linear_unit.factor);
            let b_method = resolve(&b.projection.name, &b.parameters, b_angular, b.linear_unit.factor);
            let same_method = match (a_method, b_method) {
                (Ok(a_method), Ok(b_method)) => a_method == b_method,
                (Ok(_) | Err(_), Err(_)) | (Err(_), Ok(_)) => {
                    normalize(&a.projection.name) == normalize(&b.projection.name) && a.parameters == b.parameters
                }
            };
            same_method
                && same_geographic(&a.geographic, &b.geographic)
                && same_unit(&a.linear_unit, &b.linear_unit)
                && same_axes(&a.axes, &b.axes, &[Direction::East, Direction::North])
        }
        (CoordinateSystem::Geographic(_), CoordinateSystem::Projected(_) | CoordinateSystem::Geocentric(_))
        | (CoordinateSystem::Projected(_), CoordinateSystem::Geographic(_) | CoordinateSystem::Geocentric(_))
        | (CoordinateSystem::Geocentric(_), CoordinateSystem::Geographic(_) | CoordinateSystem::Projected(_)) => false,
    }
}

fn same_geographic(a: &Geographic, b: &Geographic) -> bool {
    let units =
        same_unit(&a.angular_unit, &b.angular_unit) && a.linear_unit.as_ref().map(|u| u.factor) == b.linear_unit.as_ref().map(|u| u.factor);
    same_datum(&a.datum, &b.datum)
        && same_meridian(&a.prime_meridian, a.angular_unit.factor, &b.prime_meridian, b.angular_unit.factor)
        && units
        && same_axes(&a.axes, &b.axes, &[Direction::East, Direction::North])
}

fn same_datum(a: &Datum, b: &Datum) -> bool {
    let spheroids =
        same(a.spheroid.semi_major, b.spheroid.semi_major) && same(a.spheroid.inverse_flattening, b.spheroid.inverse_flattening);
    normalize(&a.name) == normalize(&b.name) && spheroids && a.to_wgs84 == b.to_wgs84
}

/// Prime meridians compared in radians: a geographic system states its
/// in its angular unit, a geocentric one in degrees (both factor 1 there,
/// compared as written).
fn same_meridian(a: &PrimeMeridian, a_factor: f64, b: &PrimeMeridian, b_factor: f64) -> bool {
    same(a.longitude * a_factor, b.longitude * b_factor)
}

/// Whether two finite values are equal: their difference is zero.
fn same(a: f64, b: f64) -> bool {
    a - b == 0.0_f64
}

fn same_unit(a: &Unit, b: &Unit) -> bool {
    same(a.factor, b.factor)
}

/// The directions of `axes`; no AXIS items mean `defaults`. A trailing
/// up axis is the height every system may carry, so it is dropped.
fn directions(axes: &[Axis], defaults: &[Direction]) -> Vec<Direction> {
    let mut out: Vec<Direction> = if axes.is_empty() { defaults.to_vec() } else { axes.iter().map(|axis| axis.direction).collect() };
    if out.len() == 3 && out.last() == Some(&Direction::Up) {
        out.pop();
    }
    out
}

fn same_axes(a: &[Axis], b: &[Axis], defaults: &[Direction]) -> bool {
    directions(a, defaults) == directions(b, defaults)
}
