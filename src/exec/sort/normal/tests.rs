use super::super::compare_rows;
use super::super::keyed::{Layouts, sorted_places, sorted_rows};
use super::*;
use crate::storage::{Floats, Ints as IntVector, Texts as TextVector};
use crate::value::datetime::Field as DatetimeField;
use crate::value::interval::{Interval, Qualifier};
use crate::value::{Binary, Datetime, Decimal};

/// Each direction under each null ordering.
const ORDERS: [SortOrder; 4] = [
    SortOrder { descending: false, nulls_first: false },
    SortOrder { descending: false, nulls_first: true },
    SortOrder { descending: true, nulls_first: false },
    SortOrder { descending: true, nulls_first: true },
];

/// Strings that differ past 32 bytes, by trailing spaces, by a character
/// below a space, multibyte.
const TEXTS: [&str; 12] = [
    "",
    "a",
    "a ",
    "a\t",
    "ab",
    "b",
    "é",
    "a  \tz",
    "abcdefghijklmnopqrstuvwxyz0123456789",
    "abcdefghijklmnopqrstuvwxyz0123456789 ",
    "abcdefghijklmnopqrstuvwxyz0123456788x",
    "abcdefghijklmnopqrstuvwxyz012345678\u{1}",
];

/// Approximate numbers: zeros of either sign, infinities.
const NUMBERS: [f64; 9] = [-0.0, 0.0, 1.5, -1.5, f64::INFINITY, f64::NEG_INFINITY, 1e-300, -2.0, 3.25];

/// A generator of numbers from a fixed seed (R-TEST-5): xorshift64*.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12_u32;
        self.0 ^= self.0 << 25_u32;
        self.0 ^= self.0 >> 27_u32;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D_u64)
    }

    /// A number below `n`.
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next().checked_rem(u64::try_from(n).unwrap()).unwrap()).unwrap()
    }

    /// A number from -20 to 19.
    fn small(&mut self) -> i128 {
        i128::try_from(self.below(40)).unwrap().wrapping_sub(20_i128)
    }
}

/// A binary string of `n` octets of `octet`.
fn octets(octet: u8, n: usize) -> Value {
    Value::Binary(Binary::new(vec![octet; n]))
}

/// A value of `kind`, from `g`, null one time in seven.
fn value(kind: usize, g: &mut Seeded) -> Value {
    if g.below(7) == 0 {
        return Value::Null;
    }
    let n = g.small();
    match kind {
        0 => Value::Exact(Decimal::new(n.wrapping_mul(1_000_003_i128).wrapping_rem(977_i128), 0_u32)),
        1 => Value::Exact(Decimal::new(n, u32::try_from(g.below(3)).unwrap())),
        2 => Value::Exact(Decimal::new(if g.below(2) == 0 { i128::from(i64::MAX).wrapping_mul(n) } else { n }, 1_u32)),
        3 => Value::Approx(NUMBERS[g.below(NUMBERS.len())]),
        4 => Value::Char(TEXTS[g.below(TEXTS.len())].to_owned()),
        5 => {
            [octets(0, 0), octets(0, 1), octets(0, 2), octets(1, 1), octets(0xA5, 33), octets(0xA5, 34), octets(0xA5, 40), octets(0xFF, 1)]
                [g.below(8)]
            .clone()
        }
        6 => {
            let hours = i64::try_from(n).unwrap().wrapping_mul(3_600_000_000_i64);
            Value::Datetime(Datetime::of_local(
                Moment::Timestamp,
                hours,
                i16::try_from(n.wrapping_rem(3_i128)).unwrap().wrapping_mul(60_i16),
                true,
                0_u32,
            ))
        }
        7 => Value::Interval(Interval {
            units: n.wrapping_mul(1_000_i128),
            qualifier: Qualifier { start: DatetimeField::Day, end: DatetimeField::Second, leading: 2_u8, fraction: 0_u8 },
        }),
        8 => Value::Bool(g.below(2) == 0),
        _ => Value::Char(format!("customer#{:05}", g.below(50))),
    }
}

