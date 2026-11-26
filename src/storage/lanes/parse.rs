//! \[DESIGN\] A lane parsed into its column's type (RULES.md D1148): each
//! cell's text as a row of a copy gives it, a character string given a
//! dynamic parameter of the column's type [ISO/IEC 9075:1992 §17.9 GR5f]
//! and assigned to the column [§13.8 GR4b iii], written as the type's
//! record (`SqlType::encode_record`).
//!
//! The text of a common type, in its plainest form, is written straight
//! to its record, with no value made and nothing allocated: an exact
//! number of no more digits than the type keeps, an approximate number of
//! no more than an exact one holds, a string no longer than the type, a
//! datetime's string, a truth value. Any other cell, and every cell of
//! another type, is cast and assigned as the row does it and its value
//! written, so a lane's records and its first error are the rows'.

use crate::error::{Code, Result, SqlError};
use crate::params::conformed_row;
use crate::value::cast::at_precision;
use crate::value::datetime::{Datetime, Moment, Now, parse as written};
use crate::value::{CharString, CharUnits, DataType, Decimal, MAX_EXACT_PRECISION, NARROW_EXACT_PRECISION, Value, assign, pow10};

use super::{Lane, Records, set_null};

/// The rule a row's value is assigned to its column by, as an INSERT's
/// target gives it (`db::write`): so its errors cite what the row's do.
const ASSIGNED: &str = "ISO/IEC 9075:1992 §13.8 GR4b iii";

/// The space a cast trims a number's, a datetime's or a truth value's
/// string of [ISO/IEC 9075:1992 §6.10 GR3b, GR9a; ISO/IEC 9075-2:2023
/// §6.13 GR21a].
const SPACE: char = ' ';

/// The base of a numeral's digits.
const DECIMAL_BASE: u8 = 10;

/// \[ISO/IEC 9075-2:2023 §5.3\] `<boolean literal>`'s words, in any case.
const TRUE_WORD: &str = "TRUE";
const FALSE_WORD: &str = "FALSE";
/// A BOOLEAN's record, one byte, as its kind writes it (`types::boolean`):
/// 0 for False, 1 for True.
const FALSE_BYTE: u8 = 0;
const TRUE_BYTE: u8 = 1;

/// How a type's cells are written straight from their text, if they are.
#[derive(Debug, Clone, Copy)]
enum Fast {
    /// NUMERIC(precision, scale), its units in 64 bits or, `wide`, 128.
    Exact { precision: u32, scale: u32, wide: bool },
    /// FLOAT(precision).
    Approximate { precision: u32 },
    /// CHARACTER(length), or, `varying`, CHARACTER VARYING(length).
    Characters { length: u32, units: CharUnits, varying: bool },
    /// DATE, TIME(p) or TIMESTAMP(p), WITH TIME ZONE if `zoned`.
    Datetime { moment: Moment, precision: u32, zoned: bool },
    /// BOOLEAN.
    Boolean,
    /// None: every cell cast as the row casts it.
    Slow,
}

impl Fast {
    /// How cells of `t` are written straight.
    fn of(t: &DataType) -> Fast {
        match *t {
            DataType::Exact { precision, scale, .. } => Fast::Exact { precision, scale, wide: precision > NARROW_EXACT_PRECISION },
            DataType::Approximate { precision, .. } => Fast::Approximate { precision },
            DataType::Character { length, units } => Fast::Characters { length, units, varying: false },
            DataType::Varying { length, units } => Fast::Characters { length, units, varying: true },
            DataType::Date => Fast::Datetime { moment: Moment::Date, precision: 0, zoned: false },
            DataType::Time { precision, zoned } => Fast::Datetime { moment: Moment::Time, precision, zoned },
            DataType::Timestamp { precision, zoned } => Fast::Datetime { moment: Moment::Timestamp, precision, zoned },
            DataType::Boolean => Fast::Boolean,
            DataType::DecFloat { .. }
            | DataType::Binary { .. }
            | DataType::VarBinary { .. }
            | DataType::Lob { .. }
            | DataType::Interval(_)
            | DataType::Array(_)
            | DataType::Multiset(_)
            | DataType::Row(_)
            | DataType::Distinct(_)
            | DataType::Structured(_)
            | DataType::Reference(_)
            | DataType::Json
            | DataType::Xml(_)
            | DataType::Extension(_) => Fast::Slow,
        }
    }

    /// Writes the record of the cell `text` to `out` and says so, if it
    /// is sure to be the one the row's cast and assignment give; else
    /// writes nothing.
    fn write(self, text: &str, out: &mut Vec<u8>) -> bool {
        match self {
            Fast::Exact { precision, scale, wide } => exact(text, (precision, scale), wide, out),
            Fast::Approximate { precision } => approximate(text, precision, out),
            Fast::Characters { length, units, varying } => characters(text, (length, units), varying, out),
            Fast::Datetime { moment, precision, zoned } => datetime(text, (moment, precision, zoned), out),
            Fast::Boolean => boolean(text, out),
            Fast::Slow => false,
        }
    }
}

