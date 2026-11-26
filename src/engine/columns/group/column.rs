//! \[DESIGN\] The values of a column a group is written of, read where they
//! are (RULES.md D1148): of rows, each row's value, borrowed; of a load's
//! records, each decoded as it is needed, none kept as a column of values.
//! Its zones, Bloom filters, imprints and sketch are made of them alike, so
//! a group written of records is the group written of the same rows.
//!
//! Of a column of exact numbers, datetimes or times, whose record is an
//! integer that orders as its values do (its units at the column's scale,
//! or its instant in microseconds), the least and greatest of a range are
//! found of the integers, and only the two found are decoded.

use std::borrow::Cow;
use std::cmp::Ordering;

use crate::storage::lanes::Records;
use crate::value::{DataType, Value, compare};

/// Bytes of a narrow exact number's units, and of an instant.
const NARROW: usize = 8;
/// Bytes of a wide exact number's units.
const WIDE: usize = 16;

/// The values of one column of a group's rows, in its rows' order.
pub trait Column {
    /// How many there are.
    fn len(&self) -> usize;

    /// Whether there are none.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The value of row `n`, the null value past the last.
    fn value(&self, n: usize) -> Cow<'_, Value>;

    /// The least and greatest of those from `from` to `to` that are not
    /// null, the first found of those equal, and how many are null.
    fn bounds(&self, (from, to): (usize, usize)) -> (Option<(Value, Value)>, u32) {
        bounds_by(self, (from, to))
    }
}

/// As `Column::bounds`, of each value compared as `compare` does: a value
/// cloned only as it is a new least or greatest.
fn bounds_by<C: Column + ?Sized>(column: &C, (from, to): (usize, usize)) -> (Option<(Value, Value)>, u32) {
    let mut bounds: Option<(Value, Value)> = None;
    let mut nulls = 0_u32;
    for n in from..to.min(column.len()) {
        let v = column.value(n);
        if v.is_null() {
            nulls = nulls.saturating_add(1);
            continue;
        }
        match &mut bounds {
            None => bounds = Some((v.as_ref().clone(), v.into_owned())),
            Some((least, greatest)) => match (compare(&v, least) == Some(Ordering::Less), compare(&v, greatest) == Some(Ordering::Greater))
            {
                (true, true) => (*least, *greatest) = (v.as_ref().clone(), v.into_owned()),
                (true, false) => *least = v.into_owned(),
                (false, true) => *greatest = v.into_owned(),
                (false, false) => {}
            },
        }
    }
    (bounds, nulls)
}

impl Column for [Value] {
    fn len(&self) -> usize {
        <[Value]>::len(self)
    }

    fn value(&self, n: usize) -> Cow<'_, Value> {
        Cow::Borrowed(self.get(n).unwrap_or(&Value::Null))
    }
}

impl Column for [&Value] {
    fn len(&self) -> usize {
        <[&Value]>::len(self)
    }

    fn value(&self, n: usize) -> Cow<'_, Value> {
        Cow::Borrowed(self.get(n).copied().unwrap_or(&Value::Null))
    }
}

/// The column at `at` of rows, each borrowed where it is.
pub struct Rowed<'a, R> {
    /// The rows.
    pub rows: &'a [R],
    /// The column's place in each.
    pub at: usize,
}

impl<R: AsRef<[Value]>> Column for Rowed<'_, R> {
    fn len(&self) -> usize {
        self.rows.len()
    }

    fn value(&self, n: usize) -> Cow<'_, Value> {
        Cow::Borrowed(self.rows.get(n).and_then(|r| r.as_ref().get(self.at)).unwrap_or(&Value::Null))
    }
}

/// Of a type whose record is an integer ordered as its values are, how
/// to read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ordinal {
    /// An exact number's units, 8 bytes, at the column's scale.
    Narrow,
    /// An exact number's units, 16 bytes.
    Wide,
    /// A datetime's instant, its first 8 bytes, a time zone after.
    Instant,
}

