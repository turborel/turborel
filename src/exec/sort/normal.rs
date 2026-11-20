//! \[DESIGN\] RULES.md D364: normalized sort keys. Every row's sort keys are
//! made one string of bytes of a width fixed for the sort, whose order as
//! bytes is the order `compare_rows` gives its row [X3.135-1986 §8.3 GR3b]:
//! for each key in turn, a byte placing a null first or last, as its null
//! ordering says [ISO/IEC 9075-2:2023 §10.10 GR1e ii], left out if no row's
//! value is null; then the value's bytes, every one inverted of a DESC key
//! \[GR1c\]. An exact number is its units at the key's greatest scale less
//! the least of them, big-endian, in as few bytes as the difference of the
//! greatest and the least needs; an approximate number its bits in order,
//! negative zero as zero; a datetime its instant in UTC and an interval its
//! units, as exact numbers are; a truth value one byte. A character string
//! is its first bytes, padded with spaces as a comparison pads it
//! [X3.135-1986 §5.11 GR3], then a byte saying whether what is past them
//! is less than spaces, equal or greater; a binary string, as a collated
//! key's bits are (RULES.md D60), its first octets padded with zeros, then
//! its length, or one more than they hold. The bytes of a string are as
//! many as its longest value's, at most `PREFIX_MOST`, so keys of a long
//! shared start are told apart by their bytes.
//!
//! Rows of equal keys of strings longer than their bytes may differ; the
//! sort compares them whole (`order.rs`). A key whose values are of a
//! type with no such form (DECFLOAT, a large object, a composite), of
//! kinds not alike, an approximate number not a number, or exact numbers
//! past 128 bits at their greatest scale, has none, and the sort sorts as
//! before, by its first key's prefix and its values (`prefix.rs`).
//!
//! A layout is made of what the rows hold, found first (`Found`). A run of
//! a spilled sort, or a top-N, sees its rows a part at a time, so its layout
//! is `Shape::Open`: every key nullable, numbers at their whole width, so
//! the layout made of the first part holds the later ones, which `admits`
//! checks.

mod encode;
mod order;
mod radix;

pub(super) use encode::{encode_batch, encode_row};
pub(super) use order::order;

use crate::logical::SortKey;
use crate::storage::{Batch, Cell, Column, Row, Texts};
use crate::value::datetime::Moment;
use crate::value::{SortOrder, Value, pow10};

/// \[DESIGN\] The most bytes a string's normal form holds of it: enough for
/// most keys of names and codes whole, few enough that a key of several
/// strings stays a short compare.
pub(super) const PREFIX_MOST: usize = 32;

/// What the values of one key were found to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Seen {
    /// No value, or nulls alone.
    Nothing,
    /// Exact numbers, their units at `scale` from `least` to `most`.
    Exact { scale: u32, least: i128, most: i128 },
    /// Approximate numbers.
    Approx,
    /// Character strings, of `longest` bytes at most.
    Text { longest: usize },
    /// Binary strings, of `longest` octets at most.
    Binary { longest: usize },
    /// Datetimes of `moment`'s fields, their instants from `least` to `most`.
    Instant { moment: Moment, least: i128, most: i128 },
    /// Intervals, year-month or not, their units from `least` to `most`.
    Interval { year_month: bool, least: i128, most: i128 },
    /// Truth values.
    Bool,
}

/// What one key's values were found to be, and whether any is null.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Found {
    pub(super) seen: Seen,
    pub(super) nulls: bool,
}

impl Default for Found {
    fn default() -> Found {
        Found { seen: Seen::Nothing, nulls: false }
    }
}

/// How a layout is made: of every row sorted, as narrow as they allow; or
/// of a part of them, wide enough for any later part of the same kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shape {
    /// Of every row.
    Tight,
    /// Of a part of them.
    Open,
}

/// How one key's values are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Code {
    /// Of no value but nulls: no bytes.
    Nothing,
    /// Integers (exact units at `scale`, instants, interval units) less
    /// `base`, in `bytes` bytes.
    Int { scale: u32, base: i128, bytes: usize },
    /// Approximate numbers' bits, in 8 bytes.
    Approx,
    /// A character string's first `prefix` bytes and a byte of the rest.
    Text { prefix: usize },
    /// A binary string's first `prefix` octets and a byte of its length.
    Binary { prefix: usize },
    /// A truth value, a byte.
    Bool,
}

/// The kind of value a field's integers are, so values of another kind
/// are refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ints {
    /// Exact numbers.
    Exact,
    /// Datetimes of these fields.
    Instant(Moment),
    /// Intervals, year-month or not.
    Interval(bool),
}

