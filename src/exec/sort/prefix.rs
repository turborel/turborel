//! \[DESIGN\] RULES.md D38: rows sorted by a prefix of their first key.
//! Each value's prefix is a number whose order is its key's, of values of
//! one kind: an exact number's floor, the first bytes of a string padded
//! with spaces, an approximate number's bits in order, a datetime's
//! instant in UTC (as a time or timestamp WITH TIME ZONE compares), an
//! interval's units, a bit string's first bits and its length; nulls placed
//! by `NULLS_SORT_HIGH`; reversed if the key is DESC. Two rows whose
//! prefixes differ are in that order. In memory, the prefixes and the
//! rows' places are sorted, side by side, rows of equal prefixes compared
//! whole, then by place, so the sort is stable; then the rows are put in
//! that order. If every prefix holds its value whole (an integer, an
//! instant or an interval within 64 bits, not at either end, a string of no
//! more than its bytes, an approximate number, a bit string of no more
//! than its bits), rows of equal prefixes are of equal first keys, and
//! only the rest are compared. A first key whose values are not all of one
//! kind of these, or of a number whose floor cannot be found, sorts the
//! rows whole. A spilled sort writes each row's prefix beside it, for its
//! merge (`merge.rs`).

use std::cmp::Ordering;

use super::compare_rows;
use crate::logical::SortKey;
use crate::storage::Row;
use crate::value::datetime::Moment;
use crate::value::{Bits, Decimal, NULLS_SORT_HIGH, Value, pow10};

/// The bit that orders a signed number's prefix after its sign.
const SIGN: u64 = 1 << 63;

/// The bytes of a string a prefix holds.
const PREFIX_BYTES: usize = 8;

/// The bytes of a bit string's bits a prefix holds; the last of its
/// `PREFIX_BYTES` holds the string's length.
const BITS_PREFIX_BYTES: usize = 7;

/// The most bits a bit string's prefix holds whole: `BITS_PREFIX_BYTES`
/// bytes of 8 bits.
const BITS_HELD: u32 = 56;

/// The length a bit string's prefix gives one longer than `BITS_HELD`:
/// greater than any it holds whole.
const LONG_BITS: u8 = u8::MAX;

/// A null's prefix, before a DESC key reverses it: the greatest if nulls
/// sort high, else the least.
const NULL_PREFIX: u64 = if NULLS_SORT_HIGH { u64::MAX } else { 0 };

/// `rows`, sorted by `keys` [X3.135-1986 §8.3 GR3b], stably.
pub(super) fn sorted(keys: &[SortKey], mut rows: Vec<Row>) -> Vec<Row> {
    let Some((mut order, whole)) = keys.first().and_then(|first| prefixes(first, &rows)) else {
        rows.sort_by(|a, b| compare_rows(keys, a, b));
        return rows;
    };
    // The keys rows of equal prefixes are compared by.
    let ties = if whole { keys.get(1..).unwrap_or_default() } else { keys };
    order.sort_unstable_by(|&(pa, a), &(pb, b)| {
        pa.cmp(&pb).then_with(|| rows.get(a).zip(rows.get(b)).map_or(Ordering::Equal, |(x, y)| compare_rows(ties, x, y))).then(a.cmp(&b))
    });
    let mut slots: Vec<Option<Row>> = rows.into_iter().map(Some).collect();
    order.iter().filter_map(|&(_, at)| slots.get_mut(at).and_then(Option::take)).collect()
}

/// What a sort key's value is, as its prefix knows it: the prefixes of
/// values of kinds alike (`alike`) are in their values' order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// A null, alike every kind.
    Null,
    /// An exact number.
    Exact,
    /// A character string.
    Char,
    /// An approximate number.
    Approx,
    /// A datetime of one type's fields: one of others' compares with it as
    /// equal.
    Datetime(Moment),
    /// An interval, year-month or not: one of the other class compares
    /// with it as equal.
    Interval(bool),
    /// A bit string.
    Bits,
}

/// Every kind, in the order of their codes, from 1 (`Kind::code`).
const KINDS: [Kind; 10] = [
    Kind::Null,
    Kind::Exact,
    Kind::Char,
    Kind::Approx,
    Kind::Datetime(Moment::Date),
    Kind::Datetime(Moment::Time),
    Kind::Datetime(Moment::Timestamp),
    Kind::Interval(true),
    Kind::Interval(false),
    Kind::Bits,
];

