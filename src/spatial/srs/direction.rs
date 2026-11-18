//! The ST_Direction type of ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §16.2: a direction held as its north azimuth, the angle clockwise from
//! true north in [0, 2π) (§16.2.1 Description 2 and 3), its
//! constructors from bearings, azimuths, points and lines, its bearing
//! and azimuth strings, angle arithmetic, ordering, and its text and GML
//! representations.
//!
//! Directions between points are planar: the azimuth of the vector from
//! one point to the other in the x (east) and y (north) axes, the reading
//! of "measured clockwise from True North" (§16.2.2 Description 9 f) for
//! a projected or unspecified system. The strings round, which is the
//! implementation-defined choice of §16.2.8 to §16.2.16 Description 5.

use std::cmp::Ordering;
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::ops::Add;

use super::angle::Angle;
use super::fault::{Condition, Fault, Outcome};
use super::notation::{self, GML_NAMESPACE, Scanner};
use super::number;

/// ST_MaxDirectionString, the implementation-defined longest string.
pub const MAX_DIRECTION_STRING: usize = 64;
/// Degrees of a quarter, half and three-quarter turn.
const QUARTER: f64 = 90.0;
const HALF_TURN: f64 = 180.0;
const THREE_QUARTERS: f64 = 270.0;
const TURN: f64 = 360.0;

/// Which reference a direction string measures from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reference {
    /// A quadrant bearing, "N a E" (ST_…Bearing).
    Bearing,
    /// A north azimuth, "N a" (ST_…NAzimuth).
    North,
    /// A south azimuth, "S a" (ST_…SAzimuth).
    South,
}

/// The measure a direction string writes its angle in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Measure {
    /// ST_Radian…, decimal radians.
    Radians,
    /// ST_Degrees…, decimal degrees.
    Degrees,
    /// ST_DMS…, ST_String of the angle.
    Dms,
}

/// An ST_Direction value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Direction {
    azimuth: Angle,
}

/// The x and y of a point given by its coordinates.
///
/// # Errors
///
/// F17 for no coordinates, F18 for fewer than two or any not finite.
pub(super) fn point(coordinates: &[f64]) -> Outcome<[f64; 2]> {
    if coordinates.is_empty() {
        return Err(Fault::new(Condition::EmptyPoint, "[ISO/IEC 13249-3:2016 §16.2.2 Description 9a] empty point value"));
    }
    let finite = coordinates.iter().all(|c| c.is_finite());
    let (Some(x), Some(y), true) = (coordinates.first(), coordinates.get(1), finite) else {
        return Err(Fault::new(Condition::PointNotWellFormed, "[ISO/IEC 13249-3:2016 §16.2.2 Description 9b] point value not well formed"));
    };
    Ok([*x, *y])
}

/// The start and end of a line given by its points.
///
/// # Errors
///
/// F20 for other than two points, F21 if they are equal (the line is
/// closed), F17 and F18 for empty or ill-formed points.
pub(super) fn line_ends(line: &[&[f64]]) -> Outcome<[[f64; 2]; 2]> {
    let [start, end] = line else {
        return Err(Fault::new(
            Condition::NotALine,
            format!("[ISO/IEC 13249-3:2016 §16.2.2 Description 12a] linestring of {} points is not a line", line.len()),
        ));
    };
    let ends = [point(start)?, point(end)?];
    if same_point(ends[0], ends[1]) {
        return Err(Fault::new(
            Condition::DegenerateLine,
            "[ISO/IEC 13249-3:2016 §16.2.2 Description 12b] degenerate line has no direction",
        ));
    }
    Ok(ends)
}

/// Whether two points coincide; a difference of zero is equality of
/// finite coordinates, signed zeros included.
fn same_point(a: [f64; 2], b: [f64; 2]) -> bool {
    a[0] - b[0] == 0.0_f64 && a[1] - b[1] == 0.0_f64
}

/// F02 unless `radians` is a north azimuth, in [0, 2π).
fn azimuth(radians: f64, rule: &str) -> Outcome<Direction> {
    if (0.0_f64..TAU).contains(&radians) {
        Ok(Direction { azimuth: Angle::new(radians) })
    } else {
        Err(Fault::argument(format!("[ISO/IEC 13249-3:2016 {rule}] {radians} radians is not in [0, 2π)")))
    }
}

/// `radians` reduced to [0, 2π) by whole turns (§16.2.17 Description 2).
fn reduce(radians: f64) -> f64 {
    let reduced = radians.rem_euclid(TAU);
    if reduced < TAU { reduced } else { 0.0_f64 }
}

impl Direction {
    /// ST_Direction(DOUBLE PRECISION): the north azimuth `radians`.
    ///
    /// # Errors
    ///
    /// F02 outside [0, 2π).
    pub fn new(radians: f64) -> Outcome<Direction> {
        azimuth(radians, "§16.2.2 Description 2a")
    }

