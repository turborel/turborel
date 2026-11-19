//! Requirements of ST_Angle.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

use super::super::Condition;
use super::super::angle::Angle;
use super::super::direction::Direction;

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12_f64, "{actual} is not {expected}");
}

fn condition<T: std::fmt::Debug>(outcome: Result<T, super::super::Fault>) -> Condition {
    outcome.expect_err("a fault").condition
}

/// Verifies: [ISO/IEC 13249-3:2016 §16.1.2 Description 2, 5, 9, 14] the
/// numeric constructors: radians, the 'R', 'D' and 'G' indicators (F02
/// for any other), and degrees with minutes and seconds, whose sign
/// follows a negative degree count; minutes and seconds outside [0, 60)
/// raise F02, each bound checked (MC/DC: below zero, at sixty).
#[test]
fn numeric_constructors() {
    close(Angle::new(1.5_f64).radians(), 1.5_f64);
    close(Angle::with_units("R", 1.0_f64).unwrap().radians(), 1.0_f64);
    close(Angle::with_units("D", 90.0_f64).unwrap().radians(), FRAC_PI_2);
    close(Angle::with_units("G", 100.0_f64).unwrap().radians(), FRAC_PI_2);
    assert_eq!(condition(Angle::with_units("d", 1.0_f64)), Condition::InvalidArgument);
    close(Angle::from_degrees_minutes(10, 30.0_f64).unwrap().degrees(), 10.5_f64);
    close(Angle::from_degrees_minutes(-10, 30.0_f64).unwrap().degrees(), -10.5_f64);
    close(Angle::from_dms(10, 30, 36.0_f64).unwrap().degrees(), 10.51_f64);
    close(Angle::from_dms(-10, 30, 36.0_f64).unwrap().degrees(), -10.51_f64);
    for minutes in [-1.0_f64, 60.0_f64] {
        assert_eq!(condition(Angle::from_degrees_minutes(1, minutes)), Condition::InvalidArgument);
    }
    assert_eq!(condition(Angle::from_dms(1, 60, 0.0_f64)), Condition::InvalidArgument);
    assert_eq!(condition(Angle::from_dms(1, -1, 0.0_f64)), Condition::InvalidArgument);
    assert_eq!(condition(Angle::from_dms(1, 0, 60.0_f64)), Condition::InvalidArgument);
    assert_eq!(condition(Angle::from_dms(1, 0, -0.5_f64)), Condition::InvalidArgument);
}

/// Verifies: [ISO/IEC 13249-3:2016 §16.1.2 Description 17, 19, 23] the
/// angle at a point, between directions and between lines is the lesser
/// clockwise angle; empty, ill-formed and coincident points raise F17,
/// F18 and F19, and lines that are not lines or are closed F20 and F21.
#[test]
fn geometric_constructors() {
    let origin: &[f64] = &[0.0_f64, 0.0_f64];
    close(Angle::at_point(origin, &[0.0_f64, 1.0_f64], &[1.0_f64, 0.0_f64]).unwrap().radians(), FRAC_PI_2);
    close(Angle::at_point(origin, &[1.0_f64, 0.0_f64], &[0.0_f64, 1.0_f64]).unwrap().radians(), FRAC_PI_2);
    close(Angle::at_point(origin, &[0.0_f64, 1.0_f64], &[-1.0_f64, -1.0_f64, 5.0_f64]).unwrap().radians(), 3.0_f64 * FRAC_PI_4);
    assert_eq!(condition(Angle::at_point(&[], origin, origin)), Condition::EmptyPoint);
    assert_eq!(condition(Angle::at_point(&[f64::NAN, 0.0_f64], origin, origin)), Condition::PointNotWellFormed);
    assert_eq!(condition(Angle::at_point(origin, &[1.0_f64], origin)), Condition::PointNotWellFormed);
    assert_eq!(condition(Angle::at_point(origin, &[1.0_f64, 1.0_f64], &[])), Condition::EmptyPoint);
    assert_eq!(condition(Angle::at_point(origin, origin, &[1.0_f64, 1.0_f64])), Condition::PointsEqual);
    assert_eq!(condition(Angle::at_point(origin, &[1.0_f64, 1.0_f64], origin)), Condition::PointsEqual);
    let north = Direction::new(0.0_f64).unwrap();
    let west = Direction::new(1.5_f64 * PI).unwrap();
    close(Angle::between(north, west).radians(), FRAC_PI_2);
    close(Angle::between(west, north).radians(), FRAC_PI_2);
    let east_line: [&[f64]; 2] = [origin, &[1.0_f64, 0.0_f64]];
    let north_line: [&[f64]; 2] = [&[5.0_f64, 5.0_f64], &[5.0_f64, 6.0_f64]];
    close(Angle::between_lines(&east_line, &north_line).unwrap().radians(), FRAC_PI_2);
    assert_eq!(condition(Angle::between_lines(&[origin], &north_line)), Condition::NotALine);
    assert_eq!(condition(Angle::between_lines(&[origin, origin], &north_line)), Condition::DegenerateLine);
    assert_eq!(condition(Angle::between_lines(&east_line, &[origin, origin, origin])), Condition::NotALine);
}

