//! Requirements of ST_Direction.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use super::super::angle::Angle;
use super::super::direction::{Direction, Measure, Reference};
use super::super::{Condition, Fault};

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12_f64, "{actual} is not {expected}");
}

fn condition<T: std::fmt::Debug>(outcome: Result<T, Fault>) -> Condition {
    outcome.expect_err("a fault").condition
}

fn degrees(value: f64) -> Angle {
    Angle::from_degrees(value)
}

/// Verifies: [ISO/IEC 13249-3:2016 §16.2.2 Description 2, 4, 7, §16.2.4]
/// the north azimuth in [0, 2π) (F02 at each bound), bearings in each
/// quadrant (F02 for bad indicators or angles beyond π/2), north and south
/// azimuths, and the azimuth mutator.
#[test]
fn constructors_from_angles() {
    close(Direction::new(0.0_f64).unwrap().radians(), 0.0_f64);
    assert_eq!(condition(Direction::new(-0.1_f64)), Condition::InvalidArgument);
    assert_eq!(condition(Direction::new(TAU)), Condition::InvalidArgument);
    let quadrants = [("N", "E", 30.0_f64), ("S", "E", 150.0_f64), ("S", "W", 210.0_f64), ("N", "W", 330.0_f64)];
    for (ns, ew, azimuth) in quadrants {
        close(Direction::bearing(ns, degrees(30.0_f64), ew).unwrap().angle_nazimuth().degrees(), azimuth);
    }
    close(Direction::bearing("N", degrees(0.0_f64), "W").unwrap().radians(), 0.0_f64);
    assert_eq!(condition(Direction::bearing("X", degrees(1.0_f64), "E")), Condition::InvalidArgument);
    assert_eq!(condition(Direction::bearing("N", degrees(1.0_f64), "X")), Condition::InvalidArgument);
    assert_eq!(condition(Direction::bearing("N", degrees(91.0_f64), "E")), Condition::InvalidArgument);
    assert_eq!(condition(Direction::bearing("N", degrees(-1.0_f64), "E")), Condition::InvalidArgument);
    close(Direction::azimuth("N", degrees(200.0_f64)).unwrap().angle_nazimuth().degrees(), 200.0_f64);
    close(Direction::azimuth("S", degrees(30.0_f64)).unwrap().angle_nazimuth().degrees(), 210.0_f64);
    close(Direction::azimuth("S", degrees(200.0_f64)).unwrap().angle_nazimuth().degrees(), 20.0_f64);
    assert_eq!(condition(Direction::azimuth("E", degrees(1.0_f64))), Condition::InvalidArgument);
    assert_eq!(condition(Direction::azimuth("S", Angle::new(TAU))), Condition::InvalidArgument);
    close(Direction::with_nazimuth(Angle::new(1.0_f64)).unwrap().radians(), 1.0_f64);
    assert_eq!(condition(Direction::with_nazimuth(Angle::new(7.0_f64))), Condition::InvalidArgument);
}

/// Verifies: [ISO/IEC 13249-3:2016 §16.2.2 Description 9, 12] directions
/// from points and lines are clockwise from north (+y); F17, F18, F19,
/// F20 and F21 as for ST_Angle.
#[test]
fn constructors_from_geometry() {
    let origin: &[f64] = &[0.0_f64, 0.0_f64];
    close(Direction::from_points(origin, &[0.0_f64, 2.0_f64]).unwrap().radians(), 0.0_f64);
    close(Direction::from_points(origin, &[1.0_f64, 1.0_f64]).unwrap().radians(), FRAC_PI_4);
    close(Direction::from_points(origin, &[0.0_f64, -1.0_f64]).unwrap().radians(), PI);
    close(Direction::from_points(origin, &[-1.0_f64, 0.0_f64]).unwrap().radians(), 1.5_f64 * PI);
    assert_eq!(condition(Direction::from_points(&[], origin)), Condition::EmptyPoint);
    assert_eq!(condition(Direction::from_points(origin, &[f64::INFINITY, 0.0_f64])), Condition::PointNotWellFormed);
    assert_eq!(condition(Direction::from_points(origin, origin)), Condition::PointsEqual);
    close(Direction::from_line(&[origin, &[1.0_f64, 0.0_f64]]).unwrap().radians(), FRAC_PI_2);
    assert_eq!(condition(Direction::from_line(&[origin, origin, origin])), Condition::NotALine);
    assert_eq!(condition(Direction::from_line(&[origin, origin])), Condition::DegenerateLine);
}