impl Kind {
    /// Its code, as a spilled record keeps it: from 1, 0 meaning none.
    pub(super) fn code(self) -> u8 {
        KINDS.iter().position(|&k| k == self).and_then(|at| u8::try_from(at.saturating_add(1)).ok()).unwrap_or(0)
    }

    /// The kind of `code`, if one has it.
    pub(super) fn of_code(code: u8) -> Option<Kind> {
        KINDS.get(usize::from(code).checked_sub(1)?).copied()
    }
}

/// Whether prefixes of values of kinds `a` and `b` are in their values'
/// order: of one kind, or of a null.
pub(super) fn alike(a: Kind, b: Kind) -> bool {
    a == b || a == Kind::Null || b == Kind::Null
}

/// The kind of a key's values, `held` so far, with one of `next`; none if
/// they are not alike.
fn joined(held: Kind, next: Kind) -> Option<Kind> {
    if !alike(held, next) {
        return None;
    }
    Some(if held == Kind::Null { next } else { held })
}

/// A sort key's value's prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Prefix {
    /// Its value's kind.
    pub(super) kind: Kind,
    /// The prefix, reversed of a DESC key.
    pub(super) bits: u64,
    /// Whether it holds its value whole: two such of kinds alike are equal
    /// only of equal values.
    pub(super) whole: bool,
}

/// The prefix of `value`, of a key `descending` or not; none if its kind
/// has none, or it is an exact number whose floor cannot be found.
pub(super) fn prefix_of(value: &Value, descending: bool) -> Option<Prefix> {
    let (kind, bits, held) = match value {
        Value::Null => (Kind::Null, NULL_PREFIX, true),
        Value::Exact(d) => {
            let (bits, held) = exact(d)?;
            (Kind::Exact, bits, held)
        }
        Value::Char(s) => (Kind::Char, text(s), s.len() <= PREFIX_BYTES),
        Value::Approx(f) => (Kind::Approx, approximate(*f), !f.is_nan()),
        // [ISO/IEC 9075:1992 §8.2 GR5] by their instants, in UTC.
        Value::Datetime(d) => {
            let (bits, held) = signed(i128::from(d.utc));
            (Kind::Datetime(d.moment), bits, held)
        }
        // [ISO/IEC 9075:1992 §8.2 GR6] by their units.
        Value::Interval(i) => {
            let (bits, held) = signed(i.units);
            (Kind::Interval(i.qualifier.year_month()), bits, held)
        }
        Value::Bits(b) => {
            let (bits, held) = bit_string(b);
            (Kind::Bits, bits, held)
        }
        Value::Extension(_) => return None,
    };
    // A value whose prefix is a null's does not hold it whole, so no null
    // ties a value held whole.
    let whole = held && (kind == Kind::Null || bits != NULL_PREFIX);
    Some(Prefix { kind, bits: if descending { !bits } else { bits }, whole })
}

/// Each row's prefix of `key` and its place, if its values are of one kind
/// that has prefixes; and whether every prefix holds its value whole.
fn prefixes(key: &SortKey, rows: &[Row]) -> Option<(Vec<(u64, usize)>, bool)> {
    let (mut kind, mut whole) = (Kind::Null, true);
    let mut out = Vec::with_capacity(rows.len());
    for (at, row) in rows.iter().enumerate() {
        let prefix = prefix_of(row.get(key.column)?, key.descending)?;
        kind = joined(kind, prefix.kind)?;
        whole &= prefix.whole;
        out.push((prefix.bits, at));
    }
    Some((out, whole))
}

/// The prefix of an exact number: its floor, held to 64 bits, in order;
/// and whether it is the number whole, an integer not at either end, so no
/// null's prefix is its.
fn exact(d: &Decimal) -> Option<(u64, bool)> {
    let (bits, held) = signed(d.units.div_euclid(pow10(d.scale)?));
    Some((bits, held && d.scale == 0))
}

/// The prefix of an integer: it, held to 64 bits, in order; and whether it
/// is the integer whole, not at either end.
fn signed(n: i128) -> (u64, bool) {
    let held = i64::try_from(n).unwrap_or(if n < 0 { i64::MIN } else { i64::MAX });
    let whole = held != i64::MIN && held != i64::MAX && i128::from(held) == n;
    (held.cast_unsigned() ^ SIGN, whole)
}

