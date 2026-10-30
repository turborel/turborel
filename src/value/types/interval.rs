//! [ISO/IEC 9075:1992 §6.1] INTERVAL `<interval qualifier>`: a year-month
//! or a day-time interval, its values `Value::Interval`, compared by their
//! months or microseconds [§8.2 GR6].
//!
//! A value's record bytes are its units, 128 bits, little-endian; its key
//! the units big-endian, the sign bit flipped, so keys order as intervals
//! do. Its qualifier is the column's, so neither holds it.

use std::cmp::Ordering;

use super::{Descriptor, SqlType, take};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::value::cast::uncastable;
use crate::value::datetime::{Field, MAX_TIME_PRECISION};
use crate::value::interval::{Interval, Qualifier, parse, rank, unit};
use crate::value::{Bucket, DataType, Decimal, Value, pow10, unassignable};

/// The kind of INTERVAL `<interval qualifier>`, of one class.
#[derive(Debug)]
pub struct IntervalKind {
    /// Whether of year-month intervals; else of day-time ones.
    year_month: bool,
}

/// INTERVAL of YEAR and MONTH.
pub static INTERVAL_YEAR_MONTH: IntervalKind = IntervalKind { year_month: true };
/// INTERVAL of DAY to SECOND.
pub static INTERVAL_DAY_TIME: IntervalKind = IntervalKind { year_month: false };

/// The kind of an interval of qualifier `q`.
pub fn kind_of(q: Qualifier) -> &'static IntervalKind {
    if q.year_month() { &INTERVAL_YEAR_MONTH } else { &INTERVAL_DAY_TIME }
}

/// The sign bit of a 128-bit word.
const SIGN: u128 = 1 << 127;

/// The bytes of an interval's units.
const UNIT_BYTES: usize = 16;

/// The qualifier of `t`, an interval type.
fn qualifier_of(t: &DataType) -> Option<Qualifier> {
    if let DataType::Interval(q) = t { Some(*q) } else { None }
}

/// The interval `v` is, if it is one.
fn interval(v: &Value) -> Option<&Interval> {
    if let Value::Interval(i) = v { Some(i) } else { None }
}

/// A value not of a column of `t`'s type.
fn mismatch(v: &Value, t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}"))
}

/// The field of `rank`, of a qualifier read.
fn field_of(rank: u32) -> Option<Field> {
    [Field::Year, Field::Month, Field::Day, Field::Hour, Field::Minute, Field::Second].get(usize::try_from(rank).ok()?).copied()
}

/// [ISO/IEC 9075:1992 §9.2 GR3m, §6.10 GR12] `i` in the fields of `q`.
///
/// # Errors
///
/// SQLSTATE 22015 if `q`'s leading field cannot hold it.
pub fn fitted(i: &Interval, q: Qualifier) -> Result<Value> {
    i.to(q).map(Value::Interval).map_or_else(
        || {
            fail_with(
                Code::Data,
                SqlState::INTERVAL_FIELD_OVERFLOW,
                format!("[ISO/IEC 9075:1992 §9.2 GR3m, §6.10 GR12] {i} does not fit INTERVAL {q}"),
            )
        },
        Ok,
    )
}

/// [ISO/IEC 9075:1992 §6.10 GR12a] The exact number `d` as an interval of
/// the one field of `q`, its fraction kept to the microsecond of SECOND.
///
/// # Errors
///
/// SQLSTATE 22015 if `q`'s leading field cannot hold it.
fn of_number(d: &Decimal, q: Qualifier) -> Result<Value> {
    let micros = if q.end == Field::Second { MAX_TIME_PRECISION } else { 0 };
    let scaled = if d.scale <= micros {
        pow10(micros.saturating_sub(d.scale)).and_then(|f| d.units.checked_mul(f))
    } else {
        pow10(d.scale.saturating_sub(micros)).and_then(|f| d.units.checked_div(f))
    };
    let per = if q.end == Field::Second { 1 } else { unit(q.end) };
    let units = scaled.and_then(|n| n.checked_mul(per));
    let overflow = || {
        fail_with(Code::Data, SqlState::INTERVAL_FIELD_OVERFLOW, format!("[ISO/IEC 9075:1992 §6.10 GR12a i] {d} does not fit INTERVAL {q}"))
    };
    let Some(units) = units else { return overflow() };
    fitted(&Interval { units, qualifier: q }, q).or_else(|_| overflow())
}

