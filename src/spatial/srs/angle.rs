//! The ST_Angle type of ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §16.1: an
//! angle held in radians (Description 2), its constructors, unit
//! conversions, degrees-minutes-seconds components and strings,
//! arithmetic, ordering, and its text and GML representations.
//!
//! ST_ApproximatePi, the implementation-defined π of §16.1.4 and its
//! peers, is the `f64` nearest π. Readings of the text are these.
//!
//! - §16.1.5 to §16.1.7 take the components by FLOOR, so the degree
//!   component of a negative angle is the integer below it and the minute
//!   and second components count up from there: −10.5° is −11° 30′ 0″.
//!   The parenthesis of §16.1.7's second component is misplaced (it
//!   multiplies D + M/60 by 3600, not the difference); the seconds are
//!   (degrees − D − M/60) × 3600, which the Description's "seconds part"
//!   means.
//! - §16.1.8 rounds the seconds, and a rounding up to 60″ carries into
//!   the minutes and degrees. ST_String(CHARACTER VARYING) reads "D M S"
//!   by the same FLOOR convention, so it inverts ST_String.
//! - §16.1.2 Description 17 i and 19 make the angle between directions
//!   the lesser of the two clockwise angles, in [0, π]; the Definition's
//!   CASE, which picks the negative difference, is not followed.

use std::cmp::Ordering;
use std::f64::consts::{PI, TAU};
use std::ops::Add;

use super::direction::{Direction, line_ends, point};
use super::fault::{Condition, Fault, Outcome};
use super::notation::{self, GML_NAMESPACE, Scanner};
use super::number;

/// Degrees and gradians in a half turn.
const HALF_TURN_DEGREES: f64 = 180.0;
const HALF_TURN_GRADIANS: f64 = 200.0;
/// Minutes in a degree, seconds in a minute.
const SIXTY: f64 = 60.0;
/// ST_MaxAngleString, the implementation-defined longest ST_String.
pub const MAX_ANGLE_STRING: usize = 64;
/// The characters of ST_String besides the degree digits and the
/// seconds' decimals (§16.1.8 Description 4 b).
const STRING_OVERHEAD: usize = 7;

/// An ST_Angle value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Angle {
    radians: f64,
}

/// A unit indicator of ST_Angle(CHARACTER, DOUBLE PRECISION).
fn units_factor(units: &str) -> Option<f64> {
    match units {
        "R" => Some(1.0_f64),
        "D" => Some(PI / HALF_TURN_DEGREES),
        "G" => Some(PI / HALF_TURN_GRADIANS),
        _ => None,
    }
}

/// F02 for a minute or second count outside [0, 60).
fn sexagesimal(value: f64, what: &str) -> Outcome<()> {
    if (0.0_f64..SIXTY).contains(&value) {
        Ok(())
    } else {
        Err(Fault::argument(format!("[ISO/IEC 13249-3:2016 §16.1.2 Description 8, 12, 13] {what} out of range: {value}")))
    }
}

impl Angle {
    /// ST_Angle(DOUBLE PRECISION): `radians`.
    pub fn new(radians: f64) -> Angle {
        Angle { radians }
    }

    /// ST_Angle(CHARACTER, DOUBLE PRECISION): `angle` in radians ('R'),
    /// degrees ('D') or gradians ('G').
    ///
    /// # Errors
    ///
    /// F02 invalid angle units for any other indicator.
    pub fn with_units(units: &str, angle: f64) -> Outcome<Angle> {
        let factor = units_factor(units)
            .ok_or_else(|| Fault::argument(format!("[ISO/IEC 13249-3:2016 §16.1.2 Description 5a] invalid angle units: {units}")))?;
        Ok(Angle::new(angle * factor))
    }

    /// ST_Angle(INTEGER, DOUBLE PRECISION): `degrees` and `minutes`, the
    /// minutes taking the sign of a negative `degrees`.
    ///
    /// # Errors
    ///
    /// F02 minutes out of range outside [0, 60).
    pub fn from_degrees_minutes(degrees: i32, minutes: f64) -> Outcome<Angle> {
        sexagesimal(minutes, "minutes")?;
        Ok(Angle::from_parts(degrees, minutes / SIXTY))
    }

    /// ST_Angle(INTEGER, INTEGER, DOUBLE PRECISION): `degrees`, `minutes`
    /// and `seconds`, the latter two taking the sign of a negative
    /// `degrees`.
    ///
    /// # Errors
    ///
    /// F02 minutes or seconds out of range outside [0, 60).
    pub fn from_dms(degrees: i32, minutes: i32, seconds: f64) -> Outcome<Angle> {
        sexagesimal(f64::from(minutes), "minutes")?;
        sexagesimal(seconds, "seconds")?;
        Ok(Angle::from_parts(degrees, f64::from(minutes) / SIXTY + seconds / (SIXTY * SIXTY)))
    }

