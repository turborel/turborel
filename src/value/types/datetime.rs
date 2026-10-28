//! [ISO/IEC 9075:1992 §6.1] DATE, TIME(p) and TIMESTAMP(p), each of the
//! last two WITH TIME ZONE or not: the datetime types, whose values are
//! `Value::Datetime`, compared by their instants in UTC [§8.2 GR5].
//!
//! A value's record bytes are its microseconds in UTC, a 64-bit word,
//! little-endian; of a type WITH TIME ZONE, then its displacement in
//! minutes, 16 bits. Its key is the microseconds big-endian, the sign bit
//! flipped, so keys order as instants do; values of one instant in two
//! displacements share it, as they compare equal.
//!
//! CAST to or from one needs what the SQL-session gives: its time zone
//! displacement, implied by a value of a type without time zone [§6.1
//! SR5], and the current date [§6.10 GR11c]. The binder gives them
//! (`cast_at`); `SqlType::cast`, given none, takes UTC and 0001-01-01.

use std::cmp::Ordering;

use super::{Descriptor, NUMBER_BYTES, SIGN, SqlType, take};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::value::cast::uncastable;
use crate::value::datetime::{Datetime, MICROS_PER_DAY, MICROS_PER_MINUTE, Moment, Now, parse};
use crate::value::{Bucket, DataType, Value, unassignable};

/// The kind of one datetime type.
#[derive(Debug)]
pub struct Chronon {
    /// Which fields its values have.
    moment: Moment,
    /// Whether it is WITH TIME ZONE.
    zoned: bool,
}

/// DATE.
pub static DATE: Chronon = Chronon { moment: Moment::Date, zoned: false };
/// TIME(p).
pub static TIME: Chronon = Chronon { moment: Moment::Time, zoned: false };
/// TIME(p) WITH TIME ZONE.
pub static TIME_ZONED: Chronon = Chronon { moment: Moment::Time, zoned: true };
/// TIMESTAMP(p).
pub static TIMESTAMP: Chronon = Chronon { moment: Moment::Timestamp, zoned: false };
/// TIMESTAMP(p) WITH TIME ZONE.
pub static TIMESTAMP_ZONED: Chronon = Chronon { moment: Moment::Timestamp, zoned: true };

/// The kind of `d`'s type.
pub fn kind_of(d: &Datetime) -> &'static Chronon {
    match (d.moment, d.zoned) {
        (Moment::Date, _) => &DATE,
        (Moment::Time, false) => &TIME,
        (Moment::Time, true) => &TIME_ZONED,
        (Moment::Timestamp, false) => &TIMESTAMP,
        (Moment::Timestamp, true) => &TIMESTAMP_ZONED,
    }
}

/// The fields, precision and zone of `t`, a datetime type.
fn shape(t: &DataType) -> Option<(Moment, u32, bool)> {
    match t {
        DataType::Date => Some((Moment::Date, 0, false)),
        DataType::Time { precision, zoned } => Some((Moment::Time, *precision, *zoned)),
        DataType::Timestamp { precision, zoned } => Some((Moment::Timestamp, *precision, *zoned)),
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::Bit { .. }
        | DataType::BitVarying { .. }
        | DataType::Interval(_)
        | DataType::Extension(_) => None,
    }
}

/// The datetime `v` is, if it is one.
fn datetime(v: &Value) -> Option<&Datetime> {
    if let Value::Datetime(d) = v { Some(d) } else { None }
}

/// A value not of a column of `t`'s type.
fn mismatch(v: &Value, t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}"))
}

/// [ISO/IEC 9075:1992 §6.1 SR6] `d`, if its year is from 0001 to 9999.
///
/// # Errors
///
/// SQLSTATE 22007 if it is not.
fn in_range(d: Datetime) -> Result<Value> {
    if d.in_range() {
        return Ok(Value::Datetime(d));
    }
    fail_with(Code::Data, SqlState::INVALID_DATETIME_FORMAT, format!("[ISO/IEC 9075:1992 §6.1 SR6] {d} is not of a year from 0001 to 9999"))
}

/// `d` shown as the SQL-session shows it: of a type without time zone, in
/// the session's displacement, `now`'s [§6.1 SR5].
pub fn shown(d: &Datetime, now: Now) -> Datetime {
    if d.zoned || d.moment == Moment::Date { *d } else { Datetime { zone: now.zone, ..*d } }
}