/// The prefix of a string: its first bytes, padded with spaces, as a
/// string is when compared [X3.135-1986 §5.11 GR3]; its bytes' order is
/// its characters' in UTF-8.
fn text(s: &str) -> u64 {
    let mut bytes = [b' '; PREFIX_BYTES];
    for (to, from) in bytes.iter_mut().zip(s.as_bytes()) {
        *to = *from;
    }
    u64::from_be_bytes(bytes)
}

/// The prefix of an approximate number: its bits, negative zero as zero,
/// in order.
fn approximate(f: f64) -> u64 {
    let bits = if f == 0.0_f64 { 0.0_f64.to_bits() } else { f.to_bits() };
    if bits & SIGN == 0 { bits | SIGN } else { !bits }
}

/// The prefix of a bit string, compared bit by bit, then by length
/// [ISO/IEC 9075:1992 §8.2 GR4]: its first bits, then its length, or
/// `LONG_BITS` if longer than they hold; and whether it is no longer. Of
/// two strings, one the start of the other, the shorter's prefix is no
/// greater: its bits are padded with 0s, and its length is less.
fn bit_string(b: &Bits) -> (u64, bool) {
    let mut bytes = [0_u8; PREFIX_BYTES];
    for (to, from) in bytes.iter_mut().zip(b.bytes().iter().take(BITS_PREFIX_BYTES)) {
        *to = *from;
    }
    let held = b.len() <= BITS_HELD;
    if let Some(last) = bytes.last_mut() {
        *last = if held { u8::try_from(b.len()).unwrap_or(LONG_BITS) } else { LONG_BITS };
    }
    (u64::from_be_bytes(bytes), held)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Datetime;
    use crate::value::datetime::Field;
    use crate::value::interval::{Interval, Qualifier};
    use crate::value::types::unregistered;

    /// A timestamp of `local` microseconds at a zone of `zone` minutes, of a
    /// type WITH TIME ZONE.
    fn stamp(local: i64, zone: i16) -> Value {
        Value::Datetime(Datetime::of_local(Moment::Timestamp, local, zone, true, 0_u32))
    }

    /// An interval of `units` of `start` to `end`.
    fn interval(units: i128, (start, end): (Field, Field)) -> Value {
        Value::Interval(Interval { units, qualifier: Qualifier { start, end, leading: 2_u8, fraction: 0_u8 } })
    }

    /// Verifies: [DESIGN] RULES.md D38 [ISO/IEC 9075:1992 §8.2 GR4, GR5,
    /// GR6]: of two values of one kind, the lesser's prefix is no greater,
    /// and those held whole differ: timestamps by their instants, whatever
    /// their zones, those of one instant alike; intervals by their units,
    /// past 64 bits not whole; bit strings bit by bit, then by length, past
    /// 56 bits not whole, the empty one least. A plug-in's value has none;
    /// every kind keeps its code.
    #[test]
    fn prefixes_of_datetimes_intervals_and_bits_keep_order() {
        let of = |v: &Value| prefix_of(v, false).unwrap();
        let hour = 3_600_000_000_i64;
        let stamps: Vec<Prefix> = [stamp(hour, 60_i16), stamp(hour, 0_i16), stamp(2_i64 * hour, 0_i16)].iter().map(of).collect();
        assert!(stamps.iter().map(|p| p.bits).is_sorted() && stamps.iter().all(|p| p.whole), "{stamps:?}");
        assert_eq!(of(&stamp(2_i64 * hour, 60_i16)).bits, stamps[1].bits, "one instant at two zones");
        let day = (Field::Day, Field::Second);
        let intervals: Vec<Prefix> = [-5_i128, 0_i128, 7_i128, i128::MAX].iter().map(|&n| of(&interval(n, day))).collect();
        assert!(intervals.iter().map(|p| p.bits).is_sorted(), "{intervals:?}");
        assert_eq!(intervals.iter().map(|p| p.whole).collect::<Vec<bool>>(), [true, true, true, false]);
        assert_eq!(of(&interval(1_i128, (Field::Year, Field::Month))).kind, Kind::Interval(true));
        let ones = |n: usize| "1".repeat(n);
        let digits = ["", "0", "00", "0011", "01", "1", "10"].map(str::to_owned);
        let digits = [&digits[..], &[ones(56_usize), ones(57_usize), ones(60_usize)]].concat();
        let bits: Vec<Prefix> = digits.iter().map(|d| of(&Value::Bits(Bits::of_bits(d).unwrap()))).collect();
        assert!(bits.iter().map(|p| p.bits).is_sorted(), "{bits:?}");
        assert!(bits.windows(2).take(8).all(|w| w[0].bits < w[1].bits), "whole ones differ");
        assert_eq!(bits.iter().map(|p| p.whole).collect::<Vec<bool>>(), [true, true, true, true, true, true, true, true, false, false]);
        assert_eq!(bits[8].bits, bits[9].bits, "alike past 56 bits");
        assert_eq!(prefix_of(&unregistered("POINT", "(1 2)", &[1_u8]), false), None);
        assert!(KINDS.iter().all(|&k| Kind::of_code(k.code()) == Some(k)));
        assert_eq!((Kind::of_code(0_u8), Kind::of_code(u8::MAX)), (None, None));
    }

    /// Verifies: [DESIGN] RULES.md D38 [X3.135-1986 §5.11 GR3, GR4]: of two
    /// values, the lesser's prefix is no greater: exact numbers of any
    /// scale, negative ones, those past 64 bits; strings padded, a
    /// character below a space, multibyte; approximate numbers, zeros of
    /// either sign alike.
    #[test]
    fn prefixes_keep_order() {
        let exacts = [
            Decimal::new(i128::MIN.saturating_add(1_i128), 0_u32),
            Decimal::new(-15_i128, 1_u32),
            Decimal::new(-1_i128, 0_u32),
            Decimal::new(0_i128, 0_u32),
            Decimal::new(5_i128, 1_u32),
            Decimal::new(1_i128, 0_u32),
            Decimal::new(i128::MAX, 0_u32),
        ];
        let made: Vec<(u64, bool)> = exacts.iter().map(|d| exact(d).unwrap()).collect();
        assert!(made.iter().map(|m| m.0).is_sorted(), "{made:?}");
        let wholes: Vec<bool> = made.iter().map(|m| m.1).collect();
        assert_eq!(wholes, [false, false, true, true, false, true, false], "fractions and the ends are not whole");
        assert_eq!(exact(&Decimal::new(5_i128, 1_u32)).map(|m| m.0), exact(&Decimal::new(0_i128, 0_u32)).map(|m| m.0), "a floor's tie");
        let texts: Vec<u64> = ["", "a\t", "a", "a ", "ab", "abcdefghij", "é"].iter().map(|s| text(s)).collect();
        assert!(texts.is_sorted(), "{texts:?}");
        assert_eq!(text("a"), text("a  "), "padded alike");
        let approx: Vec<u64> =
            [f64::NEG_INFINITY, -2.5_f64, -0.0_f64, 0.0_f64, 1e-300_f64, 3.0_f64].iter().map(|&f| approximate(f)).collect();
        assert!(approx.is_sorted(), "{approx:?}");
        assert_eq!(approximate(-0.0_f64), approximate(0.0_f64));
    }

    /// `rows` sorted by `keys`, as `sorted` sorts them and as comparing
    /// them whole, stably, does.
    fn both(keys: &[SortKey], rows: &[Row]) -> (Vec<Row>, Vec<Row>) {
        let mut whole = rows.to_vec();
        whole.sort_by(|a, b| compare_rows(keys, a, b));
        (sorted(keys, rows.to_vec()), whole)
    }

    /// Whether each of `columns`' rows, of their places too, sorted by
    /// prefixes as `sorted` does, are in the order comparing them whole
    /// gives, stably, ascending and DESC, alone and then by another key.
    fn as_whole(columns: &[Vec<Value>]) {
        let int = |n: i128| Value::Exact(Decimal::new(n, 0_u32));
        for column in columns {
            // The second column, the row's place, shows a sort's stability.
            let rows: Vec<Row> =
                column.iter().zip(0_i128..).map(|(v, n)| vec![v.clone(), int(n.checked_rem(2_i128).unwrap_or(0_i128)), int(n)]).collect();
            for descending in [false, true] {
                let one = [SortKey { column: 0, descending }];
                let (by_prefix, whole) = both(&one, &rows);
                assert_eq!(format!("{by_prefix:?}"), format!("{whole:?}"), "{column:?} {descending}");
                let two = [SortKey { column: 0, descending }, SortKey { column: 1, descending: !descending }];
                let (then_prefix, then_whole) = both(&two, &rows);
                assert_eq!(format!("{then_prefix:?}"), format!("{then_whole:?}"), "{column:?} {descending}, then another key");
            }
        }
    }

    /// Bit strings of `digits`, null of none.
    fn bit_strings(digits: &[String]) -> Vec<Value> {
        digits.iter().map(|d| if d.is_empty() { Value::Null } else { Value::Bits(Bits::of_bits(d).unwrap()) }).collect()
    }

    /// Verifies: [DESIGN] RULES.md D38 [X3.135-1986 §8.3 GR3b]: rows sorted
    /// by prefixes are in the order comparing them whole gives, stably:
    /// ascending and DESC, nulls among them, ties of a whole prefix broken
    /// by the next key, strings longer than a prefix, exact numbers of
    /// fractions, approximate numbers, bit strings; a key of two kinds
    /// sorted whole.
    #[test]
    fn sorted_by_prefixes_as_whole() {
        let int = |n: i128| Value::Exact(Decimal::new(n, 0_u32));
        let text = |t: &str| Value::Char(t.to_owned());
        as_whole(&[
            vec![int(3_i128), Value::Null, int(-1_i128), int(3_i128), int(i128::MAX), int(0_i128)],
            vec![text("pear"), text("apple pie"), text("apple tart"), Value::Null, text("apple pie "), text("b")],
            vec![
                Value::Exact(Decimal::new(15_i128, 1_u32)),
                Value::Exact(Decimal::new(12_i128, 1_u32)),
                int(1_i128),
                Value::Null,
                int(2_i128),
                int(1_i128),
            ],
            vec![
                Value::Approx(2.5_f64),
                Value::Approx(-0.0_f64),
                Value::Null,
                Value::Approx(0.0_f64),
                Value::Approx(-3.0_f64),
                Value::Approx(2.5_f64),
            ],
            vec![int(1_i128), text("a"), Value::Null, int(2_i128), int(0_i128), text("b")],
            bit_strings(&["1", "", "0", "1", "", "01"].map(str::to_owned)),
        ]);
        assert!(sorted(&[], vec![vec![int(2_i128)], vec![int(1_i128)]]).len() == 2, "no key, the rows as they were");
    }

    /// Verifies: [DESIGN] RULES.md D38 [X3.135-1986 §8.3 GR3b] [ISO/IEC
    /// 9075:1992 §8.2 GR4, GR5, GR6]: rows sorted by prefixes of timestamps
    /// of zones, of intervals, of bit strings longer and shorter than a
    /// prefix, are in the order comparing them whole gives, stably; a key
    /// of datetimes of two types, of intervals of two classes, or of a
    /// plug-in's values, which have no prefixes, sorted whole.
    #[test]
    fn sorted_by_prefixes_of_other_kinds_as_whole() {
        let date = Value::Datetime(Datetime::of_local(Moment::Date, 0_i64, 0_i16, false, 0_u32));
        let (day, month) = ((Field::Day, Field::Hour), (Field::Year, Field::Month));
        let ones = |n: usize| "10".repeat(n).chars().take(n).collect::<String>();
        let point = |n: u8| unregistered("POINT", "(1 2)", &[n]);
        as_whole(&[
            vec![stamp(9_i64, 60_i16), Value::Null, stamp(5_i64, 0_i16), stamp(9_i64, 0_i16), stamp(-4_i64, -60_i16), stamp(5_i64, 0_i16)],
            vec![
                interval(3_i128, day),
                interval(-2_i128, day),
                Value::Null,
                interval(3_i128, day),
                interval(0_i128, day),
                interval(i128::MIN, day),
            ],
            bit_strings(&[ones(70_usize), String::new(), ones(57_usize), ones(58_usize), "1".to_owned(), ones(56_usize)]),
            vec![stamp(3_i64, 0_i16), date.clone(), stamp(1_i64, 0_i16), Value::Null, date, stamp(2_i64, 0_i16)],
            vec![
                interval(3_i128, day),
                interval(2_i128, month),
                interval(1_i128, day),
                Value::Null,
                interval(1_i128, month),
                interval(0_i128, day),
            ],
            vec![point(2_u8), Value::Null, point(1_u8), point(2_u8), point(0_u8), Value::Null],
        ]);
    }
}