/// Verifies: [ISO/IEC 13249-3:2016 §16.2.8 to §16.2.16] the nine strings
/// in each quadrant, with the quadrant boundaries of Description 4 (90°
/// is N E, 180° S E, 270° N W), and F02 for digits below zero or beyond
/// ST_MaxDirectionString less each method's fixed characters.
#[test]
fn bearing_and_azimuth_strings() {
    let d = |value: f64| Direction::with_nazimuth(degrees(value)).unwrap();
    let bearing = |value: f64| d(value).string(Reference::Bearing, Measure::Degrees, 1_i32).unwrap();
    assert_eq!(
        [bearing(30.0_f64), bearing(90.0_f64), bearing(120.0_f64), bearing(180.0_f64)],
        ["N 30.0 E", "N 90.0 E", "S 60.0 E", "S 0.0 E"]
    );
    assert_eq!([bearing(200.0_f64), bearing(270.0_f64), bearing(300.0_f64)], ["S 20.0 W", "N 90.0 W", "N 60.0 W"]);
    let direction = d(210.5_f64);
    assert_eq!(direction.string(Reference::Bearing, Measure::Radians, 3_i32).unwrap(), "S 0.532 W");
    assert_eq!(direction.string(Reference::Bearing, Measure::Dms, 0_i32).unwrap(), "S 30 30 0 W");
    assert_eq!(direction.string(Reference::North, Measure::Radians, 4_i32).unwrap(), "N 3.6739");
    assert_eq!(direction.string(Reference::North, Measure::Degrees, 2_i32).unwrap(), "N 210.50");
    assert_eq!(direction.string(Reference::North, Measure::Dms, 1_i32).unwrap(), "N 210 30 0.0");
    assert_eq!(direction.string(Reference::South, Measure::Radians, 4_i32).unwrap(), "S 0.5323");
    assert_eq!(direction.string(Reference::South, Measure::Degrees, 1_i32).unwrap(), "S 30.5");
    assert_eq!(d(10.0_f64).string(Reference::South, Measure::Dms, 0_i32).unwrap(), "S 190 0 0");
    assert_eq!(condition(direction.string(Reference::North, Measure::Degrees, -1)), Condition::InvalidArgument);
    let limits = [
        (Reference::Bearing, Measure::Radians, 58_i32),
        (Reference::Bearing, Measure::Degrees, 57_i32),
        (Reference::Bearing, Measure::Dms, 51_i32),
        (Reference::North, Measure::Radians, 60_i32),
        (Reference::South, Measure::Degrees, 58_i32),
        (Reference::South, Measure::Dms, 52_i32),
    ];
    for (reference, measure, limit) in limits {
        assert!(direction.string(reference, measure, limit).is_ok(), "{reference:?} {measure:?}");
        assert_eq!(condition(direction.string(reference, measure, limit + 1)), Condition::InvalidArgument);
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 §16.2.17, §16.2.18, §16.2.21] adding
/// and subtracting angles reduce into [0, 2π) by whole turns, and the
/// ordering function compares azimuths.
#[test]
fn arithmetic_and_ordering() {
    let d = Direction::with_nazimuth(degrees(350.0_f64)).unwrap();
    close(d.add_angle(degrees(20.0_f64)).angle_nazimuth().degrees(), 10.0_f64);
    close(Direction::new(TAU - 0.5_f64).unwrap().add_angle(Angle::new(0.5_f64)).radians(), 0.0_f64);
    close(d.add_angle(degrees(740.0_f64)).angle_nazimuth().degrees(), 10.0_f64);
    close(d.subtract_angle(degrees(-20.0_f64)).angle_nazimuth().degrees(), 10.0_f64);
    close(Direction::new(0.1_f64).unwrap().subtract_angle(degrees(720.0_f64)).radians(), 0.1_f64);
    close(Direction::new(0.0_f64).unwrap().subtract_angle(Angle::new(f64::EPSILON * 1e-3_f64)).radians(), 0.0_f64);
    let e = Direction::new(1.0_f64).unwrap();
    assert_eq!((d.ordering_compare(e), e.ordering_compare(d), d.ordering_compare(d)), (1_i32, -1_i32, 0_i32));
}

/// Verifies: [ISO/IEC 13249-3:2016 §16.2.5, §16.2.19, §16.2.23] the
/// <direction text representation> reads and writes exactly; F22 outside
/// the BNF and F02 for radians outside [0, 2π).
#[test]
fn well_known_text() {
    let d = Direction::new(1.25_f64).unwrap();
    assert_eq!(d.as_text(), "DIRECTION(N 1.25)");
    assert_eq!(Direction::from_text(&d.as_text()), Ok(d));
    close(Direction::from_text(" direction ( n 0.5E0 ) ").unwrap().radians(), 0.5_f64);
    assert_eq!(condition(Direction::from_text("DIRECTION(N 7)")), Condition::InvalidArgument);
    for bad in ["", "DIRECTION", "DIRECTION(1)", "DIRECTION(S 1)", "DIRECTION(N1)", "DIRECTION(N 1", "DIRECTION(N 1) x", "DIRECTION N 1"] {
        assert_eq!(condition(Direction::from_text(bad)), Condition::InvalidText, "{bad}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 §16.2.6, §16.2.7, §16.2.20] the GML
/// DirectionVector reads back; F24 without a horizontal angle, F02 if it
/// is not a north azimuth.
#[test]
fn gml_representation() {
    let d = Direction::new(2.0_f64).unwrap();
    let gml = d.as_gml();
    assert!(gml.contains("<gml:horizontalAngle uom=\"urn:ogc:def:uom:EPSG::9101\">2</gml:horizontalAngle>"), "{gml}");
    assert_eq!(Direction::from_gml(&gml), Ok(d));
    close(
        Direction::from_gml(r#"<DirectionVector><horizontalAngle uom="deg">90</horizontalAngle></DirectionVector>"#).unwrap().radians(),
        FRAC_PI_2,
    );
    assert_eq!(condition(Direction::from_gml("<DirectionVector/>")), Condition::InvalidGml);
    assert_eq!(condition(Direction::from_gml("<horizontalAngle>-1</horizontalAngle>")), Condition::InvalidArgument);
}