/// [ISO/IEC 9075:1992 §6.10 GR9-GR11] `d` CAST to `(moment, precision,
/// zoned)`, in the SQL-session `now` gives.
fn convert(d: &Datetime, (moment, precision, zoned): (Moment, u32, bool), now: Now) -> Result<Value> {
    let d = shown(d, now);
    // [GR10b, c; GR11c, d] its own displacement, of a type WITH TIME
    // ZONE; else the session's.
    let zone = if zoned { d.zone } else { now.zone };
    let at = |local: i64| Datetime::of_local(moment, local, zone, zoned, precision);
    let today = Datetime { moment: Moment::Timestamp, utc: now.utc, zone: now.zone, zoned: true, precision: 0 }.days();
    // Its instant, shown in the displacement it takes.
    let same = || at(d.utc.saturating_add(i64::from(zone).saturating_mul(MICROS_PER_MINUTE)));
    let converted = match (d.moment, moment) {
        (Moment::Date, Moment::Date) | (Moment::Timestamp, Moment::Date) => {
            Datetime::of_local(Moment::Date, d.days().saturating_mul(MICROS_PER_DAY), 0, false, 0)
        }
        // [GR11b] at its midnight, in the session's displacement.
        (Moment::Date, Moment::Timestamp) => {
            Datetime::of_local(moment, d.days().saturating_mul(MICROS_PER_DAY), now.zone, zoned, precision)
        }
        (Moment::Time | Moment::Timestamp, Moment::Time) | (Moment::Timestamp, Moment::Timestamp) => same(),
        // [GR11c] on CURRENT_DATE.
        (Moment::Time, Moment::Timestamp) => at(today.saturating_mul(MICROS_PER_DAY).saturating_add(d.time_of_day())),
        (Moment::Date, Moment::Time) | (Moment::Time, Moment::Date) => {
            return uncastable(&Value::Datetime(d), &type_of(moment, precision, zoned));
        }
    };
    in_range(converted.truncated(precision))
}

/// The datetime type of `moment`, `precision` and `zoned`.
fn type_of(moment: Moment, precision: u32, zoned: bool) -> DataType {
    match moment {
        Moment::Date => DataType::Date,
        Moment::Time => DataType::Time { precision, zoned },
        Moment::Timestamp => DataType::Timestamp { precision, zoned },
    }
}

/// [ISO/IEC 9075:1992 §6.10 GR9a, GR10a, GR11a] The datetime the
/// character string `s` writes, its spaces trimmed, as a value of `(moment,
/// precision, zoned)`: of the displacement it writes, else the session's.
fn of_text(s: &str, (moment, precision, zoned): (Moment, u32, bool), now: Now) -> Result<Value> {
    let Some(w) = parse(moment, s.trim_matches(' ')) else {
        let msg = format!(
            "[ISO/IEC 9075:1992 §6.10 GR9a ii, GR10a ii, GR11a ii] '{s}' is no {} literal's string",
            type_of(moment, precision, zoned)
        );
        return fail_with(Code::Data, SqlState::INVALID_CHARACTER_VALUE_FOR_CAST, msg);
    };
    let d = Datetime::of_local(moment, w.local, w.zone.unwrap_or(now.zone), zoned, precision);
    in_range(Datetime { zone: if zoned { d.zone } else { now.zone }, ..d }.truncated(precision))
}

/// [ISO/IEC 9075:1992 §6.10] `v` CAST to `t`, a datetime type or a
/// character string type, in the SQL-session `now` gives.
///
/// # Errors
///
/// SQLSTATE 22018 if a string writes no datetime of the type, 22007 if
/// the year leaves 0001 to 9999, 22001 if a datetime's literal does not fit
/// a string type [GR5e iv, GR6e iii]; 22005 if `v` is no value CAST takes
/// to `t` [SR4].
pub fn cast_at(v: Value, t: &DataType, now: Now) -> Result<Value> {
    if let (Value::Datetime(d), Some(length), Some(fixed)) = (&v, string_length(t), Some(matches!(t, DataType::Character { .. }))) {
        return as_string(&shown(d, now), length, fixed);
    }
    let Some(shape) = shape(t) else { return t.kind().cast(v, t) };
    match &v {
        Value::Null => Ok(Value::Null),
        Value::Char(s) => of_text(s, shape, now),
        Value::Datetime(d) => convert(d, shape, now),
        Value::Exact(_) | Value::Approx(_) | Value::Bits(_) | Value::Interval(_) | Value::Extension(_) => uncastable(&v, t),
    }
}

/// The length of `t`, if a character string type.
fn string_length(t: &DataType) -> Option<u32> {
    if let DataType::Character { length } | DataType::Varying { length } = t { Some(*length) } else { None }
}