/// [ISO/IEC 9075:1992 §6.10 GR3c] The interval `i`, of a single field, as
/// an exact number of it: of SECOND, of six digits of fraction.
pub fn as_number(i: &Interval) -> Decimal {
    if i.qualifier.end == Field::Second {
        return Decimal::new(i.units, MAX_TIME_PRECISION);
    }
    Decimal::new(i.units.checked_div(unit(i.qualifier.end)).unwrap_or(0), 0)
}

impl SqlType for IntervalKind {
    fn name(&self) -> &'static str {
        "INTERVAL"
    }

    fn written(&self, t: &DataType) -> String {
        qualifier_of(t).map_or_else(|| "INTERVAL".to_owned(), |q| format!("INTERVAL {q}"))
    }

    /// [ISO/IEC 9075:1992 §4.5.2] Intervals of one class: year-month, or
    /// day-time.
    fn comparable(&self, other: &DataType) -> bool {
        qualifier_of(other).is_some_and(|q| q.year_month() == self.year_month)
    }

    /// [ISO/IEC 9075:1992 §8.2 GR6] Of one class, by their units.
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        let (a, b) = (interval(a)?, interval(b)?);
        (a.qualifier.year_month() == b.qualifier.year_month()).then(|| a.units.cmp(&b.units))
    }

    /// Its units, which equal intervals share.
    fn bucket(&self, v: &Value) -> Option<Bucket> {
        interval(v).map(|i| Bucket::Span(i.units))
    }

    /// [ISO/IEC 9075:1992 §9.2 SR2, GR3m] An interval of the same class, in
    /// the type's fields.
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        let (Some(i), Some(q)) = (interval(&v), qualifier_of(t)) else { return unassignable(&v, t, rule) };
        if i.qualifier.year_month() != q.year_month() {
            return unassignable(&v, t, rule);
        }
        fitted(i, q)
    }

    /// [ISO/IEC 9075:1992 §6.10 GR12] A number, as the one field of the
    /// type \[GR12a\]; a string, the interval it writes, its spaces trimmed
    /// \[GR12b\]; an interval of the same class, in the type's fields
    /// \[GR12c, d\].
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        let Some(q) = qualifier_of(t) else { return uncastable(&v, t) };
        match &v {
            Value::Null => Ok(Value::Null),
            Value::Exact(d) => of_number(d, q),
            Value::Char(s) => parse(&q, s.trim_matches(' ')).map_or_else(
                || {
                    let msg = format!("[ISO/IEC 9075:1992 §6.10 GR12b ii] '{s}' is no interval literal's string of INTERVAL {q}");
                    fail_with(Code::Data, SqlState::INVALID_CHARACTER_VALUE_FOR_CAST, msg)
                },
                |units| Ok(Value::Interval(Interval { units, qualifier: q })),
            ),
            Value::Interval(i) => {
                if i.qualifier.year_month() == q.year_month() {
                    fitted(i, q)
                } else {
                    uncastable(&v, t)
                }
            }
            Value::Approx(_) | Value::Bits(_) | Value::Datetime(_) | Value::Extension(_) => uncastable(&v, t),
        }
    }

    /// [ISO/IEC 9075:1992 §5.3] `<interval literal>`: INTERVAL, its sign,
    /// its string and its qualifier.
    fn literal(&self, v: &Value) -> String {
        interval(v).map_or_else(
            || v.to_string(),
            |i| {
                let text = i.to_string();
                let (sign, body) = text.strip_prefix('-').map_or(("", text.as_str()), |b| ("-", b));
                format!("INTERVAL {sign}'{body}' {}", i.qualifier)
            },
        )
    }

    fn text(&self, v: &Value) -> String {
        v.to_string()
    }

    /// Its units as a number of its least significant field (`as_number`),
    /// approximately.
    fn position(&self, v: &Value) -> Option<f64> {
        interval(v).map(|i| as_number(i).to_f64())
    }

    /// [ISO/IEC 9075:1992 §21.3.5] Its fraction's digits: of SECOND, its
    /// `<interval fractional seconds precision>`; else 0.
    fn descriptor(&self, t: &DataType) -> Descriptor {
        Descriptor { datetime_precision: Some(qualifier_of(t).map_or(0, |q| u32::from(q.fraction))), ..Descriptor::of("INTERVAL") }
    }

    /// Its qualifier, then its units.
    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        let i = interval(v).copied().unwrap_or(Interval { units: 0, qualifier: Qualifier::new(Field::Day, Field::Day, 2, 0) });
        self.put_params(&DataType::Interval(i.qualifier), out);
        out.put_i128(i.units);
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let DataType::Interval(qualifier) = self.params(r)? else { return Err(r.corrupt("not an interval's qualifier")) };
        Ok(Value::Interval(Interval { units: r.i128()?, qualifier }))
    }

    /// Its fields' ranks, then its precisions.
    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        let q = qualifier_of(t).unwrap_or(Qualifier::new(Field::Day, Field::Day, 2, 0));
        for n in [rank(q.start), rank(q.end), q.leading, q.fraction] {
            out.put_u32(u32::from(n));
        }
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        let (start, end) = (field_of(r.u32()?), field_of(r.u32()?));
        let (leading, fraction) = (r.u32()?, r.u32()?);
        let (Some(start), Some(end)) = (start, end) else { return Err(r.corrupt("an interval's field is out of range")) };
        Ok(DataType::Interval(Qualifier::new(start, end, leading, fraction)))
    }

    /// Its units.
    fn record_width(&self, _: &DataType) -> Option<usize> {
        Some(UNIT_BYTES)
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        Ok(interval(v).ok_or_else(|| mismatch(v, t))?.units.to_le_bytes().to_vec())
    }

    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        let units = i128::from_le_bytes(field.try_into().ok()?);
        Some(Value::Interval(Interval { units, qualifier: qualifier_of(t)? }))
    }

    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let units = interval(v).ok_or_else(|| mismatch(v, t))?.units;
        out.extend_from_slice(&(u128::from_be_bytes(units.to_be_bytes()) ^ SIGN).to_be_bytes());
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], t: &DataType) -> Result<Value> {
        let bits = <[u8; UNIT_BYTES]>::try_from(take(rest, UNIT_BYTES)?).map_err(|_| super::malformed())?;
        let units = i128::from_be_bytes((u128::from_be_bytes(bits) ^ SIGN).to_be_bytes());
        Ok(Value::Interval(Interval { units, qualifier: qualifier_of(t).ok_or_else(super::malformed)? }))
    }

    /// Itself: every interval has a key.
    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        interval(v).map(|_| (v.clone(), true))
    }
}

