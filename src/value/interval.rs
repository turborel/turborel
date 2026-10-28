//! [ISO/IEC 9075:1992 §4.5.2, §10.1] An interval's value: of a year-month
//! interval its months, of a day-time interval its microseconds, signed,
//! in 128 bits, with the `<interval qualifier>` of its type, which says
//! which fields it shows. Two intervals of one class compare by these
//! units [§8.2 GR6], whatever their qualifiers.

use std::fmt;

use super::datetime::{Field, MAX_TIME_PRECISION, MICROS_PER_MINUTE, MICROS_PER_SECOND};

/// [ISO/IEC 9075:1992 §10.1 SR3] The greatest `<interval leading field
/// precision>`, implementation-defined, at least 2.
pub const MAX_LEADING_PRECISION: u32 = 9;
/// [ISO/IEC 9075:1992 §10.1 SR5] The `<interval leading field precision>`
/// implied: 2.
pub const DEFAULT_LEADING_PRECISION: u32 = 2;
/// [ISO/IEC 9075:1992 §10.1 SR6] The `<interval fractional seconds
/// precision>` implied: 6.
pub const DEFAULT_FRACTION_PRECISION: u32 = 6;

/// An `<interval qualifier>` [ISO/IEC 9075:1992 §10.1]: its fields, from
/// `start` to `end`, the leading field's digits, and the second's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Qualifier {
    /// The most significant field.
    pub start: Field,
    /// The least significant field; `start` if it is a single field.
    pub end: Field,
    /// The `<interval leading field precision>`.
    pub leading: u8,
    /// The `<interval fractional seconds precision>`, of SECOND; else 0.
    pub fraction: u8,
}

/// An interval's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Interval {
    /// Months, of a year-month interval; microseconds, of a day-time one.
    pub units: i128,
    /// The qualifier of its type.
    pub qualifier: Qualifier,
}

/// The order of significance of `field`, YEAR the most [§10.1 SR1].
pub fn rank(field: Field) -> u8 {
    match field {
        Field::Year => 0,
        Field::Month => 1,
        Field::Day => 2,
        Field::Hour => 3,
        Field::Minute => 4,
        Field::Second => 5,
    }
}

/// The field of `rank`.
fn of_rank(rank: u8) -> Field {
    match rank {
        0 => Field::Year,
        1 => Field::Month,
        2 => Field::Day,
        3 => Field::Hour,
        4 => Field::Minute,
        _ => Field::Second,
    }
}

/// The units, months or microseconds, of one of `field`.
pub fn unit(field: Field) -> i128 {
    match field {
        Field::Year => 12,
        Field::Month => 1,
        Field::Day => i128::from(MICROS_PER_MINUTE).saturating_mul(1440),
        Field::Hour => i128::from(MICROS_PER_MINUTE).saturating_mul(60),
        Field::Minute => i128::from(MICROS_PER_MINUTE),
        Field::Second => i128::from(MICROS_PER_SECOND),
    }
}

/// 10 to the power `n`, within 128 bits.
fn pow10(n: u32) -> i128 {
    10_i128.checked_pow(n).unwrap_or(i128::MAX)
}

/// What a field holds below the next more significant, of a field not
/// the leading one [§4.5.2 Table 7]: 12 months, 24 hours, 60 minutes or
/// seconds.
fn within(field: Field) -> i128 {
    match field {
        Field::Month => 12,
        Field::Hour => 24,
        Field::Year | Field::Day | Field::Minute | Field::Second => 60,
    }
}

impl Qualifier {
    /// A qualifier from `start` to `end`, of `leading` and `fraction`
    /// digits.
    pub fn new(start: Field, end: Field, leading: u32, fraction: u32) -> Qualifier {
        let fraction = if end == Field::Second { fraction.min(MAX_TIME_PRECISION) } else { 0 };
        Qualifier {
            start,
            end,
            leading: u8::try_from(leading.min(MAX_LEADING_PRECISION)).unwrap_or(2),
            fraction: u8::try_from(fraction).unwrap_or(0),
        }
    }

