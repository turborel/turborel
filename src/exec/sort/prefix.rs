//! \[DESIGN\] RULES.md D38: rows sorted in memory by a prefix of their
//! first key. Each row's prefix is a number whose order is its key's (an
//! exact number's floor, the first bytes of a string padded with spaces,
//! an approximate number's bits in order), nulls placed by
//! `NULLS_SORT_HIGH`, and reversed if the key is DESC: two rows whose
//! prefixes differ are in that order. The prefixes and the rows' places
//! are sorted, side by side in memory, rows of equal prefixes compared
//! whole, then by place, so the sort is stable; then the rows are put in
//! that order. If every prefix holds its value whole (an integer within 64
//! bits, not at either end, a string of no more than its bytes, an
//! approximate number), rows of equal prefixes are of equal first keys,
//! and only the rest are compared. A first key whose values are not all of one kind of these
//! three, or of a number whose floor cannot be found, sorts the rows whole.

use std::cmp::Ordering;

use super::compare_rows;
use crate::logical::SortKey;
use crate::storage::Row;
use crate::value::{Decimal, NULLS_SORT_HIGH, Value, pow10};

/// The bit that orders a signed number's prefix after its sign.
const SIGN: u64 = 1 << 63;

/// The bytes of a string a prefix holds.
const PREFIX_BYTES: usize = 8;

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

/// What a sort key's values are, those not null.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Exact,
    Char,
    Approx,
}

/// Each row's prefix of `key` and its place, if its values are of one kind
/// that has prefixes; and whether every prefix holds its value whole.
fn prefixes(key: &SortKey, rows: &[Row]) -> Option<(Vec<(u64, usize)>, bool)> {
    let (mut kind, mut whole) = (None, true);
    let mut out = Vec::with_capacity(rows.len());
    for (at, row) in rows.iter().enumerate() {
        let value = row.get(key.column)?;
        let (prefix, held) = match (value, kind) {
            (Value::Null, _) => (if NULLS_SORT_HIGH { u64::MAX } else { 0 }, true),
            (Value::Exact(d), None | Some(Kind::Exact)) => {
                kind = Some(Kind::Exact);
                exact(d)?
            }
            (Value::Char(s), None | Some(Kind::Char)) => {
                kind = Some(Kind::Char);
                (text(s), s.len() <= PREFIX_BYTES)
            }
            (Value::Approx(f), None | Some(Kind::Approx)) => {
                kind = Some(Kind::Approx);
                (approximate(*f), !f.is_nan())
            }
            (Value::Exact(_) | Value::Char(_) | Value::Approx(_), Some(_))
            | (Value::Bits(_) | Value::Datetime(_) | Value::Interval(_) | Value::Extension(_), _) => return None,
        };
        whole &= held;
        out.push((if key.descending { !prefix } else { prefix }, at));
    }
    Some((out, whole))
}

/// The prefix of an exact number: its floor, held to 64 bits, in order;
/// and whether it is the number whole, an integer not at either end, so no
/// null's prefix is its.
fn exact(d: &Decimal) -> Option<(u64, bool)> {
    let floor = d.units.div_euclid(pow10(d.scale)?);
    let held = i64::try_from(floor).unwrap_or(if floor < 0 { i64::MIN } else { i64::MAX });
    let whole = d.scale == 0 && held != i64::MIN && held != i64::MAX && i128::from(held) == d.units;
    Some((held.cast_unsigned() ^ SIGN, whole))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Bits;

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

    /// Verifies: [DESIGN] RULES.md D38 [X3.135-1986 §8.3 GR3b]: rows sorted
    /// by prefixes are in the order comparing them whole gives, stably:
    /// ascending and DESC, nulls among them, ties of a whole prefix broken
    /// by the next key, strings longer than a prefix, exact numbers of
    /// fractions, approximate numbers; a key of two kinds, or of none with
    /// prefixes, sorted whole.
    #[test]
    fn sorted_by_prefixes_as_whole() {
        let int = |n: i128| Value::Exact(Decimal::new(n, 0_u32));
        let text = |t: &str| Value::Char(t.to_owned());
        let columns: [Vec<Value>; 6] = [
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
            ["1", "", "0", "1", "", "01"]
                .iter()
                .map(|d| if d.is_empty() { Value::Null } else { Value::Bits(Bits::of_bits(d).unwrap()) })
                .collect(),
        ];
        for column in &columns {
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
        assert!(sorted(&[], vec![vec![int(2_i128)], vec![int(1_i128)]]).len() == 2, "no key, the rows as they were");
    }
}