// Unit tests: the interval kinds' bytes, literals, casts and refusals
// (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;

    /// DAY TO SECOND(2) and YEAR TO MONTH.
    fn qualifiers() -> (Qualifier, Qualifier) {
        (Qualifier::new(Field::Day, Field::Second, 2, 2), Qualifier::new(Field::Year, Field::Month, 2, 0))
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.3, §6.1, §21.3.5] an
    /// interval kind is written with its qualifier, its value as a literal
    /// of its sign, described with its fraction's digits; its value's
    /// bytes read back; a field out of range, or a record of the wrong
    /// bytes, is no value of it.
    #[test]
    fn interval_kinds_write_and_keep() {
        let (dt, ym) = qualifiers();
        let i = Value::Interval(Interval { units: -1_500_000, qualifier: dt });
        assert_eq!(INTERVAL_DAY_TIME.written(&DataType::Interval(dt)), "INTERVAL DAY(2) TO SECOND(2)");
        assert_eq!(INTERVAL_DAY_TIME.written(&DataType::Date), "INTERVAL");
        assert_eq!(INTERVAL_DAY_TIME.literal(&i), "INTERVAL -'0 00:00:01.50' DAY(2) TO SECOND(2)");
        assert_eq!(INTERVAL_DAY_TIME.literal(&Value::Null), "NULL");
        assert_eq!((INTERVAL_DAY_TIME.text(&i), INTERVAL_DAY_TIME.position(&i)), ("-0 00:00:01.50".to_owned(), Some(-1.5_f64)));
        assert_eq!(INTERVAL_DAY_TIME.descriptor(&DataType::Interval(dt)).datetime_precision, Some(2));
        assert_eq!(INTERVAL_YEAR_MONTH.descriptor(&DataType::Interval(ym)).datetime_precision, Some(0));
        let mut out = Vec::new();
        INTERVAL_DAY_TIME.put_value(&i, &mut out);
        assert_eq!(INTERVAL_DAY_TIME.value(&mut Reader::new(&out, "value")).unwrap().to_string(), "-0 00:00:01.50");
        let mut bad = Vec::new();
        bad.put_u32(7);
        bad.put_u32(7);
        bad.put_u32(2);
        bad.put_u32(0);
        assert_eq!(INTERVAL_DAY_TIME.params(&mut Reader::new(&bad, "type")).unwrap_err().code, Code::Corrupt);
        assert!(INTERVAL_DAY_TIME.decode_record(&[0; 8], &DataType::Interval(dt)).is_none());
        assert!(INTERVAL_DAY_TIME.decode_record(&[0; 16], &DataType::Date).is_none());
        assert_eq!(INTERVAL_DAY_TIME.decode_key(&mut &[0_u8; 16][..], &DataType::Date).unwrap_err().code, Code::Corrupt);
        assert!(INTERVAL_DAY_TIME.anchor(&Value::Null, &DataType::Interval(dt)).is_none());
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.10 SR4-SR6, GR3c, GR12;
    /// §9.2 SR2] CAST to an interval takes an exact number to one field, a
    /// string, an interval of its class; no interval of the other class,
    /// no approximate number or datetime (SQLSTATE 22005); assignment of
    /// one of the other class is refused (22005); an interval of one field
    /// is a number of it; intervals of two classes do not compare.
    #[test]
    fn interval_kinds_cast_and_refuse() {
        let (dt, ym) = qualifiers();
        let (t_dt, t_ym) = (DataType::Interval(dt), DataType::Interval(ym));
        let month = Value::Interval(Interval { units: 13, qualifier: ym });
        for v in [month.clone(), Value::Approx(1.0), Value::Bits(crate::value::Bits::default())] {
            assert_eq!(INTERVAL_DAY_TIME.cast(v, &t_dt).unwrap_err().code, Code::Data);
        }
        assert!(INTERVAL_DAY_TIME.cast(Value::Null, &t_dt).unwrap().is_null());
        assert_eq!(
            INTERVAL_DAY_TIME.cast(Value::Exact(Decimal::new(1, 0)), &DataType::Date).unwrap_err().code,
            Code::Data,
            "no interval type"
        );
        assert_eq!(INTERVAL_DAY_TIME.assign(month.clone(), &t_dt, "test").unwrap_err().code, Code::Data);
        assert_eq!(INTERVAL_YEAR_MONTH.assign(Value::Char("1".into()), &t_ym, "test").unwrap_err().code, Code::Data);
        let years = Interval { units: 36, qualifier: Qualifier::new(Field::Year, Field::Year, 2, 0) };
        assert_eq!(as_number(&years).to_string(), "3");
        assert!(INTERVAL_YEAR_MONTH.compare(&month, &Value::Interval(Interval { units: 13, qualifier: dt })).is_none());
        assert!(!INTERVAL_YEAR_MONTH.comparable(&t_dt) && INTERVAL_YEAR_MONTH.comparable(&t_ym));
        let mut key = Vec::new();
        assert_eq!(INTERVAL_YEAR_MONTH.encode_key(&Value::Null, &t_ym, &mut key).unwrap_err().code, Code::Api);
        assert_eq!(INTERVAL_YEAR_MONTH.encode_record(&Value::Null, &t_ym).unwrap_err().code, Code::Api);
    }
}