/// \[DESIGN\] (RULES.md D1148) The records of `lane`, of `rows` cells, a
/// column of type `t`: each cell's text as a copy's row gives it, a
/// character string, cast to `t` as a dynamic parameter of it [ISO/IEC
/// 9075:1992 §17.9 GR5f], assigned to the column [§13.8 GR4b iii, §9.2]
/// and written as `t`'s record; a null cell, and a cell whose cast is the
/// null value (BOOLEAN's 'UNKNOWN'), null.
///
/// # Errors
///
/// As `Lane::check` and `Lane::cell`; of the first cell that fails, the
/// error its row's cast or assignment gives; SQLCODE -910 of the first
/// cell whose cast warns, as of a row given with others
/// (`params::conformed_row`), so the caller loads its rows a row at a
/// time, which tells the warning; SQLCODE -804 if its records pass the
/// bounds a `u32` holds.
pub fn parse(t: &DataType, lane: &Lane, rows: usize) -> Result<Records> {
    lane.check(rows)?;
    let fast = Fast::of(t);
    let mut out = Records { bytes: Vec::with_capacity(lane.bytes.len()), ends: Vec::with_capacity(rows), nulls: vec![0; rows.div_ceil(8)] };
    for n in 0..rows {
        let written = match lane.cell(n)? {
            None => false,
            Some(text) => fast.write(text, &mut out.bytes) || slow(t, text, &mut out.bytes)?,
        };
        if !written {
            set_null(&mut out.nulls, n);
        }
        let end = u32::try_from(out.bytes.len())
            .map_err(|_| SqlError::new(Code::Dynamic, "[DESIGN] a copy's column parses to more octets of records than a column holds"))?;
        out.ends.push(end);
    }
    Ok(out)
}

/// Writes the record of the cell `text` to `out` as the row writes it:
/// cast to `t` as a dynamic parameter (`params::conformed_row`), assigned
/// to the column and encoded. Says whether it wrote one: none if the
/// value is null.
fn slow(t: &DataType, text: &str, out: &mut Vec<u8>) -> Result<bool> {
    let row = conformed_row(vec![Value::Char(CharString::from(text))], std::slice::from_ref(t))?;
    let value = assign(row.into_iter().next().unwrap_or(Value::Null), t, ASSIGNED)?;
    if value.is_null() {
        return Ok(false);
    }
    t.kind().encode_record_into(&value, t, out)?;
    Ok(true)
}

/// An exact numeral as written plainly, `[+|-] digits [. digits]`: its
/// sign, its units, and how many digits it has before the point, its
/// leading zeros not counted, and after it.
struct Numeral {
    /// Whether it is negative.
    negative: bool,
    /// Its digits, as one integer, unsigned.
    units: i128,
    /// Its digits before the point, from the first not zero.
    whole: u32,
    /// Its digits after the point.
    fraction: u32,
}

