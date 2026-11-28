//! Unit tests of the integers of chunks of 16-byte fields, an exact
//! number's of a type of more digits than 64 bits always hold (RULES.md
//! D1287).

use super::super::encoded::{CONSTANT, DICTIONARY, FIXED};
use super::super::*;
use super::int;
use crate::value::Spelled;

const WIDE: DataType = DataType::Exact { precision: 38, scale: 0, spelled: Spelled::Numeric };
/// 2 to the power 64, whose low 64 bits are 0.
const PAST: i128 = 1 << 64_u32;

/// The bytes of the chunk of all of `values`, compressed if `compress`.
fn chunk_of(values: &[Value], compress: bool) -> Vec<u8> {
    let fields = Fields::of(&WIDE, values.iter().cloned()).unwrap();
    fields.chunk(&WIDE, 0, (fields.len(), compress)).unwrap()
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
