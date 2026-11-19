//! \[DESIGN\] How wide a column of a query's rows is shown (RULES.md
//! D46): as wide as its type's values are, or its name, so no row need be
//! held to measure them; numbers aligned right, text left. A value wider
//! than its column is shown whole, past it (`format.rs`).

use turborel::value::DataType;
use turborel::value::types::MAX_TYPE_NESTING;

/// The widest a column is made for its type: wider values go past it.
pub const MAX_WIDTH: usize = 60;
/// The characters of an approximate number at its longest:
/// `-1.2345678901234567E-308`.
const APPROXIMATE_WIDTH: usize = 24;
/// The characters of a truth value at its longest: `FALSE`.
const BOOLEAN_WIDTH: usize = 5;
/// The hexits an octet of a binary string shows as [ISO/IEC 9075-2:2023
/// §5.3 SR20].
const HEXITS_PER_OCTET: usize = 2;

/// How a column is shown.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Shape {
    /// Its width, in columns of a screen.
    pub(crate) width: usize,
    /// Whether its values align right.
    pub(crate) right: bool,
}

/// [ISO/IEC 9075:1992 §6.1 SR27] The positions of a DATE, of a TIME and of
/// a TIMESTAMP without their fractions and displacements.
const DATE_WIDTH: usize = 10;
/// See [`DATE_WIDTH`].
const TIME_WIDTH: usize = 8;
/// The positions of a DECFLOAT value beside its digits: a sign, a 0 and a
/// point, then six zeros before them (IV110).
const DECFLOAT_EXTRA: usize = 9;
/// See [`DATE_WIDTH`].
const TIMESTAMP_WIDTH: usize = 19;
/// See [`DATE_WIDTH`]: of a displacement, ±HH:MM.
const ZONE_WIDTH: usize = 6;

/// The positions of a time or timestamp of `base` positions, `precision`
/// digits of fraction, and a displacement if `zoned`.
fn positions(base: usize, precision: u32, zoned: bool) -> usize {
    let fraction = usize::try_from(precision).unwrap_or(0);
    base.saturating_add(fraction).saturating_add(usize::from(fraction > 0)).saturating_add(if zoned { ZONE_WIDTH } else { 0 })
}

/// What a column of `t`'s values needs, its name `named` columns wide.
pub(crate) fn shape(named: usize, t: &DataType) -> Shape {
    let (width, right) = measured(t, 0);
    Shape { width: width.min(MAX_WIDTH).max(named), right }
}

/// The characters of an array's text beside its elements: `ARRAY[]`.
const ARRAY_EXTRA: usize = 7;
/// The characters of a row's text beside its fields: `ROW()`.
const ROW_EXTRA: usize = 5;
/// The characters each element or field of a composite's text takes beside
/// its own: the `, ` between them, or a string's quotes.
const PART_EXTRA: usize = 2;

/// The width of `t`'s values, at most `MAX_WIDTH`, nested `depth` in
/// composite types (at most `MAX_TYPE_NESTING`, as the types are; RULES.md
/// R-CF-6), and whether they align right.
fn measured(t: &DataType, depth: usize) -> (usize, bool) {
    let part = |inner: &DataType| {
        if depth >= MAX_TYPE_NESTING { MAX_WIDTH } else { measured(inner, depth.saturating_add(1)).0.saturating_add(PART_EXTRA) }
    };
    let (width, right) = match t {
        DataType::Character { length, .. } | DataType::Varying { length, .. } => (usize::try_from(*length).unwrap_or(MAX_WIDTH), false),
        // A binary string as its hexits, two to an octet.
        DataType::Binary { length } | DataType::VarBinary { length } => {
            (usize::try_from(*length).unwrap_or(MAX_WIDTH).saturating_mul(HEXITS_PER_OCTET), false)
        }
        // A sign and a point beside the digits.
        DataType::Exact { precision, .. } => (usize::try_from(*precision).unwrap_or(MAX_WIDTH).saturating_add(2), true),
        DataType::Approximate { .. } => (APPROXIMATE_WIDTH, true),
        // Its digits, and a sign, a point and six zeros, or a sign, a
        // point and an exponent of five.
        DataType::DecFloat { precision } => (usize::try_from(*precision).unwrap_or(MAX_WIDTH).saturating_add(DECFLOAT_EXTRA), true),
        DataType::Boolean => (BOOLEAN_WIDTH, false),
        // [ISO/IEC 9075:1992 §6.1 SR27] its length in positions.
        DataType::Date => (DATE_WIDTH, false),
        DataType::Time { precision, zoned } => (positions(TIME_WIDTH, *precision, *zoned), false),
        DataType::Timestamp { precision, zoned } => (positions(TIMESTAMP_WIDTH, *precision, *zoned), false),
        // [ISO/IEC 9075:1992 §10.1 GR5] its positions, and its sign.
        DataType::Interval(q) => (usize::try_from(q.positions()).unwrap_or(MAX_WIDTH).saturating_add(1), true),
        // [ISO/IEC 9075-2:2023 §6.49, §7.1] its text, each element or field
        // as its literal.
        DataType::Array(a) => {
            (part(&a.element).saturating_mul(usize::try_from(a.max).unwrap_or(MAX_WIDTH)).saturating_add(ARRAY_EXTRA), false)
        }
        DataType::Row(r) => (r.fields.iter().map(|f| part(&f.data_type)).fold(ROW_EXTRA, usize::saturating_add), false),
        // A distinct type as its source, as its values are.
        DataType::Distinct(d) => measured(&d.source, depth),
        // A type beyond the standard, shown as its text, as wide as it is;
        // a large object as its text or hexits, as wide as a column grows.
        // A multiset, of no maximum cardinality [ISO/IEC 9075-2:2023
        // §6.52], as wide as a column may be; a structured value, whose
        // attributes the catalog holds, a reference, a JSON value's text
        // and an XML value's serialization, as wide too.
        DataType::Extension(_)
        | DataType::Xml(_)
        | DataType::Lob { .. }
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Json => (MAX_WIDTH, false),
    };
    (width.min(MAX_WIDTH), right)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D46; [ISO/IEC 9075:1992 §6.1 SR27] a
    /// column of a datetime is as wide as its length in positions, its
    /// fraction and displacement counted; of a binary string, its hexits;
    /// never narrower than its name.
    #[test]
    fn datetime_columns_are_as_wide_as_their_positions() {
        let width = |t: DataType| shape(1, &t).width;
        assert_eq!(width(DataType::Date), 10);
        assert_eq!(width(DataType::Time { precision: 0, zoned: false }), 8);
        assert_eq!(width(DataType::Time { precision: 2, zoned: true }), 17);
        assert_eq!(width(DataType::Timestamp { precision: 6, zoned: false }), 26);
        assert_eq!(width(DataType::Timestamp { precision: 3, zoned: true }), 29);
        assert_eq!(width(DataType::VarBinary { length: 12 }), 24);
        assert_eq!(shape(12, &DataType::Date).width, 12);
        let day_second = turborel::value::interval::Qualifier::new(
            turborel::value::datetime::Field::Day,
            turborel::value::datetime::Field::Second,
            2,
            6,
        );
        assert_eq!(width(DataType::Interval(day_second)), 19, "its positions and its sign");
    }
}
