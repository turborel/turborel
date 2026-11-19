//! Requirements of the units of measure table and its conversions.

use super::super::Condition;
use super::super::units::{self, Kind};

/// Verifies: [ISO/IEC 13249-3:2016 §20.4 Description 3] METRE and RADIAN
/// are the base units with factor 1, every factor is above zero and
/// every name is unique.
#[test]
fn base_units_and_factors_hold() {
    assert_eq!(units::require(units::METRE, Kind::Linear).map(|unit| unit.factor), Ok(1.0_f64));
    assert_eq!(units::require(units::RADIAN, Kind::Angular).map(|unit| unit.factor), Ok(1.0_f64));
    let all: Vec<_> = units::all().collect();
    assert_eq!(all.len(), crate::system::UNITS.len());
    for (at, unit) in all.iter().enumerate() {
        assert!(unit.factor > 0.0_f64, "{unit:?}");
        assert!(all.iter().skip(at + 1).all(|other| other.name != unit.name), "{unit:?}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.41 Description 4a to 4c] names
/// match without regard to case, a unit of the wrong kind or no unit
/// raises F14, and conversions use the defined factors.
#[test]
fn lookup_and_conversion() {
    assert_eq!(units::find(" us survey foot ").map(|unit| unit.name), Some(units::US_SURVEY_FOOT));
    let fault = units::require("DEGREE", Kind::Linear).expect_err("degree is angular");
    assert_eq!(fault.condition, Condition::UnsupportedUnit);
    assert_eq!(units::require("furlong", Kind::Linear).map_err(|error| error.condition), Err(Condition::UnsupportedUnit));
    let foot = units::require("FOOT", Kind::Linear).map(|unit| unit.factor).unwrap_or_default();
    assert!((units::convert(1.0_f64, 1.0_f64, foot) - 3.280_839_895_013_123_f64).abs() < 1e-12_f64);
    assert!((units::convert_area(1.0_f64, 1000.0_f64, 1.0_f64) - 1e6_f64).abs() < 1e-6_f64);
    assert_eq!(Kind::Angular.name(), units::ANGULAR);
    assert_eq!(Kind::Linear.name(), units::LINEAR);
}
