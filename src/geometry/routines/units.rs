//! The unit forms of ST_Buffer, ST_Distance, ST_Length, ST_Area and
//! ST_Perimeter [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.30, §5.1.41,
//! §7.1.2, §8.1.2, §8.1.4]. A value's numbers are in its system's
//! `<linear unit>`, and a unit form converts them by the CONVERSION_FACTOR
//! rows of ST_UNITS_OF_MEASURE (`spatial::srs::units`).
//!
//! The unspecified plane, SRID 0, measures in metres, the
//! implementation-defined unit of §5.1.41 Description 2b ii. A system
//! without a `<linear unit>`, a geographic one, supports no linear unit
//! for a planar measure, so its unit forms raise unsupported unit
//! specified [§5.1.41 Description 4c]. A system is one supplied or one
//! the statement's catalog defines (`SpatialRefSys::from_srid_in`). A
//! call without a catalog, a constant folded as a statement is planned,
//! knows only those supplied, and leaves any other to its statement.

use crate::catalog::SpatialCatalog;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::spatial::srs::system::SpatialRefSys;
use crate::spatial::srs::units::{self, Kind};
use crate::spatial::srs::{Fault, system};

/// The SQL exception condition of an engine fault, its §22 subclass.
pub(in crate::geometry) fn fault(f: &Fault) -> SqlError {
    let state = SqlState::parse(&format!("2F{}", f.condition.subclass())).unwrap_or(SqlState::SPATIAL_INVALID_ARGUMENT);
    SqlError::new(Code::Data, f.message.clone()).with_state(state)
}

/// The systems a call knows: the statement's catalog, if it has one.
pub(in super::super) type Systems<'a> = Option<&'a SpatialCatalog>;

/// Metres per unit of the numbers of a value of SRID `srid`.
fn metres_per_unit(srid: i32, systems: Systems<'_>) -> Result<f64> {
    if srid == system::UNSPECIFIED {
        return Ok(1.0_f64);
    }
    let system = SpatialRefSys::from_srid_in(srid, systems).map_err(|f| fault(&f))?;
    system.definition().and_then(|cs| cs.linear_unit()).map(|unit| unit.factor).ok_or_else(|| {
        SqlError::new(
            Code::Data,
            format!("[ISO/IEC 13249-3:2016 §5.1.41 Description 4c] the system of SRID {srid} has no linear unit for a planar measure"),
        )
        .with_state(SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED)
    })
}

/// Metres per unit of the linear unit named `name`.
fn metres_per(name: &str) -> Result<f64> {
    units::require(name, Kind::Linear).map(|unit| unit.factor).map_err(|f| fault(&f))
}

/// A length `value` of a value of SRID `srid` in the unit `name`.
pub(in super::super) fn length_in(value: f64, (srid, systems): (i32, Systems<'_>), name: &str) -> Result<f64> {
    Ok(units::convert(value, metres_per_unit(srid, systems)?, metres_per(name)?))
}

/// An area `value` of a value of SRID `srid` in the square of unit `name`.
pub(super) fn area_in(value: f64, (srid, systems): (i32, Systems<'_>), name: &str) -> Result<f64> {
    Ok(units::convert_area(value, metres_per_unit(srid, systems)?, metres_per(name)?))
}

/// A length `value` in the unit `name` as numbers of a value of SRID
/// `srid`, which a buffer's distance is.
pub(super) fn length_of(value: f64, (srid, systems): (i32, Systems<'_>), name: &str) -> Result<f64> {
    Ok(units::convert(value, metres_per(name)?, metres_per_unit(srid, systems)?))
}
