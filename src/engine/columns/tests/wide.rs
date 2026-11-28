//! Unit tests of the integers of chunks of 16-byte fields, an exact
//! number's of a type of more digits than 64 bits always hold (RULES.md
//! D1287), and of such a column's fields written in 8 bytes if 64 bits
//! hold every value (RULES.md D1285).

use std::borrow::Cow;

use super::super::encoded::{CONSTANT, DICTIONARY, FIXED, FRAME, RUNS};
use super::super::*;
use super::{int, records};
use crate::value::Spelled;

const WIDE: DataType = DataType::Exact { precision: 38, scale: 0, spelled: Spelled::Numeric };
/// 2 to the power 64, whose low 64 bits are 0.
const PAST: i128 = 1 << 64_u32;

/// The bytes of the chunk of all of `values`, compressed if `compress`,
/// of a column with a value past 64 bits after them, so its fields keep
/// 16 bytes (RULES.md D1285).
fn chunk_of(values: &[Value], compress: bool) -> Vec<u8> {
    let fields = Fields::of(&WIDE, values.iter().cloned().chain([int(PAST)])).unwrap();
    fields.chunk(&WIDE, 0, (values.len(), compress)).unwrap()
}

/// The integers the chunk `bytes` gives of its positions `from` to `to`,
/// a null shown as such, if it holds integers.
fn ints_of(bytes: &[u8], (from, to): (usize, usize)) -> Option<Vec<String>> {
    let chunk = Chunk::parse(bytes).unwrap();
    let (mut units, mut nulls) = (Vec::new(), Mask::default());
    chunk.ints((from, to), (&mut units, &mut nulls))?;
    Some(units.iter().zip(&nulls.to_bools()).map(|(u, &null)| if null { "NULL".to_owned() } else { u.to_string() }).collect())
}

/// The values of the chunk `bytes` at positions `from` to `to`, read one
/// at a time.
fn values_of(bytes: &[u8], (from, to): (usize, usize)) -> Vec<String> {
    let chunk = Chunk::parse(bytes).unwrap();
    (from..to).map(|n| chunk.value(n, &WIDE).unwrap().to_string()).collect()
}

/// Each value of the chunk `bytes` not null read alone as an integer
/// (`int_at`), if every one is an integer.
fn alone(bytes: &[u8], values: &[Value]) -> Option<Vec<String>> {
    let chunk = Chunk::parse(bytes).unwrap();
    values.iter().enumerate().filter(|(_, v)| !v.is_null()).map(|(n, _)| chunk.body.int_at(n, None).map(|u| u.to_string())).collect()
}

fn with_nulls(values: Vec<i128>) -> Vec<Value> {
    values.into_iter().enumerate().map(|(n, v)| if n % 7 == 3 { Value::Null } else { int(v) }).collect()
}

fn shown(values: &[Value]) -> Vec<String> {
    values.iter().filter(|v| !v.is_null()).map(ToString::to_string).collect()
}

/// Verifies: [DESIGN] RULES.md D1287, a chunk of 16-byte fields read as
/// integers: fixed fields, a constant, a constant of nulls only and a
/// dictionary, each of values 64 bits hold, the least and the greatest of
/// them among them, give of a range of positions, whole and in part, and
/// of each position alone, the values read one at a time, nulls as nulls;
/// and a constant gives where its run ends.
#[test]
fn chunks_of_wide_fields_give_their_integers() {
    let (least, most) = (i128::from(i64::MIN), i128::from(i64::MAX));
    let spread =
        with_nulls((0..40_i128).map(|n| [least, most, -1, 0, 9_000_000_000_000_000_000 - n][usize::try_from(n % 5).unwrap()]).collect());
    let fixed = chunk_of(&spread, false);
    assert_eq!((fixed[0], fixed.len() > 40 * 16), (FIXED, true), "fields of 16 bytes");
    let few = with_nulls((0..300_i128).map(|n| [least, most, 5][usize::try_from(n % 3).unwrap()]).collect());
    let dictionary = chunk_of(&few, true);
    assert_eq!(dictionary[0], DICTIONARY);
    let same = with_nulls(vec![most; 30]);
    let constant = chunk_of(&same, true);
    assert_eq!(constant[0], CONSTANT);
    let nulls = chunk_of(&vec![Value::Null; 12], true);
    for (bytes, values, what) in [(&fixed, &spread, "fixed"), (&dictionary, &few, "a dictionary"), (&constant, &same, "a constant")] {
        let len = values.len();
        assert_eq!(ints_of(bytes, (0, len)), Some(values_of(bytes, (0, len))), "{what}");
        assert_eq!(ints_of(bytes, (5, 27)), Some(values_of(bytes, (5, 27))), "{what}, a part");
        assert_eq!(alone(bytes, values), Some(shown(values)), "{what}, each alone");
    }
    assert_eq!(ints_of(&nulls, (0, 12)), Some(vec!["NULL".to_owned(); 12]), "nulls only");
    assert_eq!(Chunk::parse(&nulls).unwrap().body.int_at(3, None), Some(0), "a null's is of no meaning");
    let (mut units, mut marks, mut ends) = (Vec::new(), Mask::default(), Vec::new());
    assert_eq!(Chunk::parse(&constant).unwrap().ints_ending((2, 9), (&mut units, &mut marks), (&mut ends, 100)), Some(true));
    assert_eq!((units, ends), (vec![i64::MAX; 7], vec![107_usize]), "one run");
}