/// `rows` as text, to compare.
fn shown(rows: &[Row]) -> String {
    format!("{rows:?}")
}

/// `rows` sorted by `keys` by comparing them whole, stably.
fn whole(keys: &[SortKey], rows: &[Row]) -> Vec<Row> {
    let mut sorted = rows.to_vec();
    sorted.sort_by(|a, b| compare_rows(keys, a, b));
    sorted
}

/// `count` rows of the kinds `kinds`, then their place, from `g`.
fn rows_of(kinds: &[usize], count: usize, g: &mut Seeded) -> Vec<Row> {
    (0..count)
        .map(|n| {
            let mut row: Row = kinds.iter().map(|&k| value(k, g)).collect();
            row.push(Value::Exact(Decimal::new(i128::try_from(n).unwrap(), 0_u32)));
            row
        })
        .collect()
}

/// `rows` as one batch of vectors of values.
fn batch_of(rows: &[Row]) -> Batch {
    let width = rows.first().map_or(0, Vec::len);
    Batch { columns: (0..width).map(|c| Column::Values(rows.iter().map(|r| r[c].clone()).collect())).collect(), rows: rows.len() }
}

/// The rows of `batches`, sorted by `keys` as a sort of batches sorts them,
/// by a layout of `layouts` if given; and whether keys were made.
fn by_places(keys: &[SortKey], batches: &[(Batch, Vec<usize>)], layouts: Option<&mut Layouts>) -> (Vec<Row>, bool) {
    let places: Vec<(u32, u32)> =
        batches.iter().zip(0_u32..).flat_map(|((_, rows), b)| rows.iter().map(move |&r| (b, u32::try_from(r).unwrap()))).collect();
    let (order, made) = sorted_places(keys, batches, &places, layouts, &crate::kernels::Portable);
    let rows = order
        .iter()
        .map(|&at| {
            let (b, r) = places[usize::try_from(at).unwrap()];
            let batch = &batches[usize::try_from(b).unwrap()].0;
            (0..batch.columns.len()).map(|c| batch.cell(c, usize::try_from(r).unwrap()).to_value()).collect()
        })
        .collect();
    (rows, made.is_some())
}

/// Verifies: [DESIGN] RULES.md D364 [X3.135-1986 §8.3 GR3b] [ISO/IEC
/// 9075-2:2023 §10.10 GR1e ii]: rows of random keys from a fixed seed,
/// sorted by their normal keys, rows and batches alike, are in the order
/// the comparator gives, stably, of every kind with a normal form and
/// nulls among them: exact numbers of one scale, of several, past 64
/// bits; approximate numbers, zeros of either sign, infinities; strings
/// that differ after 32 bytes, by trailing spaces, by a character below a
/// space, multibyte; binary strings of zeros, long and empty; timestamps
/// of zones; intervals; truth values; ASC and DESC under each null
/// ordering, alone and as one of three keys. Every kind has a layout.
#[test]
fn normal_keys_sort_as_the_comparator() {
    let mut g = Seeded(0x5EED_1234_ABCD_0001_u64);
    for kind in 0_usize..10_usize {
        for order in ORDERS {
            let rows = rows_of(&[kind, kind % 3_usize, 9_usize], 300_usize, &mut g);
            let one = [SortKey::new(0, order)];
            let three = [SortKey::new(1, SortOrder::of(!order.descending, None)), SortKey::new(0, order), SortKey::new(2, order)];
            for keys in [&one[..], &three[..]] {
                let expected = shown(&whole(keys, &rows));
                assert_eq!(shown(&sorted_rows(keys, rows.clone())), expected, "rows: kind {kind} {keys:?}");
                assert!(Layout::new(keys, &seen_rows(keys, &rows), Shape::Tight).is_some(), "kind {kind} has a layout");
                let halves = [(batch_of(&rows[..150]), (0..150).collect()), (batch_of(&rows[150..]), (0..150).collect())];
                let (made, keyed) = by_places(keys, &halves, None);
                assert!(keyed, "kind {kind} keyed");
                assert_eq!(shown(&made), expected, "batches: kind {kind} {keys:?}");
                let (open, _) = by_places(keys, &halves, Some(&mut Layouts::default()));
                assert_eq!(shown(&open), expected, "open: kind {kind} {keys:?}");
            }
        }
    }
}