/// [ISO/IEC 9075:1992 §5.3] `text`, its spaces trimmed as a cast trims
/// them \[§6.10 GR3b\], read if it is a plain `<signed numeric literal>`
/// of no more digits than an exact number holds, so its value is exact
/// and none of its digits is cut (`cast::numeric_literal`). None if it
/// has an exponent, an underscore, a character of another kind, or no
/// digit: those the row's cast reads.
fn numeral(text: &str) -> Option<Numeral> {
    let trimmed = text.trim_matches(SPACE);
    let (negative, body) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let (whole, fraction) = body.split_once('.').unwrap_or((body, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || !digits(fraction) || (whole.is_empty() && fraction.is_empty()) {
        return None;
    }
    let significant = whole.trim_start_matches('0');
    let (whole_digits, fraction_digits) = (u32::try_from(significant.len()).ok()?, u32::try_from(fraction.len()).ok()?);
    if whole_digits.checked_add(fraction_digits)? > MAX_EXACT_PRECISION {
        return None;
    }
    let units = significant
        .bytes()
        .chain(fraction.bytes())
        .try_fold(0_i128, |u, d| u.checked_mul(i128::from(DECIMAL_BASE))?.checked_add(i128::from(d.wrapping_sub(b'0'))))?;
    Some(Numeral { negative, units, whole: whole_digits, fraction: fraction_digits })
}

/// [ISO/IEC 9075:1992 §6.10 GR3b, §9.2 GR3k] Writes the record of a
/// column of NUMERIC(`precision`, `scale`) of the numeral `text`, if it
/// is plain, has no more digits after the point than the scale, so none
/// is truncated, and no more before it than the precision leaves, so it
/// fits: its units at the scale, little-endian, in 64 bits or, `wide`,
/// 128 (`types::exact`).
fn exact(text: &str, (precision, scale): (u32, u32), wide: bool, out: &mut Vec<u8>) -> bool {
    let Some(n) = numeral(text) else { return false };
    if precision > MAX_EXACT_PRECISION || n.fraction > scale {
        return false;
    }
    if precision.checked_sub(scale).is_none_or(|room| n.whole > room) {
        return false;
    }
    let Some(units) = pow10(scale.saturating_sub(n.fraction)).and_then(|f| n.units.checked_mul(f)) else { return false };
    let Some(units) = (if n.negative { units.checked_neg() } else { Some(units) }) else { return false };
    if wide {
        out.extend_from_slice(&units.to_le_bytes());
        return true;
    }
    let Ok(narrow) = i64::try_from(units) else { return false };
    out.extend_from_slice(&narrow.to_le_bytes());
    true
}

/// [ISO/IEC 9075:1992 §6.10 GR4b, §9.2 GR3k] Writes the record of a
/// column of FLOAT(`precision`) of the numeral `text`, if it is plain:
/// the exact number it writes, approximated in the type as the cast of
/// that number and its assignment approximate it (`types::approximate`),
/// if finite; its bits, little-endian.
fn approximate(text: &str, precision: u32, out: &mut Vec<u8>) -> bool {
    let Some(n) = numeral(text) else { return false };
    let Some(units) = (if n.negative { n.units.checked_neg() } else { Some(n.units) }) else { return false };
    let cast = at_precision(Decimal::new(units, n.fraction).to_f64(), precision);
    let assigned = at_precision(cast, precision);
    if !assigned.is_finite() {
        return false;
    }
    out.extend_from_slice(&assigned.to_bits().to_le_bytes());
    true
}

/// [ISO/IEC 9075:1992 §6.10 GR5c, GR6c; §9.2 GR3b, GR3e] Writes the
/// record of a column of CHARACTER(`length`), or of CHARACTER
/// VARYING(`length`) if `varying`, of the string `text`, if it is no
/// longer, so nothing is cut and no warning raised: as it is, of a
/// varying type; else without trailing spaces, as the type's record keeps
/// the string padded to the length (`types::character`).
fn characters(text: &str, (length, units): (u32, CharUnits), varying: bool, out: &mut Vec<u8>) -> bool {
    if usize::try_from(length).is_ok_and(|most| units.count(text) <= most) {
        out.extend_from_slice(if varying { text } else { text.trim_end_matches(SPACE) }.as_bytes());
        return true;
    }
    false
}

/// [ISO/IEC 9075:1992 §6.10 GR9a, GR10a, GR11a; §9.2 GR3l] Writes the
/// record of a column of the datetime type `(moment, precision, zoned)`
/// of the string `text`, if it writes a datetime of it in its year range,
/// as the cast reads it (`types::datetime`), in the displacement a cast
/// of a dynamic parameter takes, `Now`'s by default, and assignment then
/// keeps it: its microseconds in UTC, and of a zoned type its
/// displacement, little-endian.
fn datetime(text: &str, (moment, precision, zoned): (Moment, u32, bool), out: &mut Vec<u8>) -> bool {
    let Some(w) = written(moment, text.trim_matches(SPACE)) else { return false };
    let now = Now::default();
    let local = Datetime::of_local(moment, w.local, w.zone.unwrap_or(now.zone), zoned, precision);
    let cast = Datetime { zone: if zoned { local.zone } else { now.zone }, ..local }.truncated(precision);
    let assigned = Datetime { zoned, ..cast }.truncated(precision);
    if !cast.in_range() || !assigned.in_range() {
        return false;
    }
    out.extend_from_slice(&assigned.utc.to_le_bytes());
    if zoned {
        out.extend_from_slice(&assigned.zone.to_le_bytes());
    }
    true
}

/// [ISO/IEC 9075-2:2023 §6.13 GR21a] Writes the record of a BOOLEAN
/// column of the string `text`, if it is, its spaces trimmed, TRUE or
/// FALSE in any case: UNKNOWN, the null value, and every other string,
/// the row's cast reads.
fn boolean(text: &str, out: &mut Vec<u8>) -> bool {
    let word = text.trim_matches(SPACE);
    let byte = if word.eq_ignore_ascii_case(TRUE_WORD) {
        TRUE_BYTE
    } else if word.eq_ignore_ascii_case(FALSE_WORD) {
        FALSE_BYTE
    } else {
        return false;
    };
    out.push(byte);
    true
}

// Unit tests: a lane's records against the rows' (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