/// One key's place in a row's normal key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Field {
    /// The key's column.
    pub(super) column: usize,
    /// Its direction and null ordering.
    pub(super) order: SortOrder,
    /// How its values are written.
    pub(super) code: Code,
    /// The kind of an integer field's values.
    pub(super) ints: Ints,
    /// Whether it begins with a byte placing a null.
    pub(super) nullable: bool,
    /// Where it begins in the row's key.
    pub(super) at: usize,
}

impl Field {
    /// Its bytes, the null byte among them.
    pub(super) fn width(&self) -> usize {
        usize::from(self.nullable).saturating_add(self.code.bytes())
    }
}

impl Code {
    /// The bytes a value takes.
    fn bytes(self) -> usize {
        match self {
            Code::Nothing => 0,
            Code::Int { bytes, .. } => bytes,
            Code::Approx => size_of::<u64>(),
            Code::Text { prefix } | Code::Binary { prefix } => prefix.saturating_add(1),
            Code::Bool => 1,
        }
    }
}

/// A sort's normal keys: each key's field, the width of a row's key, and
/// whether rows of equal keys may differ, a string being longer than its
/// field holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Layout {
    pub(super) fields: Vec<Field>,
    pub(super) width: usize,
    pub(super) truncates: bool,
}

impl Layout {
    /// The layout of `keys`, whose values were found as `found` says, of
    /// `shape`; none if a key's values have no normal form.
    pub(super) fn new(keys: &[SortKey], found: &[Option<Found>], shape: Shape) -> Option<Layout> {
        let (mut fields, mut at, mut truncates) = (Vec::with_capacity(keys.len()), 0_usize, false);
        for (key, each) in keys.iter().zip(found) {
            let seen = (*each)?;
            let (code, ints) = code_of(seen.seen, shape);
            truncates |= cut(seen.seen, code);
            let nullable = seen.nulls || shape == Shape::Open;
            let field = Field { column: key.column, order: key.order(), code, ints, nullable, at };
            at = at.checked_add(field.width())?;
            fields.push(field);
        }
        (fields.len() == keys.len()).then_some(Layout { fields, width: at, truncates })
    }

    /// Whether rows whose keys were found as `found` say are written by it
    /// as their own layout would order them.
    pub(super) fn admits(&self, found: &[Option<Found>]) -> bool {
        self.fields.len() == found.len() && self.fields.iter().zip(found).all(|(f, found)| found.is_some_and(|x| holds(f, x)))
    }
}

/// Whether `field` holds values found as `found` says.
fn holds(field: &Field, found: Found) -> bool {
    if found.nulls && !field.nullable {
        return false;
    }
    match (found.seen, field.code) {
        (Seen::Nothing, _)
        | (Seen::Approx, Code::Approx)
        | (Seen::Bool, Code::Bool)
        | (Seen::Text { .. }, Code::Text { .. })
        | (Seen::Binary { .. }, Code::Binary { .. }) => true,
        (Seen::Exact { scale, least, most }, Code::Int { scale: to, base, bytes }) => {
            // Of a greater scale than the field's, `ranged` gives none.
            field.ints == Ints::Exact && ranged((least, most), (scale, to)).is_some_and(|r| within(r, base, bytes))
        }
        (Seen::Instant { moment, least, most }, Code::Int { base, bytes, .. }) => {
            field.ints == Ints::Instant(moment) && within((least, most), base, bytes)
        }
        (Seen::Interval { year_month, least, most }, Code::Int { base, bytes, .. }) => {
            field.ints == Ints::Interval(year_month) && within((least, most), base, bytes)
        }
        (
            Seen::Exact { .. }
            | Seen::Approx
            | Seen::Text { .. }
            | Seen::Binary { .. }
            | Seen::Instant { .. }
            | Seen::Interval { .. }
            | Seen::Bool,
            Code::Nothing | Code::Int { .. } | Code::Approx | Code::Text { .. } | Code::Binary { .. } | Code::Bool,
        ) => false,
    }
}

/// Whether `least` to `most` less `base` fit `bytes` bytes.
fn within((least, most): (i128, i128), base: i128, bytes: usize) -> bool {
    least >= base && most >= least && bytes_of(most.wrapping_sub(base).cast_unsigned()) <= bytes
}

/// `least` and `most`, units at `from`, at `to`, if they fit 128 bits.
fn ranged((least, most): (i128, i128), (from, to): (u32, u32)) -> Option<(i128, i128)> {
    let factor = pow10(to.checked_sub(from)?)?;
    Some((least.checked_mul(factor)?, most.checked_mul(factor)?))
}

/// The bytes that hold `n`.
fn bytes_of(n: u128) -> usize {
    let bits = u128::BITS.saturating_sub(n.leading_zeros());
    usize::try_from(bits.div_ceil(u8::BITS)).unwrap_or(size_of::<u128>())
}