    /// [§4.5.2] Whether it is of a year-month interval.
    pub fn year_month(&self) -> bool {
        self.start == Field::Year || self.start == Field::Month
    }

    /// Its fields, most significant first.
    pub fn fields(&self) -> impl Iterator<Item = Field> {
        (rank(self.start)..=rank(self.end)).map(of_rank)
    }

    /// Whether it has `field`.
    pub fn has(&self, field: Field) -> bool {
        (rank(self.start)..=rank(self.end)).contains(&rank(field))
    }

    /// The units of its least significant field, as it keeps them: of
    /// SECOND, its fraction's.
    pub fn granule(&self) -> i128 {
        if self.end == Field::Second { pow10(MAX_TIME_PRECISION.saturating_sub(u32::from(self.fraction))) } else { unit(self.end) }
    }

    /// The least magnitude, in units, its leading field cannot hold.
    pub fn limit(&self) -> i128 {
        pow10(u32::from(self.leading)).saturating_mul(unit(self.start))
    }

    /// [§6.15 GR4f, GR5e] Its fields with those of `other`, of the same
    /// class, of the greater precisions, and of the leading precision
    /// `leading`.
    #[must_use]
    pub fn joined(&self, other: &Qualifier, leading: u32) -> Qualifier {
        let start = of_rank(rank(self.start).min(rank(other.start)));
        let end = of_rank(rank(self.end).max(rank(other.end)));
        let fraction = if end == Field::Second {
            [self, other].iter().filter(|q| q.end == Field::Second).map(|q| q.fraction).max().unwrap_or(0)
        } else {
            0
        };
        Qualifier::new(start, end, leading, u32::from(fraction))
    }

    /// [§10.1 GR5] Its length in positions: the leading field's digits,
    /// three for each other field, and the fraction's, with its period.
    pub fn positions(&self) -> u32 {
        let others = u32::from(rank(self.end).saturating_sub(rank(self.start)));
        let fraction = u32::from(self.fraction);
        u32::from(self.leading).saturating_add(others.saturating_mul(3)).saturating_add(fraction).saturating_add(u32::from(fraction > 0))
    }
}

/// The key word of `field`.
pub fn field_name(field: Field) -> &'static str {
    match field {
        Field::Year => "YEAR",
        Field::Month => "MONTH",
        Field::Day => "DAY",
        Field::Hour => "HOUR",
        Field::Minute => "MINUTE",
        Field::Second => "SECOND",
    }
}

/// [ISO/IEC 9075:1992 §10.1] As written: `start(leading)`, then TO and
/// `end`, SECOND with its fraction's digits.
impl fmt::Display for Qualifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let start = field_name(self.start);
        if self.start == self.end {
            if self.end == Field::Second {
                return write!(f, "SECOND({}, {})", self.leading, self.fraction);
            }
            return write!(f, "{start}({})", self.leading);
        }
        write!(f, "{start}({}) TO {}", self.leading, field_name(self.end))?;
        if self.end == Field::Second {
            write!(f, "({})", self.fraction)?;
        }
        Ok(())
    }
}

/// The separator written before `field`, not the leading one, in an
/// interval's string [§5.3]: `-` before MONTH, a space before HOUR, `:`
/// before MINUTE and SECOND.
fn separator(field: Field) -> u8 {
    match field {
        Field::Month => b'-',
        Field::Hour => b' ',
        Field::Year | Field::Day | Field::Minute | Field::Second => b':',
    }
}