impl Ordinal {
    /// Of type `t`, its ordinal, if its record is one.
    pub fn of(t: &DataType) -> Option<Ordinal> {
        match t {
            DataType::Exact { .. } => match t.kind().record_width(t) {
                Some(NARROW) => Some(Ordinal::Narrow),
                Some(WIDE) => Some(Ordinal::Wide),
                Some(_) | None => None,
            },
            DataType::Date | DataType::Time { .. } | DataType::Timestamp { .. } => Some(Ordinal::Instant),
            DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Approximate { .. }
            | DataType::DecFloat { .. }
            | DataType::Binary { .. }
            | DataType::VarBinary { .. }
            | DataType::Lob { .. }
            | DataType::Boolean
            | DataType::Interval(_)
            | DataType::Array(_)
            | DataType::Multiset(_)
            | DataType::Structured(_)
            | DataType::Reference(_)
            | DataType::Row(_)
            | DataType::Distinct(_)
            | DataType::Json
            | DataType::Extension(_)
            | DataType::Xml(_) => None,
        }
    }

    /// The integer `record` holds, none if it is not one of its width.
    pub fn read(self, record: &[u8]) -> Option<i128> {
        match self {
            Ordinal::Narrow => <[u8; NARROW]>::try_from(record).ok().map(|b| i128::from(i64::from_le_bytes(b))),
            Ordinal::Wide => <[u8; WIDE]>::try_from(record).ok().map(i128::from_le_bytes),
            Ordinal::Instant => record.first_chunk::<NARROW>().map(|b| i128::from(i64::from_le_bytes(*b))),
        }
    }
}

/// A column of a load, its values as records of type `t`, each decoded
/// as it is read.
pub struct Decoded<'a> {
    /// Its type.
    pub t: &'a DataType,
    /// Its records.
    pub records: &'a Records,
    ordinal: Option<Ordinal>,
}

impl<'a> Decoded<'a> {
    /// The column `records` holds, of type `t`.
    pub fn new(t: &'a DataType, records: &'a Records) -> Decoded<'a> {
        Decoded { t, records, ordinal: Ordinal::of(t) }
    }

    /// The value of record `n`, if it decodes.
    fn decoded(&self, n: usize) -> Option<Value> {
        self.records.record(n).and_then(|r| self.t.decode_record(r))
    }

    /// As `Column::bounds`, of a column of `ordinal`, its integers
    /// compared: the first least and greatest found, then decoded. A
    /// record that is not one is a null, as it decodes to none.
    fn ordered_bounds(&self, ordinal: Ordinal, (from, to): (usize, usize)) -> Option<(Option<(Value, Value)>, u32)> {
        let (mut nulls, mut least, mut greatest) = (0_u32, None::<(i128, usize)>, None::<(i128, usize)>);
        for n in from..to.min(self.records.len()) {
            let Some(k) = self.records.record(n).and_then(|r| ordinal.read(r)) else {
                nulls = nulls.saturating_add(1);
                continue;
            };
            if least.is_none_or(|(l, _)| k < l) {
                least = Some((k, n));
            }
            if greatest.is_none_or(|(g, _)| k > g) {
                greatest = Some((k, n));
            }
        }
        let (Some((_, l)), Some((_, g))) = (least, greatest) else { return Some((None, nulls)) };
        Some((Some((self.decoded(l)?, self.decoded(g)?)), nulls))
    }
}

impl Column for Decoded<'_> {
    fn len(&self) -> usize {
        self.records.len()
    }

    fn value(&self, n: usize) -> Cow<'_, Value> {
        self.decoded(n).map_or(Cow::Borrowed(&Value::Null), Cow::Owned)
    }

    fn bounds(&self, (from, to): (usize, usize)) -> (Option<(Value, Value)>, u32) {
        self.ordinal.and_then(|o| self.ordered_bounds(o, (from, to))).unwrap_or_else(|| bounds_by(self, (from, to)))
    }
}
