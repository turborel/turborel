//! Units of measure: the rows of the ST_UNITS_OF_MEASURE base table of
//! ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.4, which the views of
//! §19.4 and §19.6 (ST_UNITS) list, and the conversions the unit forms of
//! ST_Distance, ST_Length, ST_Area, ST_Buffer and the curve routines use
//! (§5.1.41 Description 4 and its peers).
//!
//! §20.4 Description 3 makes METRE and RADIAN the base units, each with a
//! CONVERSION_FACTOR of 1. Which other units exist is implementation-
//! defined (§14.1.9 Description 1 m). The rows are the catalog's
//! ST_UNITS_OF_MEASURE, `crate::system::UNITS`, so the views and the
//! engine never disagree.

use super::fault::{Condition, Fault, Outcome};

/// UNIT_TYPE of a linear unit (§20.4 constraint UNIT_TYPE_VALUE).
pub const LINEAR: &str = "LINEAR";
/// UNIT_TYPE of an angular unit (§20.4 constraint UNIT_TYPE_VALUE).
pub const ANGULAR: &str = "ANGULAR";
/// The base linear unit (§20.4 Description 3).
pub const METRE: &str = "METRE";
/// The base angular unit (§20.4 Description 3).
pub const RADIAN: &str = "RADIAN";
/// The angular unit of the seed geographic systems.
pub const DEGREE: &str = "DEGREE";
/// The linear unit of the Texas and other US state plane systems.
pub const US_SURVEY_FOOT: &str = "US SURVEY FOOT";

/// Whether a unit measures lengths or angles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// UNIT_TYPE 'LINEAR', converted to metres.
    Linear,
    /// UNIT_TYPE 'ANGULAR', converted to radians.
    Angular,
}

impl Kind {
    /// The UNIT_TYPE column value.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Linear => LINEAR,
            Kind::Angular => ANGULAR,
        }
    }
}

/// One row of ST_UNITS_OF_MEASURE.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Unit {
    /// UNIT_NAME.
    pub name: &'static str,
    /// UNIT_TYPE.
    pub kind: Kind,
    /// CONVERSION_FACTOR, metres or radians per unit; always above zero.
    pub factor: f64,
    /// DESCRIPTION.
    pub description: &'static str,
}

impl Unit {
    /// The row `(name, type, factor, description)` of the catalog table,
    /// if its type is LINEAR or ANGULAR.
    fn of(row: &(&'static str, &'static str, f64, &'static str)) -> Option<Unit> {
        let (name, kind, factor, description) = *row;
        let kind = [Kind::Linear, Kind::Angular].into_iter().find(|candidate| candidate.name() == kind)?;
        Some(Unit { name, kind, factor, description })
    }
}

/// Every supported unit: the rows of the catalog's ST_UNITS_OF_MEASURE
/// (`crate::system::UNITS`), the one table of units the engine and the
/// views share.
pub fn all() -> impl Iterator<Item = Unit> {
    crate::system::UNITS.iter().filter_map(Unit::of)
}

/// The unit named `name`, compared without regard to ASCII case (an
/// implementation-defined reading of "equal to the value of the
/// UNIT_NAME column", §5.1.41 Description 4b).
pub fn find(name: &str) -> Option<Unit> {
    all().find(|unit| unit.name.eq_ignore_ascii_case(name.trim()))
}

/// The unit `name` of kind `kind`: a supported `<unit name>` in the sense of
/// §5.1.41 Description 4a to 4c, which for distances is a 'LINEAR' row.
///
/// # Errors
///
/// F14 unsupported unit specified if no row of that kind has the name.
pub fn require(name: &str, kind: Kind) -> Outcome<Unit> {
    find(name).filter(|unit| unit.kind == kind).ok_or_else(|| {
        Fault::new(
            Condition::UnsupportedUnit,
            format!("[ISO/IEC 13249-3:2016 §5.1.41 Description 4c] {name} is not a supported {} unit", kind.name()),
        )
    })
}

/// `value` measured in a unit of `from` metres or radians, expressed in a
/// unit of `to`. Both factors are positive by §20.4 FACTOR_VALUE.
pub fn convert(value: f64, from: f64, to: f64) -> f64 {
    value * from / to
}

/// An area of `value` square units of `from` metres, in square units of
/// `to` metres; the area unit forms square the linear factor.
pub fn convert_area(value: f64, from: f64, to: f64) -> f64 {
    value * (from / to) * (from / to)
}