/// [ISO/IEC 9075:1992 §6.10 GR5e, GR6e] `d`'s literal's string as a
/// character string of `length` characters, fixed and padded with spaces
/// or at most so many.
///
/// # Errors
///
/// SQLSTATE 22001 if it is longer [GR5e iv, GR6e iii].
pub fn as_string(d: &Datetime, length: u32, fixed: bool) -> Result<Value> {
    fitted(d.to_string(), length, fixed)
}

/// [ISO/IEC 9075:1992 §6.10 GR5e, GR6e] `text`, a datetime's or an
/// interval's literal's string, as a character string of `length`
/// characters, fixed and padded with spaces or at most so many.
///
/// # Errors
///
/// SQLSTATE 22001 if it is longer [GR5e iv, GR6e iii].
pub fn fitted(text: String, length: u32, fixed: bool) -> Result<Value> {
    let (count, fits) = (text.chars().count(), usize::try_from(length).unwrap_or(usize::MAX));
    if count > fits {
        let msg = format!("[ISO/IEC 9075:1992 §6.10 GR5e iv, GR6e iii] '{text}' is longer than {length} characters");
        return fail_with(Code::Data, SqlState::STRING_DATA_RIGHT_TRUNCATION, msg);
    }
    Ok(Value::Char(if fixed { format!("{text}{}", " ".repeat(fits.saturating_sub(count))) } else { text }))
}

/// `utc` as a 64-bit word's bits.
fn word(utc: i64) -> u64 {
    u64::from_be_bytes(utc.to_be_bytes())
}

/// The nearest `f64` to `n`, as an `i64` holds it: its high and low
/// halves.
fn approximately(n: i64) -> f64 {
    let (high, low) =
        (i32::try_from(n.checked_shr(32).unwrap_or(0_i64)).unwrap_or(0_i32), u32::try_from(n & 0xFFFF_FFFF_i64).unwrap_or(0_u32));
    f64::from(high).mul_add(4_294_967_296.0_f64, f64::from(low))
}