/// Verifies: [DESIGN] RULES.md D1287, what a chunk of 16-byte fields
/// refuses: one value past 64 bits, the first past the greatest, the first
/// below the least, and one whose low 64 bits are 0 or a small number,
/// keeps fixed fields, a dictionary and a constant from being read as
/// integers, in a range, alone and as a run, so none is cut to 64 bits;
/// the values still read one at a time. MC/DC of the fit, the high half
/// being the low half's sign: a value 64 bits hold (read) against each of
/// these (refused).
#[test]
fn a_value_past_64_bits_is_not_read_as_an_integer() {
    let (least, most) = (i128::from(i64::MIN), i128::from(i64::MAX));
    for past in [most + 1, least - 1, PAST, PAST + 5, -PAST, 10_i128.pow(37)] {
        let mut spread: Vec<Value> = (0..40_i128).map(|n| int(n * 1_000_000_007)).collect();
        spread[17] = int(past);
        let fixed = chunk_of(&spread, false);
        assert_eq!(ints_of(&fixed, (0, 40)), None, "fixed, {past}");
        assert_eq!(ints_of(&fixed, (17, 18)), None, "fixed, the one value, {past}");
        assert_eq!(ints_of(&fixed, (0, 17)), Some(values_of(&fixed, (0, 17))), "the fields before it are integers");
        assert_eq!(Chunk::parse(&fixed).unwrap().body.int_at(17, None), None, "fixed, alone, {past}");
        assert_eq!(values_of(&fixed, (17, 18)), vec![past.to_string()]);
        let few: Vec<Value> = (0..300_i128).map(|n| int([past, 7, 5][usize::try_from(n % 3).unwrap()])).collect();
        let dictionary = chunk_of(&few, true);
        assert_eq!(dictionary[0], DICTIONARY);
        assert_eq!(ints_of(&dictionary, (1, 3)), None, "a dictionary, {past}");
        assert_eq!(Chunk::parse(&dictionary).unwrap().body.int_at(1, None), None, "a dictionary, alone, {past}");
        let constant = chunk_of(&vec![int(past); 30], true);
        assert_eq!(constant[0], CONSTANT);
        assert_eq!(ints_of(&constant, (0, 30)), None, "a constant, {past}");
        assert_eq!(Chunk::parse(&constant).unwrap().body.int_at(4, None), None, "a constant, alone, {past}");
        let (mut units, mut marks, mut ends) = (Vec::new(), Mask::default(), Vec::new());
        assert_eq!(Chunk::parse(&constant).unwrap().ints_ending((0, 30), (&mut units, &mut marks), (&mut ends, 0)), None, "a run, {past}");
        assert_eq!(values_of(&constant, (29, 30)), vec![past.to_string()]);
    }
}

/// The bytes of the records of `fields` that are not null.
fn widths(fields: &Fields<'_>) -> Vec<usize> {
    (0..fields.len()).filter_map(|n| fields.records().record(n).map(<[u8]>::len)).collect()
}