    /// ST_Direction(CHARACTER, ST_Angle, CHARACTER): the bearing
    /// `north_south` `angle` `east_west`, such as N 30° E.
    ///
    /// # Errors
    ///
    /// F02 for an indicator other than 'N' or 'S', then 'E' or 'W', or an
    /// angle outside [0, π/2].
    pub fn bearing(north_south: &str, angle: Angle, east_west: &str) -> Outcome<Direction> {
        let a = angle.radians();
        let invalid = |what: &str| Fault::argument(format!("[ISO/IEC 13249-3:2016 §16.2.2 Description 4] invalid argument: {what}"));
        let south = indicator(north_south, "N", "S").ok_or_else(|| invalid(north_south))?;
        if !(0.0_f64..=FRAC_PI_2).contains(&a) {
            return Err(invalid("bearing angle outside [0, π/2]"));
        }
        let west = indicator(east_west, "E", "W").ok_or_else(|| invalid(east_west))?;
        let radians = match (south, west) {
            (false, false) => a,
            (true, false) => PI - a,
            (true, true) => PI + a,
            (false, true) => TAU - a,
        };
        Ok(Direction { azimuth: Angle::new(reduce(radians)) })
    }

    /// ST_Direction(CHARACTER, ST_Angle): the north ('N') or south ('S')
    /// azimuth `angle`.
    ///
    /// # Errors
    ///
    /// F02 for another indicator or an angle outside [0, 2π).
    pub fn azimuth(north_south: &str, angle: Angle) -> Outcome<Direction> {
        let south = indicator(north_south, "N", "S")
            .ok_or_else(|| Fault::argument(format!("[ISO/IEC 13249-3:2016 §16.2.2 Description 7a] invalid argument: {north_south}")))?;
        let given = azimuth(angle.radians(), "§16.2.2 Description 7b")?;
        if !south {
            return Ok(given);
        }
        let turned = if angle.degrees() < HALF_TURN { angle.radians() + PI } else { angle.radians() - PI };
        Ok(Direction { azimuth: Angle::new(reduce(turned)) })
    }

    /// The direction from `from` to `to`, x east and y north.
    ///
    /// # Errors
    ///
    /// F19 if the points are equal.
    pub fn between_points(from: [f64; 2], to: [f64; 2]) -> Outcome<Direction> {
        if same_point(from, to) {
            return Err(Fault::new(Condition::PointsEqual, "[ISO/IEC 13249-3:2016 §16.2.2 Description 9e] points are equal"));
        }
        let [x0, y0] = from;
        let [x1, y1] = to;
        Ok(Direction { azimuth: Angle::new(reduce((x1 - x0).atan2(y1 - y0))) })
    }

    /// ST_Direction(ST_Point, ST_Point), each point its coordinates.
    ///
    /// # Errors
    ///
    /// F17, F18 and F19 in the order of §16.2.2 Description 9 a to e.
    pub fn from_points(from: &[f64], to: &[f64]) -> Outcome<Direction> {
        let from = point(from)?;
        Direction::between_points(from, point(to)?)
    }

    /// ST_Direction(ST_LineString), the line its points.
    ///
    /// # Errors
    ///
    /// F20 not a line, F21 closed, then F17 and F18.
    pub fn from_line(line: &[&[f64]]) -> Outcome<Direction> {
        let [start, end] = line_ends(line)?;
        Direction::between_points(start, end)
    }

    /// ST_Radians().
    pub fn radians(self) -> f64 {
        self.azimuth.radians()
    }

    /// ST_AngleNAzimuth().
    pub fn angle_nazimuth(self) -> Angle {
        self.azimuth
    }

    /// ST_AngleNAzimuth(ST_Angle), the mutator.
    ///
    /// # Errors
    ///
    /// F02 outside [0, 2π).
    pub fn with_nazimuth(angle: Angle) -> Outcome<Direction> {
        azimuth(angle.radians(), "§16.2.4 Description 4c")
    }

    /// ST_AddAngle(ST_Angle), reduced to [0, 2π).
    #[must_use]
    pub fn add_angle(self, angle: Angle) -> Direction {
        Direction { azimuth: Angle::new(reduce(self.radians() + angle.radians())) }
    }

    /// ST_SubtractAngle(ST_Angle), reduced to [0, 2π).
    #[must_use]
    pub fn subtract_angle(self, angle: Angle) -> Direction {
        Direction { azimuth: Angle::new(reduce(self.radians() - angle.radians())) }
    }

    /// ST_OrderingCompare(ST_Direction, ST_Direction): −1, 0 or 1.
    pub fn ordering_compare(self, another: Direction) -> i32 {
        match self.radians().partial_cmp(&another.radians()) {
            Some(Ordering::Less) => -1,
            Some(Ordering::Greater) => 1,
            Some(Ordering::Equal) | None => 0,
        }
    }

    /// The quadrant bearing of §16.2.8 Description 4: N or S, the angle
    /// from that pole, E or W.
    fn quadrant(self) -> (&'static str, Angle, &'static str) {
        let degrees = self.azimuth.degrees();
        let half = Angle::from_degrees(HALF_TURN);
        if degrees <= QUARTER {
            ("N", self.azimuth, "E")
        } else if degrees <= HALF_TURN {
            ("S", half.subtract(self.azimuth), "E")
        } else if degrees < THREE_QUARTERS {
            ("S", self.azimuth.subtract(half), "W")
        } else {
            ("N", Angle::from_degrees(TURN).subtract(self.azimuth), "W")
        }
    }