/// How values seen as `seen` are written in a layout of `shape`, and of
/// what kind its integers are.
fn code_of(seen: Seen, shape: Shape) -> (Code, Ints) {
    let int = |(least, most): (i128, i128), scale: u32| match shape {
        Shape::Tight => Code::Int { scale, base: least, bytes: bytes_of(most.wrapping_sub(least).cast_unsigned()) },
        Shape::Open => open_int((least, most), scale),
    };
    match seen {
        Seen::Nothing => (Code::Nothing, Ints::Exact),
        Seen::Exact { scale, least, most } => (int((least, most), scale), Ints::Exact),
        Seen::Approx => (Code::Approx, Ints::Exact),
        Seen::Text { longest } => (Code::Text { prefix: prefix_of(longest, shape) }, Ints::Exact),
        Seen::Binary { longest } => (Code::Binary { prefix: prefix_of(longest, shape) }, Ints::Exact),
        Seen::Instant { moment, least, most } => (int((least, most), 0), Ints::Instant(moment)),
        Seen::Interval { year_month, least, most } => (int((least, most), 0), Ints::Interval(year_month)),
        Seen::Bool => (Code::Bool, Ints::Exact),
    }
}

/// An integer field of an open layout, at `scale`, of values from `least`
/// to `most`: every value of 64 bits, or of 128 if they are past them.
fn open_int((least, most): (i128, i128), scale: u32) -> Code {
    let (base, bytes) = (i128::from(i64::MIN), size_of::<u64>());
    if within((least, most), base, bytes) {
        Code::Int { scale, base, bytes }
    } else {
        Code::Int { scale, base: i128::MIN, bytes: size_of::<u128>() }
    }
}

/// The bytes of a string a field of `shape` holds, of strings of `longest`.
fn prefix_of(longest: usize, shape: Shape) -> usize {
    match shape {
        Shape::Tight => longest.min(PREFIX_MOST),
        Shape::Open => PREFIX_MOST,
    }
}

/// Whether some value seen as `seen` is longer than `code` holds.
fn cut(seen: Seen, code: Code) -> bool {
    match (seen, code) {
        (Seen::Text { longest } | Seen::Binary { longest }, Code::Text { prefix } | Code::Binary { prefix }) => longest > prefix,
        (
            Seen::Nothing
            | Seen::Exact { .. }
            | Seen::Approx
            | Seen::Text { .. }
            | Seen::Binary { .. }
            | Seen::Instant { .. }
            | Seen::Interval { .. }
            | Seen::Bool,
            _,
        ) => false,
    }
}

/// Values seen as `a` and as `b`, seen together; none if not alike.
pub(super) fn joined(first: Seen, second: Seen) -> Option<Seen> {
    Some(match (first, second) {
        (Seen::Nothing, seen) | (seen, Seen::Nothing) => seen,
        (Seen::Exact { scale, least, most }, Seen::Exact { scale: other, least: low, most: high }) => {
            let to = scale.max(other);
            let ((l0, m0), (l1, m1)) = (ranged((least, most), (scale, to))?, ranged((low, high), (other, to))?);
            Seen::Exact { scale: to, least: l0.min(l1), most: m0.max(m1) }
        }
        (Seen::Approx, Seen::Approx) => Seen::Approx,
        (Seen::Bool, Seen::Bool) => Seen::Bool,
        (Seen::Text { longest }, Seen::Text { longest: other }) => Seen::Text { longest: longest.max(other) },
        (Seen::Binary { longest }, Seen::Binary { longest: other }) => Seen::Binary { longest: longest.max(other) },
        (Seen::Instant { moment, least, most }, Seen::Instant { moment: other, least: low, most: high }) => {
            (moment == other).then_some(Seen::Instant { moment, least: least.min(low), most: most.max(high) })?
        }
        (Seen::Interval { year_month, least, most }, Seen::Interval { year_month: other, least: low, most: high }) => {
            (year_month == other).then_some(Seen::Interval { year_month, least: least.min(low), most: most.max(high) })?
        }
        (
            Seen::Exact { .. }
            | Seen::Approx
            | Seen::Bool
            | Seen::Text { .. }
            | Seen::Binary { .. }
            | Seen::Instant { .. }
            | Seen::Interval { .. },
            _,
        ) => return None,
    })
}

/// `found` with `more` found too; none if not alike.
pub(super) fn with(found: Option<Found>, more: Option<Found>) -> Option<Found> {
    let (a, b) = (found?, more?);
    Some(Found { seen: joined(a.seen, b.seen)?, nulls: a.nulls || b.nulls })
}