    fn from_parts(degrees: i32, fraction: f64) -> Angle {
        let degrees = f64::from(degrees);
        let total = if degrees >= 0.0_f64 { degrees + fraction } else { degrees - fraction };
        Angle::new(total.to_radians())
    }

    /// ST_Angle(ST_Point, ST_Point, ST_Point): the lesser angle at
    /// `at` between the directions to `a` and to `another`. Each point is
    /// its coordinates, x and y first; no coordinates is the empty point.
    ///
    /// # Errors
    ///
    /// F17, F18 and F19 in the order of §16.1.2 Description 17 a to h.
    pub fn at_point(at: &[f64], a: &[f64], another: &[f64]) -> Outcome<Angle> {
        let at = point(at)?;
        let a = point(a)?;
        let another = point(another)?;
        let first = Direction::between_points(at, a)?;
        let second = Direction::between_points(at, another)?;
        Ok(Angle::between(first, second))
    }

    /// ST_Angle(ST_Direction, ST_Direction): the lesser of the clockwise
    /// angles from one direction to the other, in [0, π].
    pub fn between(direction: Direction, another: Direction) -> Angle {
        let clockwise = (another.radians() - direction.radians()).rem_euclid(TAU);
        Angle::new(clockwise.min(TAU - clockwise))
    }

    /// ST_Angle(ST_LineString, ST_LineString): the angle between the
    /// directions of two lines, each given by its points.
    ///
    /// # Errors
    ///
    /// F20 if a line has other than two points, F21 if it is closed, the
    /// first line checked first; F17 and F18 for empty or ill-formed points.
    pub fn between_lines(line: &[&[f64]], another: &[&[f64]]) -> Outcome<Angle> {
        let first = line_ends(line)?;
        let second = line_ends(another)?;
        Ok(Angle::between(Direction::between_points(first[0], first[1])?, Direction::between_points(second[0], second[1])?))
    }

    /// ST_Radians().
    pub fn radians(self) -> f64 {
        self.radians
    }

    /// ST_Degrees().
    pub fn degrees(self) -> f64 {
        self.radians * HALF_TURN_DEGREES / PI
    }

    /// ST_Gradians().
    pub fn gradians(self) -> f64 {
        self.radians / PI * HALF_TURN_GRADIANS
    }

    /// ST_Degrees(DOUBLE PRECISION), the mutator.
    pub fn from_degrees(degrees: f64) -> Angle {
        Angle::new(PI * degrees / HALF_TURN_DEGREES)
    }

    /// ST_Gradians(DOUBLE PRECISION), the mutator.
    pub fn from_gradians(gradians: f64) -> Angle {
        Angle::new(PI * gradians / HALF_TURN_GRADIANS)
    }

    /// The FLOOR components D, M and S of §16.1.5 to §16.1.7.
    fn components(self) -> (f64, f64, f64) {
        let degrees = self.degrees();
        let whole = degrees.floor();
        let minutes = ((degrees - whole) * SIXTY).abs().floor();
        (whole, minutes, (degrees - whole - minutes / SIXTY) * SIXTY * SIXTY)
    }

    /// ST_DegreeComponent(): FLOOR of the degrees.
    ///
    /// # Errors
    ///
    /// F02 if the component exceeds the INTEGER range.
    pub fn degree_component(self) -> Outcome<i32> {
        integer(self.components().0)
    }

    /// ST_MinuteComponent(), 0 to 59.
    pub fn minute_component(self) -> i32 {
        integer(self.components().1).unwrap_or_default()
    }

    /// ST_SecondComponent(), in [0, 60).
    pub fn second_component(self) -> f64 {
        self.components().2
    }

    /// ST_String(INTEGER): "D M S" with the seconds rounded to `places`
    /// decimals, or fewer if ST_MaxAngleString would be exceeded. ST_String()
    /// is `string(2)`.
    ///
    /// # Errors
    ///
    /// F02 if `places` is negative or the degrees exceed the INTEGER range.
    pub fn string(self, places: i32) -> Outcome<String> {
        let places = usize::try_from(places)
            .map_err(|_| Fault::argument("[ISO/IEC 13249-3:2016 §16.1.8 Description 5a] number of digits is negative"))?;
        let (degrees, minutes, seconds) = self.components();
        integer(degrees)?;
        let digits = number::fixed(degrees, 0).len();
        let places = places.min(MAX_ANGLE_STRING.saturating_sub(STRING_OVERHEAD.saturating_add(digits)));
        let mut text = number::fixed(seconds, places);
        let (mut minutes, mut degrees) = (minutes, degrees);
        if number::value(&text).is_some_and(|s| s >= SIXTY) {
            text = number::fixed(0.0_f64, places);
            minutes += 1.0_f64;
        }
        if minutes >= SIXTY {
            minutes = 0.0_f64;
            degrees += 1.0_f64;
        }
        Ok(format!("{} {} {text}", number::fixed(degrees, 0), number::fixed(minutes, 0)))
    }