/// A batch of six rows, two null: integers of `scale`, approximate
/// numbers, texts.
fn vectors(scale: u32) -> Batch {
    let nulls = || Some(vec![false, true, false, false, true, false].into());
    Batch {
        columns: vec![
            Column::Ints(IntVector::new(vec![5_i64, 0_i64, -3_i64, 12_i64, 0_i64, -3_i64], scale, nulls())),
            Column::Floats(Floats::new(vec![1.5_f64, 0.0_f64, -0.0_f64, 2.0_f64, 0.0_f64, -9.0_f64], nulls())),
            Column::Texts(TextVector::new("bbaaab c".to_owned(), vec![2_usize, 2_usize, 3_usize, 5_usize, 5_usize, 8_usize], nulls())),
        ],
        rows: 6,
    }
}

/// Verifies: [DESIGN] RULES.md D364: vectors of integers of two scales, of
/// approximate numbers, and of texts, with nulls, give the keys their
/// values give, rows of a selection alone sorted; an open layout made of
/// one run admits a later run of a lesser scale and not one of a greater,
/// which has a layout numbered apart;
/// a key of two kinds has no layout, and sorts as the comparator does.
#[test]
fn vectors_and_open_layouts_sort_as_the_comparator() {
    let held = vec![(vectors(1), vec![0_usize, 1, 2, 3, 4, 5]), (vectors(0), vec![5_usize, 3, 1])];
    let rows: Vec<Row> =
        held.iter().flat_map(|(b, rows)| rows.iter().map(|&r| (0..3).map(|c| b.cell(c, r).to_value()).collect::<Row>())).collect();
    for order in ORDERS {
        for keys in [
            vec![SortKey::new(0, order)],
            vec![SortKey::new(1, order), SortKey::new(0, order)],
            vec![SortKey::new(2, order), SortKey::new(0, order)],
        ] {
            let (made, keyed) = by_places(&keys, &held, None);
            assert!(keyed);
            assert_eq!(shown(&made), shown(&whole(&keys, &rows)), "{keys:?}");
        }
    }
    let keys = [SortKey::ascending(0)];
    let first = vectors(1);
    let open = Layout::new(&keys, &[seen_column(&first, 0, &[0, 1, 2])], Shape::Open).unwrap();
    assert!(open.admits(&[seen_column(&vectors(0), 0, &[0, 1])]), "a lesser scale");
    assert!(!open.admits(&[seen_column(&vectors(2), 0, &[0, 1])]), "a greater scale");
    let mut runs = Layouts::default();
    let mut number = |scale: u32| runs.of(&keys, &[seen_column(&vectors(scale), 0, &[0, 1])]).map(|(_, n)| n);
    assert_eq!(number(1), Some(1), "the first run's is shared");
    assert_eq!(number(0), Some(1), "a run it admits shares it");
    assert_eq!(number(2), Some(2), "one it does not has its own");
    let mixed = vec![vec![Value::Char("a".into())], vec![Value::Exact(Decimal::new(1_i128, 0_u32))], vec![Value::Null]];
    assert_eq!(shown(&sorted_rows(&keys, mixed.clone())), shown(&whole(&keys, &mixed)));
    let (made, keyed) = by_places(&keys, &[(batch_of(&mixed), vec![0, 1, 2])], None);
    assert!(!keyed, "two kinds have no layout");
    assert_eq!(shown(&made), shown(&whole(&keys, &mixed)));
}

/// The key of `row` under `layout`.
fn key_of(layout: &Layout, row: &Row) -> Vec<u8> {
    let mut key = vec![0_u8; layout.width];
    encode_row(layout, row, &mut key).unwrap();
    key
}