/// Verifies: [ISO/IEC 13249-3:2016 §16.1.3 to §16.1.9] radians, degrees
/// and gradians observed and set, and the FLOOR components of positive
/// and negative angles (−10.5° is −11° 30′ 0″).
#[test]
fn conversions_and_components() {
    let angle = Angle::from_degrees(-10.5_f64);
    close(angle.radians(), -10.5_f64.to_radians());
    close(Angle::from_gradians(50.0_f64).degrees(), 45.0_f64);
    close(Angle::from_degrees(45.0_f64).gradians(), 50.0_f64);
    assert_eq!(angle.degree_component(), Ok(-11_i32));
    assert_eq!(angle.minute_component(), 30_i32);
    assert!(angle.second_component().abs() < 1e-9_f64);
    let positive = Angle::from_dms(12, 34, 56.789_f64).unwrap();
    assert_eq!((positive.degree_component(), positive.minute_component()), (Ok(12_i32), 34_i32));
    assert!((positive.second_component() - 56.789_f64).abs() < 1e-9_f64);
    assert_eq!(condition(Angle::from_degrees(1e300_f64).degree_component()), Condition::InvalidArgument);
}

/// Verifies: [ISO/IEC 13249-3:2016 §16.1.8 Description 2, 4, 5, 7] the
/// "D M S" string rounds the seconds (carrying 60″ into the minutes and
/// 60′ into the degrees), refuses a negative digit count with F02, caps
/// the digits at ST_MaxAngleString, and reads back by the same convention.
#[test]
fn degree_minute_second_strings() {
    let positive = Angle::from_dms(12, 34, 56.789_f64).unwrap();
    assert_eq!(positive.string(2).unwrap(), "12 34 56.79");
    assert_eq!(positive.string(0).unwrap(), "12 34 57");
    assert_eq!(Angle::from_degrees(-10.5_f64).string(1).unwrap(), "-11 30 0.0");
    assert_eq!(Angle::from_dms(1, 59, 59.996_f64).unwrap().string(2).unwrap(), "2 0 0.00");
    assert_eq!(condition(positive.string(-1)), Condition::InvalidArgument);
    assert_eq!(positive.string(1000).unwrap().len(), super::super::angle::MAX_ANGLE_STRING);
    for degrees in [12.582_441_4_f64, -10.5_f64, -0.25_f64, 359.999_f64] {
        let text = Angle::from_degrees(degrees).string(8).unwrap();
        close(Angle::from_string(&text).unwrap().degrees(), degrees);
    }
    for bad in ["", "1 2", "1 2 3 4", "1.5 0 0", "1 60 0", "1 0 60", "a b c"] {
        assert_eq!(condition(Angle::from_string(bad)), Condition::InvalidArgument, "{bad}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 §16.1.10 to §16.1.13, §16.1.19]
/// arithmetic, division by zero raising F13, and the ordering function.
#[test]
fn arithmetic_and_ordering() {
    let a = Angle::new(1.0_f64);
    let b = Angle::new(0.25_f64);
    close((a + b).radians(), 1.25_f64);
    close(a.subtract(b).radians(), 0.75_f64);
    close(a.multiply(3.0_f64).radians(), 3.0_f64);
    close(a.divide(4.0_f64).unwrap().radians(), 0.25_f64);
    assert_eq!(condition(a.divide(0.0_f64)), Condition::DivisionByZero);
    assert_eq!((a.ordering_compare(b), b.ordering_compare(a), a.ordering_compare(a)), (1_i32, -1_i32, 0_i32));
    assert_eq!(Angle::new(f64::NAN).ordering_compare(a), 0_i32);
}

/// Verifies: [ISO/IEC 13249-3:2016 §16.1.14, §16.1.17, §16.1.21] the
/// `<angle text representation>` in each unit and any case, written in
/// radians that read back exactly; F22 for text outside the BNF and F02
/// for its units never arises since the keywords pick them.
#[test]
fn well_known_text() {
    close(Angle::from_text("ANGLE DEGREES(90)").unwrap().radians(), FRAC_PI_2);
    close(Angle::from_text("  angle gradians ( 1E2 ) ").unwrap().radians(), FRAC_PI_2);
    close(Angle::from_text("ANGLE RADIANS(-0.5)").unwrap().radians(), -0.5_f64);
    let angle = Angle::new(0.1_f64 + 0.2_f64);
    assert_eq!(angle.as_text(), "ANGLE RADIANS(0.30000000000000004)");
    assert_eq!(Angle::from_text(&angle.as_text()), Ok(angle));
    for bad in [
        "",
        "ANGLE",
        "ANGLEDEGREES(1)",
        "ANGLE MINUTES(1)",
        "ANGLE DEGREES 1",
        "ANGLE DEGREES(1",
        "ANGLE DEGREES(1) x",
        "ANGLE DEGREES()",
        "DIRECTION(N 1)",
    ] {
        assert_eq!(condition(Angle::from_text(bad)), Condition::InvalidText, "{bad}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 §16.1.15, §16.1.16, §16.1.18] the GML
/// Angle element, written in radians and read in radians, degrees or
/// gradians by its uom; F24 without the element or a number, F14 for a
/// unit that is not angular.
#[test]
fn gml_representation() {
    let angle = Angle::new(0.75_f64);
    let gml = angle.as_gml();
    assert!(gml.starts_with("<gml:Angle xmlns:gml=\"http://www.opengis.net/gml\" uom=\"urn:ogc:def:uom:EPSG::9101\">0.75<"), "{gml}");
    assert_eq!(Angle::from_gml(&gml), Ok(angle));
    close(Angle::from_gml(r#"<Angle uom="urn:ogc:def:uom:EPSG::9102">90</Angle>"#).unwrap().radians(), FRAC_PI_2);
    close(Angle::from_gml("<x:Angle  uom = 'grad' >100</x:Angle>").unwrap().radians(), FRAC_PI_2);
    close(Angle::from_gml("<Angle>2</Angle>").unwrap().radians(), 2.0_f64);
    close(Angle::from_gml(r#"<Angle uom="DEGREE">180</Angle>"#).unwrap().radians(), PI);
    for bad in ["", "<Angles>1</Angles>", "<Angle/>", "<Angle>x</Angle>", "<Angle>1", "<Angle>1e999</Angle>"] {
        assert_eq!(condition(Angle::from_gml(bad)), Condition::InvalidGml, "{bad}");
    }
    assert_eq!(condition(Angle::from_gml(r#"<Angle uom="metre">1</Angle>"#)), Condition::UnsupportedUnit);
    assert_eq!(condition(Angle::from_gml(r#"<Angle uom="EPSG::9001">1</Angle>"#)), Condition::UnsupportedUnit);
}