    /// ST_String(CHARACTER VARYING): the angle "D M S", the degrees an
    /// integer and the minutes and seconds in [0, 60), read as
    /// D + M/60 + S/3600 degrees so that it inverts [`Angle::string`].
    ///
    /// # Errors
    ///
    /// F02 if `dms` is not three such numbers separated by spaces.
    pub fn from_string(dms: &str) -> Outcome<Angle> {
        let invalid =
            || Fault::argument(format!("[ISO/IEC 13249-3:2016 §16.1.8 Description 7c] not a degrees, minutes and seconds string: {dms}"));
        let mut scanner = Scanner::new(dms);
        let parts = [scanner.number(), scanner.number(), scanner.number()];
        let [Some(degrees), Some(minutes), Some(seconds)] = parts else {
            return Err(invalid());
        };
        let well_formed = scanner.finished() && degrees.fract() == 0.0_f64;
        if !well_formed || sexagesimal(minutes, "minutes").is_err() || sexagesimal(seconds, "seconds").is_err() {
            return Err(invalid());
        }
        Ok(Angle::from_degrees(degrees + minutes / SIXTY + seconds / (SIXTY * SIXTY)))
    }

    /// ST_Subtract(ST_Angle).
    #[must_use]
    pub fn subtract(self, angle: Angle) -> Angle {
        Angle::new(self.radians - angle.radians)
    }

    /// ST_Multiply(DOUBLE PRECISION).
    #[must_use]
    pub fn multiply(self, factor: f64) -> Angle {
        Angle::new(self.radians * factor)
    }

    /// ST_Divide(DOUBLE PRECISION).
    ///
    /// # Errors
    ///
    /// F13 attempted division by zero.
    pub fn divide(self, divisor: f64) -> Outcome<Angle> {
        if divisor == 0.0_f64 {
            return Err(Fault::new(Condition::DivisionByZero, "[ISO/IEC 13249-3:2016 §16.1.13 Description 2a] attempted division by zero"));
        }
        Ok(Angle::new(self.radians / divisor))
    }

    /// ST_OrderingCompare(ST_Angle, ST_Angle): −1, 0 or 1.
    pub fn ordering_compare(self, another: Angle) -> i32 {
        match self.radians.partial_cmp(&another.radians) {
            Some(Ordering::Less) => -1,
            Some(Ordering::Greater) => 1,
            Some(Ordering::Equal) | None => 0,
        }
    }

    /// ST_AsText(): `ANGLE RADIANS(r)`, radians so that the text reads
    /// back exactly.
    pub fn as_text(self) -> String {
        format!("ANGLE RADIANS({})", number::shortest(self.radians))
    }

    /// ST_AngleFromText: the `<angle text representation>`.
    ///
    /// # Errors
    ///
    /// F22 if `text` is not producible in the BNF of §16.1.21.
    pub fn from_text(text: &str) -> Outcome<Angle> {
        let mut scanner = Scanner::new(text);
        let invalid = || Fault::text(format!("[ISO/IEC 13249-3:2016 §16.1.17 Description 2a] not an <angle text representation>: {text}"));
        if !scanner.keyword("ANGLE") {
            return Err(invalid());
        }
        let units = [("DEGREES", "D"), ("GRADIANS", "G"), ("RADIANS", "R")].into_iter().find(|(word, _)| scanner.keyword(word));
        let Some((_, units)) = units else {
            return Err(invalid());
        };
        let Some(value) = scanner.symbol('(').then(|| scanner.number()).flatten() else {
            return Err(invalid());
        };
        if scanner.symbol(')') && scanner.finished() { Angle::with_units(units, value) } else { Err(invalid()) }
    }

    /// ST_AsGML(): an Angle element in radians.
    pub fn as_gml(self) -> String {
        let element = notation::measure("Angle", self.radians);
        element.replacen("<gml:Angle ", &format!("<gml:Angle xmlns:gml=\"{GML_NAMESPACE}\" "), 1)
    }

    /// ST_AngleFromGML and ST_GMLToSQL: the Angle element of `gml`.
    ///
    /// # Errors
    ///
    /// F24 if there is no Angle element with a number, F14 if its `uom`
    /// is not an angular unit.
    pub fn from_gml(gml: &str) -> Outcome<Angle> {
        notation::angle_measure(gml, "Angle", "ISO/IEC 13249-3:2016 §16.1.15 Description 2").map(Angle::new)
    }
}

/// ST_Add(ST_Angle).
impl Add for Angle {
    type Output = Angle;

    fn add(self, angle: Angle) -> Angle {
        Angle::new(self.radians + angle.radians)
    }
}

/// `value`, an integral `f64`, as an INTEGER.
fn integer(value: f64) -> Outcome<i32> {
    number::whole(value)
        .and_then(|whole| i32::try_from(whole).ok())
        .ok_or_else(|| Fault::argument(format!("[ISO/IEC 13249-3:2016 §16.1.5 Description 2] {value} degrees exceeds the INTEGER range")))
}