/// What a value is, as a key's: none if it has no normal form. A null is
/// `Found` of nothing, null.
pub(super) fn seen_value(value: &Value) -> Option<Found> {
    let seen = match value {
        Value::Null => return Some(Found { seen: Seen::Nothing, nulls: true }),
        Value::Exact(d) => Seen::Exact { scale: d.scale, least: d.units, most: d.units },
        Value::Approx(f) => (!f.is_nan()).then_some(Seen::Approx)?,
        Value::Char(s) => Seen::Text { longest: s.len() },
        Value::Binary(b) => Seen::Binary { longest: b.bytes().len() },
        Value::Datetime(d) => Seen::Instant { moment: d.moment, least: i128::from(d.utc), most: i128::from(d.utc) },
        Value::Interval(i) => Seen::Interval { year_month: i.qualifier.year_month(), least: i.units, most: i.units },
        Value::Bool(_) => Seen::Bool,
        Value::DecFloat(_)
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Json(_)
        | Value::Extension(_)
        | Value::Xml(_) => return None,
    };
    Some(Found { seen, nulls: false })
}

/// What a batch's `cell` is, as `seen_value` says of its value.
fn seen_cell(cell: Cell<'_>) -> Option<Found> {
    let seen = match cell {
        Cell::Value(v) => return seen_value(v),
        Cell::Null => return Some(Found { seen: Seen::Nothing, nulls: true }),
        Cell::Exact(units, scale) => Seen::Exact { scale, least: i128::from(units), most: i128::from(units) },
        Cell::Approx(f) => (!f.is_nan()).then_some(Seen::Approx)?,
        Cell::Text(t) => Seen::Text { longest: t.len() },
        Cell::Datetime(d) => Seen::Instant { moment: d.moment, least: i128::from(d.utc), most: i128::from(d.utc) },
        Cell::Bool(_) => Seen::Bool,
    };
    Some(Found { seen, nulls: false })
}

/// What column `at` of `batch` holds at `rows`, as a key's: of a vector of
/// one kind, read of the whole vector, which holds no less; of values,
/// of each row's.
pub(super) fn seen_column(batch: &Batch, at: usize, rows: &[usize]) -> Option<Found> {
    let whole = |seen: Seen, nulls: bool| Some(Found { seen, nulls });
    match batch.columns.get(at)? {
        Column::Ints(ints) => {
            let (least, most) = ints.units.iter().fold((i64::MAX, i64::MIN), |(l, m), &u| (l.min(u), m.max(u)));
            let seen = if ints.units.is_empty() { Seen::Nothing } else { exact_seen(ints.scale, least, most) };
            whole(seen, ints.nulls.as_ref().is_some_and(|n| rows.iter().any(|&r| n.get(r))))
        }
        Column::Datetimes(d) => {
            let (least, most) = d.utc.iter().fold((i64::MAX, i64::MIN), |(l, m), &u| (l.min(u), m.max(u)));
            let seen =
                if d.utc.is_empty() { Seen::Nothing } else { Seen::Instant { moment: d.moment, least: least.into(), most: most.into() } };
            whole(seen, d.nulls.as_ref().is_some_and(|n| rows.iter().any(|&r| n.get(r))))
        }
        Column::Bools(b) => whole(Seen::Bool, b.nulls.as_ref().is_some_and(|n| rows.iter().any(|&r| n.get(r)))),
        Column::Nulls(_) => whole(Seen::Nothing, !rows.is_empty()),
        Column::Floats(f) => {
            let nan = f.numbers.iter().any(|n| n.is_nan());
            (!nan).then_some(Found { seen: Seen::Approx, nulls: f.nulls.as_ref().is_some_and(|n| rows.iter().any(|&r| n.get(r))) })
        }
        Column::Texts(t) => {
            whole(Seen::Text { longest: longest_text(t) }, t.nulls.as_ref().is_some_and(|n| rows.iter().any(|&r| n.get(r))))
        }
        Column::Values(_) => rows.iter().try_fold(Found::default(), |found, &r| with(Some(found), seen_cell(batch.cell(at, r)))),
    }
}

/// The bytes of the longest of `texts`, of a dictionary's entries if coded.
fn longest_text(texts: &Texts) -> usize {
    let ends =
        |t: &Texts| t.ends.iter().scan(0_usize, |from, &end| Some(end.saturating_sub(std::mem::replace(from, end)))).max().unwrap_or(0);
    texts.coded.as_ref().map_or_else(|| ends(texts), |c| ends(&c.entries))
}

/// Exact numbers of `scale` from `least` to `most` units.
fn exact_seen(scale: u32, least: i64, most: i64) -> Seen {
    Seen::Exact { scale, least: least.into(), most: most.into() }
}

/// What each of `keys` holds in `rows`.
pub(super) fn seen_rows(keys: &[SortKey], rows: &[Row]) -> Vec<Option<Found>> {
    keys.iter()
        .map(|k| rows.iter().try_fold(Found::default(), |found, row| with(Some(found), row.get(k.column).and_then(seen_value))))
        .collect()
}

// Unit tests: layouts, and keys as bytes against the comparator, of every
// kind (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