/// Verifies: [DESIGN] RULES.md D1285, a wide exact column's fields
/// narrowed: of values 64 bits hold, nulls among them, each is written in
/// 8 bytes, of rows and of a load's records of 16 bytes alike, so its
/// chunk is a frame of reference, runs, a dictionary, a constant or fixed
/// fields of 8 bytes as a narrow column's is, shorter than fields of 16
/// bytes, and reads back the values written, one at a time and as
/// integers; the least and the greatest 64 bits hold are narrowed too.
#[test]
fn a_wide_column_of_values_64_bits_hold_is_written_in_8_bytes() {
    let (least, most) = (i128::from(i64::MIN), i128::from(i64::MAX));
    let near = with_nulls((0..200_i128).map(|n| 9_000_000_000_000_000_000 + n * 37 % 200).collect());
    let ran: Vec<Value> =
        (0..2000_i128).map(|n| if n % 97 == 5 { Value::Null } else { int(most - n.checked_div(250).unwrap() * 1000) }).collect();
    let few = with_nulls((0..300_i128).map(|n| [least, most, 5][usize::try_from(n % 3).unwrap()]).collect());
    let same = with_nulls(vec![most; 30]);
    let ends: Vec<Value> = (0..50_i128).map(|n| int(if n % 2 == 0 { least + n } else { most - n })).collect();
    for (values, kind, what) in [
        (&near, FRAME, "a frame"),
        (&ran, RUNS, "runs"),
        (&few, DICTIONARY, "a dictionary"),
        (&same, CONSTANT, "a constant"),
        (&ends, FIXED, "fixed"),
    ] {
        let fields = Fields::of(&WIDE, values.iter().cloned()).unwrap();
        assert!(widths(&fields).iter().all(|&w| w == 8) && widths(&fields).len() == shown(values).len(), "{what}: 8 bytes each");
        let loaded = records(&WIDE, values);
        assert_eq!(loaded.bytes.len(), 16 * shown(values).len(), "{what}: a load's records are of 16 bytes");
        assert_eq!(Fields::from_records(&WIDE, Cow::Borrowed(&loaded)).records(), fields.records(), "{what}: of records as of rows");
        let (len, bytes) = (values.len(), fields.chunk(&WIDE, 0, (values.len(), true)).unwrap());
        assert_eq!(bytes[0], kind, "{what}");
        assert!(bytes.len() < 9 * len + 20, "{what}: {} bytes", bytes.len());
        assert_eq!(values_of(&bytes, (0, len)), values.iter().map(ToString::to_string).collect::<Vec<_>>(), "{what} reads back");
        assert_eq!(ints_of(&bytes, (0, len)), Some(values_of(&bytes, (0, len))), "{what} as integers");
    }
}

/// Verifies: [DESIGN] RULES.md D1285, what is not narrowed: one value past
/// 64 bits, the first above the greatest or the first below the least,
/// keeps every field of its column in 16 bytes, of rows and of records,
/// read back as written; and a column of a type 64 bits always hold is
/// taken as it is. MC/DC of the narrowing: a wide type whose values all
/// fit (narrowed, the test above) against a wide type with one value past
/// 64 bits, and against a narrow type (each left as it is).
#[test]
fn a_column_with_a_value_past_64_bits_keeps_its_16_bytes() {
    let (least, most) = (i128::from(i64::MIN), i128::from(i64::MAX));
    for past in [most + 1, least - 1, PAST] {
        let mut values = with_nulls((0..60_i128).map(|n| n * 1_000_000_007).collect());
        values[41] = int(past);
        let fields = Fields::of(&WIDE, values.iter().cloned()).unwrap();
        assert!(widths(&fields).iter().all(|&w| w == 16), "{past}: 16 bytes each");
        let loaded = records(&WIDE, &values);
        assert_eq!(Fields::from_records(&WIDE, Cow::Borrowed(&loaded)).records(), &loaded, "{past}: a load's records as they are");
        let bytes = fields.chunk(&WIDE, 0, (60, true)).unwrap();
        assert_eq!(bytes[0], FIXED, "{past}");
        assert_eq!(values_of(&bytes, (0, 60)), values.iter().map(ToString::to_string).collect::<Vec<_>>(), "{past} reads back");
    }
    let narrow = DataType::Exact { precision: 18, scale: 0, spelled: Spelled::Numeric };
    let values = with_nulls((0..60_i128).map(|n| n * 1_000_000_007).collect());
    let loaded = records(&narrow, &values);
    assert_eq!(Fields::from_records(&narrow, Cow::Borrowed(&loaded)).records(), &loaded, "a narrow column as it is");
}