/// [ISO/IEC 9075:1992 §5.3] The value `text`, an `<interval string>`'s
/// body, writes in the fields of `q`, a sign before it taken: its units;
/// none if it writes no value of `q` [SR20, SR23], a field not the leading
/// one out of its range [§4.5.2 Table 7], or the leading one of more
/// digits than `q` gives [§10.1 SR5]. Digits of the second past `q`'s are
/// dropped.
pub fn parse(q: &Qualifier, text: &str) -> Option<i128> {
    let (negative, body) = match text.as_bytes().first() {
        Some(b'-') => (true, text.get(1..)?),
        Some(b'+') => (false, text.get(1..)?),
        Some(_) | None => (false, text),
    };
    let mut rest = body.as_bytes();
    let mut units = 0_i128;
    for (i, field) in q.fields().enumerate() {
        if i > 0 {
            let (&sep, after) = rest.split_first()?;
            if sep != separator(field) {
                return None;
            }
            rest = after;
        }
        let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        let (whole, after) = rest.split_at(digits);
        let leading = i == 0;
        if whole.is_empty() || (leading && whole.len() > usize::from(q.leading)) {
            return None;
        }
        let n = whole.iter().fold(0_i128, |n, &b| n.saturating_mul(10).saturating_add(i128::from(b.saturating_sub(b'0'))));
        if !leading && n >= within(field) {
            return None;
        }
        units = units.checked_add(n.checked_mul(unit(field))?)?;
        rest = after;
    }
    if q.end == Field::Second
        && let Some((b'.', after)) = rest.split_first()
    {
        let digits = after.iter().take_while(|b| b.is_ascii_digit()).count();
        let micros = after
            .iter()
            .take(digits)
            .chain(std::iter::repeat(&b'0'))
            .take(6)
            .fold(0_i128, |n, &b| n.saturating_mul(10).saturating_add(i128::from(b.saturating_sub(b'0'))));
        units = units.checked_add(micros)?;
        rest = after.get(digits..)?;
    }
    if !rest.is_empty() {
        return None;
    }
    let units = units.saturating_sub(units.checked_rem(q.granule()).unwrap_or(0));
    Some(if negative { units.checked_neg()? } else { units })
}

impl Interval {
    /// [§9.2 GR3m, §6.10 GR12d] `self` in the fields of `q`: truncated to
    /// its least significant field; none if its leading field cannot hold
    /// it (interval field overflow).
    pub fn to(&self, q: Qualifier) -> Option<Interval> {
        let units = self.units.saturating_sub(self.units.checked_rem(q.granule()).unwrap_or(0));
        (units.unsigned_abs() < q.limit().unsigned_abs()).then_some(Interval { units, qualifier: q })
    }

    /// The value of `field` in it, as shown, of its sign [§6.6 GR3a i]:
    /// the leading field all it holds of it; SECOND in microseconds; none
    /// if it has no such field.
    pub fn field(&self, field: Field) -> Option<i128> {
        if !self.qualifier.has(field) {
            return None;
        }
        let size = self.units.unsigned_abs();
        let above = if field == self.qualifier.start {
            size
        } else {
            size.checked_rem(unit(of_rank(rank(field).saturating_sub(1))).unsigned_abs()).unwrap_or(0)
        };
        let n = if field == Field::Second { above } else { above.checked_div(unit(field).unsigned_abs()).unwrap_or(0) };
        let n = i128::try_from(n).unwrap_or(i128::MAX);
        Some(if self.units < 0 { n.saturating_neg() } else { n })
    }
}

/// [ISO/IEC 9075:1992 §6.10 GR5e] Its literal's string, as shown: its
/// sign, if negative; its leading field's digits; each other field two,
/// after its separator; the second's fraction to its precision.
impl fmt::Display for Interval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.units < 0 {
            f.write_str("-")?;
        }
        for (i, field) in self.qualifier.fields().enumerate() {
            let n = self.field(field).unwrap_or(0).unsigned_abs();
            let whole = if field == Field::Second { n.checked_div(MICROS_PER_SECOND.unsigned_abs().into()).unwrap_or(0) } else { n };
            if i == 0 {
                write!(f, "{whole}")?;
            } else {
                write!(f, "{}{whole:02}", char::from(separator(field)))?;
            }
            if field == Field::Second && self.qualifier.fraction > 0 {
                let fraction = format!("{:06}", n.checked_rem(MICROS_PER_SECOND.unsigned_abs().into()).unwrap_or(0));
                write!(f, ".{}", fraction.get(..usize::from(self.qualifier.fraction)).unwrap_or_default())?;
            }
        }
        Ok(())
    }
}

mod arith;
pub use arith::{add, difference, minutes, scale, shift};

#[cfg(test)]
mod tests;