    /// The south azimuth of §16.2.14 Description 3.
    fn south(self) -> Angle {
        let half = Angle::from_degrees(HALF_TURN);
        if self.azimuth.degrees() < HALF_TURN { self.azimuth.add(half) } else { self.azimuth.subtract(half) }
    }

    /// The nine bearing and azimuth strings of §16.2.8 to §16.2.16:
    /// ST_RadianBearing is `string(Reference::Bearing, Measure::Radians,
    /// places)`, ST_DMSSAzimuth `string(Reference::South, Measure::Dms,
    /// places)`, and so on.
    ///
    /// # Errors
    ///
    /// F02 if `places` is negative or above ST_MaxDirectionString less the
    /// method's fixed characters (§16.2.8 Description 2 and its peers).
    pub fn string(self, reference: Reference, measure: Measure, places: i32) -> Outcome<String> {
        let fixed = overhead(reference, measure);
        let limit = MAX_DIRECTION_STRING.saturating_sub(fixed);
        let places = usize::try_from(places).ok().filter(|p| *p <= limit).ok_or_else(|| {
            Fault::argument(format!(
                "[ISO/IEC 13249-3:2016 §16.2.8 Description 2] invalid argument: {places} decimal digits is outside [0, {limit}]"
            ))
        })?;
        let (prefix, angle, suffix) = match reference {
            Reference::Bearing => self.quadrant(),
            Reference::North => ("N", self.azimuth, ""),
            Reference::South => ("S", self.south(), ""),
        };
        let value = match measure {
            Measure::Radians => number::fixed(angle.radians(), places),
            Measure::Degrees => number::fixed(angle.degrees(), places),
            Measure::Dms => angle.string(i32::try_from(places).unwrap_or(i32::MAX))?,
        };
        let text = format!("{prefix} {value} {suffix}");
        Ok(text.trim_end().to_owned())
    }

    /// ST_AsText(): `DIRECTION(N r)`.
    pub fn as_text(self) -> String {
        format!("DIRECTION(N {})", number::shortest(self.radians()))
    }

    /// ST_DirectionFrmTxt: the \<direction text representation\>.
    ///
    /// # Errors
    ///
    /// F22 if `text` is not producible in the BNF of §16.2.23, F02 if its
    /// radians are outside [0, 2π) (§16.2.23 Description 1c).
    pub fn from_text(text: &str) -> Outcome<Direction> {
        let mut scanner = Scanner::new(text);
        let opened = scanner.keyword("DIRECTION") && scanner.symbol('(') && scanner.keyword("N");
        let value = opened.then(|| scanner.number()).flatten();
        let closed = scanner.symbol(')') && scanner.finished();
        match (value, closed) {
            (Some(radians), true) => azimuth(radians, "§16.2.23 Description 1c"),
            (Some(_) | None, false) | (None, true) => {
                Err(Fault::text(format!("[ISO/IEC 13249-3:2016 §16.2.19 Description 2a] not a <direction text representation>: {text}")))
            }
        }
    }

    /// ST_AsGML(): a GML DirectionVector of horizontal and vertical angles.
    pub fn as_gml(self) -> String {
        format!(
            "<gml:DirectionVector xmlns:gml=\"{GML_NAMESPACE}\">{}{}</gml:DirectionVector>",
            notation::measure("horizontalAngle", self.radians()),
            notation::measure("verticalAngle", 0.0_f64)
        )
    }

    /// ST_DirectionFrmGML and ST_GMLToSQL: the horizontal angle of the
    /// DirectionVector in `gml`.
    ///
    /// # Errors
    ///
    /// F24 without a horizontalAngle, F14 for a non-angular `uom`, F02
    /// outside [0, 2π).
    pub fn from_gml(gml: &str) -> Outcome<Direction> {
        let radians = notation::angle_measure(gml, "horizontalAngle", "ISO/IEC 13249-3:2016 §16.2.6 Description 2")?;
        azimuth(radians, "§16.2.6 Description 3")
    }
}

/// Whether `value` is `second` (true) or `first` (false), or none.
fn indicator(value: &str, first: &str, second: &str) -> Option<bool> {
    if value == first {
        Some(false)
    } else if value == second {
        Some(true)
    } else {
        None
    }
}

/// The fixed characters of each string method besides its decimals.
fn overhead(reference: Reference, measure: Measure) -> usize {
    match (reference, measure) {
        (Reference::Bearing, Measure::Radians) | (Reference::North | Reference::South, Measure::Degrees) => 6,
        (Reference::Bearing, Measure::Degrees) => 7,
        (Reference::Bearing, Measure::Dms) => 13,
        (Reference::North | Reference::South, Measure::Radians) => 4,
        (Reference::North | Reference::South, Measure::Dms) => 12,
    }
}