impl SqlType for Chronon {
    fn name(&self) -> &'static str {
        match (self.moment, self.zoned) {
            (Moment::Date, _) => "DATE",
            (Moment::Time, false) => "TIME",
            (Moment::Time, true) => "TIME WITH TIME ZONE",
            (Moment::Timestamp, false) => "TIMESTAMP",
            (Moment::Timestamp, true) => "TIMESTAMP WITH TIME ZONE",
        }
    }

    fn written(&self, t: &DataType) -> String {
        let precision = shape(t).map_or(0, |s| s.1);
        match (self.moment, self.zoned) {
            (Moment::Date, _) => "DATE".to_owned(),
            (Moment::Time, false) => format!("TIME({precision})"),
            (Moment::Time, true) => format!("TIME({precision}) WITH TIME ZONE"),
            (Moment::Timestamp, false) => format!("TIMESTAMP({precision})"),
            (Moment::Timestamp, true) => format!("TIMESTAMP({precision}) WITH TIME ZONE"),
        }
    }

    /// [ISO/IEC 9075:1992 §4.5.1] Datetimes of the same fields, with time
    /// zone or not.
    fn comparable(&self, other: &DataType) -> bool {
        shape(other).is_some_and(|(moment, _, _)| moment == self.moment)
    }

    /// [ISO/IEC 9075:1992 §8.2 GR5] Their instants.
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        datetime(a)?.compare(datetime(b)?)
    }

    /// Its instant, which equal datetimes share.
    fn bucket(&self, v: &Value) -> Option<Bucket> {
        datetime(v).map(|d| Bucket::Instant(d.utc))
    }

    /// [ISO/IEC 9075:1992 §9.2 SR2, GR3l] A datetime of the same fields,
    /// as the type represents it: its fraction cut to the precision; of a
    /// type WITH TIME ZONE, of its displacement, implied or its own.
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        let (Some(d), Some((moment, precision, zoned))) = (datetime(&v), shape(t)) else { return unassignable(&v, t, rule) };
        if d.moment != moment {
            return unassignable(&v, t, rule);
        }
        in_range(Datetime { zoned, ..*d }.truncated(precision))
    }

    /// As `cast_at`, in UTC on 0001-01-01: the binder gives the session's.
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        cast_at(v, t, Now::default())
    }

    /// [ISO/IEC 9075:1992 §5.3] `<datetime literal>`: DATE, TIME or
    /// TIMESTAMP, then its string.
    fn literal(&self, v: &Value) -> String {
        datetime(v).map_or_else(|| v.to_string(), |d| format!("{} '{d}'", d.key_word()))
    }

    fn text(&self, v: &Value) -> String {
        v.to_string()
    }

    /// Its instant, in microseconds.
    fn position(&self, v: &Value) -> Option<f64> {
        datetime(v).map(|d| approximately(d.utc))
    }

    /// [ISO/IEC 9075:1992 §21.3.5] Its fraction's digits; 0 of a DATE.
    fn descriptor(&self, t: &DataType) -> Descriptor {
        Descriptor { datetime_precision: Some(shape(t).map_or(0, |s| s.1)), ..Descriptor::of(self.name()) }
    }

    /// Its fields' kind, its precision and its displacement, then its
    /// instant.
    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        let d = datetime(v).copied().unwrap_or_else(|| Datetime::of_local(self.moment, 0, 0, self.zoned, 0));
        out.put_u8(u8::from(d.zoned));
        out.put_u8(d.precision);
        out.put_u32(u32::from(d.zone.cast_unsigned()));
        out.put_u64(word(d.utc));
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let (zoned, precision) = (r.u8()? == 1, r.u8()?);
        let zone = u16::try_from(r.u32()?).map_err(|_| r.corrupt("a time zone displacement is out of range"))?.cast_signed();
        let utc = i64::from_be_bytes(r.u64()?.to_be_bytes());
        Ok(Value::Datetime(Datetime { moment: self.moment, utc, zone, zoned, precision }))
    }

    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        out.put_u32(shape(t).map_or(0, |s| s.1));
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        let precision = r.u32()?;
        Ok(type_of(self.moment, precision, self.zoned))
    }

    /// Its microseconds, and of a type WITH TIME ZONE its displacement.
    fn record_width(&self, _: &DataType) -> Option<usize> {
        Some(if self.zoned { NUMBER_BYTES.saturating_add(2) } else { NUMBER_BYTES })
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        let d = datetime(v).filter(|d| d.moment == self.moment).ok_or_else(|| mismatch(v, t))?;
        let mut out = d.utc.to_le_bytes().to_vec();
        if self.zoned {
            out.extend_from_slice(&d.zone.to_le_bytes());
        }
        Ok(out)
    }

    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        let (micros, zone) = field.split_at_checked(NUMBER_BYTES)?;
        let utc = i64::from_le_bytes(micros.try_into().ok()?);
        let zone = if self.zoned { i16::from_le_bytes(zone.try_into().ok()?) } else { 0 };
        let precision = u8::try_from(shape(t)?.1).ok()?;
        Some(Value::Datetime(Datetime { moment: self.moment, utc, zone, zoned: self.zoned, precision }))
    }

    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let d = datetime(v).filter(|d| d.moment == self.moment).ok_or_else(|| mismatch(v, t))?;
        out.extend_from_slice(&(word(d.utc) ^ SIGN).to_be_bytes());
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], t: &DataType) -> Result<Value> {
        let bits = <[u8; NUMBER_BYTES]>::try_from(take(rest, NUMBER_BYTES)?).map_err(|_| super::malformed())?;
        let utc = i64::from_be_bytes((u64::from_be_bytes(bits) ^ SIGN).to_be_bytes());
        let precision = u8::try_from(shape(t).map_or(0, |s| s.1)).unwrap_or(0);
        Ok(Value::Datetime(Datetime { moment: self.moment, utc, zone: 0, zoned: self.zoned, precision }))
    }

    /// Itself: every instant has a key.
    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        datetime(v).filter(|d| d.moment == self.moment).map(|_| (v.clone(), true))
    }
}

/// [ISO/IEC 9075:1992 §6.8 GR1] `now`'s instant as a value of `moment`, of
/// `precision`: CURRENT_DATE, CURRENT_TIME or CURRENT_TIMESTAMP, of the
/// session's displacement.
pub fn current(moment: Moment, precision: u32, now: Now) -> Value {
    let instant = Datetime { moment: Moment::Timestamp, utc: now.utc, zone: now.zone, zoned: true, precision: 6 };
    let local = instant.local();
    let d = match moment {
        Moment::Date => Datetime::of_local(Moment::Date, instant.days().saturating_mul(MICROS_PER_DAY), 0, false, 0),
        Moment::Time => Datetime::of_local(Moment::Time, local, now.zone, true, precision),
        Moment::Timestamp => Datetime::of_local(Moment::Timestamp, local, now.zone, true, precision),
    };
    Value::Datetime(d.truncated(precision))
}

// Unit tests: CAST between the datetime types in a session, and the
// kinds' bytes (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
