//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.42 Description 4, §7.1.3
//! Description 4] The units of a measure in space: a length, area or
//! volume in the linear unit of SELF's spatial reference system, or in
//! the unit a call names. A value of SRID 0, or of a system the catalog
//! does not hold, measures in metres; one of a geographic system has no
//! linear unit to convert from, so naming a unit for it is unsupported.

use super::spatial;
use crate::error::{Result, SqlState};
use crate::spatial::srs::system::SpatialRefSys;
use crate::spatial::srs::units::{Kind, require};

/// The factor of the linear unit lengths of SRID `srid` are in, metres
/// each.
///
/// # Errors
///
/// SQLSTATE 2FF14 for a geographic system.
fn factor_of(srid: i32) -> Result<f64> {
    let Ok(system) = SpatialRefSys::from_srid(srid) else { return Ok(1.0_f64) };
    let Some(definition) = system.definition() else { return Ok(1.0_f64) };
    if definition.is_geodetic() {
        return Err(spatial(
            SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED,
            "§5.1.42 Description 4c",
            "a measure in space of a geographic system has no linear unit",
        ));
    }
    Ok(definition.linear_unit().map_or(1.0_f64, |u| u.factor))
}

/// `value`, a measure of `power` dimensions (1 length, 2 area, 3 volume)
/// of a value of SRID `srid`, in the unit `unit` names, or as it is.
///
/// # Errors
///
/// SQLSTATE 2FF14 if `unit` names no linear unit or the system has none.
pub fn in_unit(value: f64, srid: i32, unit: Option<&str>, power: i32) -> Result<f64> {
    let Some(name) = unit else { return Ok(value) };
    let target = require(name, Kind::Linear)
        .map_err(|f| spatial(SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED, "§5.1.42 Description 4c", &f.message))?;
    let ratio = factor_of(srid)? / target.factor;
    Ok(value * ratio.powi(power))
}