/// Verifies: [DESIGN] RULES.md D364 [X3.135-1986 §5.11 GR3] [ISO/IEC
/// 9075-2:2023 §10.10 GR1e ii]: a string's prefix is padded with spaces,
/// so 'a' and 'a ' have one key and 'a' is after 'a\t'; what is past the
/// prefix is compared with spaces; a null's byte is not inverted by DESC,
/// so a null is last under NULLS LAST either way; a tight integer of one
/// value takes no bytes, an open one 8 and a null byte.
#[test]
fn padding_null_bytes_and_widths() {
    let up = SortOrder::of(false, None);
    let field = Field { column: 0, order: up, code: Code::Text { prefix: 4 }, ints: Ints::Exact, nullable: false, at: 0 };
    let texts = Layout { fields: vec![field], width: 5, truncates: true };
    let at = |t: &str| key_of(&texts, &vec![Value::Char(t.to_owned())]);
    assert_eq!(at("a"), at("a "));
    assert!(at("a\t") < at("a"), "a tab is below the space padding");
    assert!(at("abcd\t") < at("abcd") && at("abcd") < at("abcde"), "past the prefix");
    let last = SortOrder { descending: true, nulls_first: false };
    let found = [Some(Found { seen: Seen::Exact { scale: 0, least: 1, most: 1 }, nulls: true })];
    let open = Layout::new(&[SortKey::new(0, last)], &found, Shape::Open).unwrap();
    assert!(key_of(&open, &vec![Value::Exact(Decimal::new(1_i128, 0_u32))]) < key_of(&open, &vec![Value::Null]), "NULLS LAST under DESC");
    assert_eq!(open.width, 9);
    let one = [Some(Found { seen: Seen::Exact { scale: 0, least: 300, most: 300 }, nulls: false })];
    assert_eq!(Layout::new(&[SortKey::new(0, up)], &one, Shape::Tight).unwrap().width, 0, "one value, no bytes");
}

/// Verifies: [DESIGN] RULES.md D364: a radix sort of keys of 8 and of 16
/// bytes, from a fixed seed, of few values and of many, gives the order a
/// stable sort by key gives, ties in their places' order; and so do keys
/// sorted by `order`, by radix sort and by comparison, enough of them and
/// too few for radix sort.
#[test]
fn radix_sorts_as_a_stable_sort() {
    let mut g = Seeded(0xABCD_EF01_2345_6789_u64);
    for spread in [3_u64, 1_u64 << 20_u32, u64::MAX] {
        let short: Vec<(u64, u32)> = (0_u32..5000).map(|p| (g.next().checked_rem(spread).unwrap(), p)).collect();
        let mut short_sorted = short.clone();
        short_sorted.sort_by_key(|&(k, _)| k);
        assert_eq!(radix::sort(short), short_sorted, "8 bytes, spread {spread}");
        let long: Vec<(u128, u32)> =
            (0_u32..5000).map(|p| ((u128::from(g.next().checked_rem(spread).unwrap()) << 64_u32) | 7_u128, p)).collect();
        let mut long_sorted = long.clone();
        long_sorted.sort_by_key(|&(k, _)| k);
        assert_eq!(radix::sort(long), long_sorted, "16 bytes, spread {spread}");
    }
    for (rows, width) in [(5000_usize, 3_usize), (5000, 12), (100, 3), (5000, 20)] {
        let keys: Vec<u8> = (0..rows.checked_mul(width).unwrap()).map(|_| u8::try_from(g.below(3)).unwrap()).collect();
        let places = order((&keys, rows), width, false, |_, _| std::cmp::Ordering::Equal).unwrap();
        let mut expected: Vec<u32> = (0_u32..).take(rows).collect();
        expected.sort_by_key(|&p| keys.chunks_exact(width).nth(usize::try_from(p).unwrap()).unwrap());
        assert_eq!(places, expected, "{rows} keys of {width} bytes");
    }
}
