//! Unit tests of column chunks (`columns.rs`).

use super::encoded::{CONSTANT, DICTIONARY, FIXED, FRAME, RUNS, VARYING};
use super::*;
use crate::value::{Decimal, Spelled};

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

const INTEGER: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
const TEXT: DataType = DataType::Varying { length: 100 };

/// The values of the chunk of all of `values`, read back.
fn round_trip(t: &DataType, values: &[Value]) -> Vec<String> {
    let fields = Fields::of(t, values.iter().cloned()).unwrap();
    let bytes = fields.chunk(t, 0, (fields.len(), false)).unwrap();
    let chunk = Chunk::parse(&bytes).unwrap();
    assert_eq!(chunk.len(), values.len());
    (0..chunk.len()).map(|n| chunk.value(n, t).unwrap().to_string()).collect()
}

/// Verifies: [DESIGN] RULES.md D31, column chunks: values of a type of a
/// fixed width in a record, and of one of variable width, read back as
/// written, nulls among them, each found alone; nine values, so the nulls
/// take a second byte.
#[test]
fn chunks_read_back() {
    let numbers: Vec<Value> = (0..9).map(|n| if n % 4 == 1 { Value::Null } else { int(n * 1000 - 3) }).collect();
    let expected: Vec<String> = numbers.iter().map(ToString::to_string).collect();
    assert_eq!(round_trip(&INTEGER, &numbers), expected);
    let texts: Vec<Value> = ["", "a", "bc", "", "def"].iter().map(|s| Value::Char((*s).to_owned())).chain([Value::Null]).collect();
    assert_eq!(round_trip(&TEXT, &texts), ["", "a", "bc", "", "def", "NULL"]);
    let fields = Fields::of(&TEXT, texts.into_iter()).unwrap();
    let bytes = fields.chunk(&TEXT, 0, (fields.len(), false)).unwrap();
    assert!(Chunk::parse(&bytes).unwrap().value(5, &TEXT).unwrap().is_null(), "a null is null, not empty");
    assert_eq!(bytes.first(), Some(&VARYING));
    let fixed = Fields::of(&INTEGER, [int(1)].into_iter()).unwrap().chunk(&INTEGER, 0, (1, false)).unwrap();
    assert_eq!(fixed.first(), Some(&FIXED));
}

/// Verifies: [DESIGN] RULES.md D31, column chunks: as many values fit a
/// chunk as its capacity holds, from where asked, and at least one, so a
/// value longer than a chunk has one of its own; a chunk of a part of the
/// values holds that part.
#[test]
fn values_fit_their_chunks() {
    let fields = Fields::of(&INTEGER, (0..100).map(int)).unwrap();
    let width = INTEGER.kind().record_width(&INTEGER).unwrap();
    let capacity = HEADER + WORD + 2 + 10 * width;
    assert_eq!(fields.fitting(&INTEGER, 0, (capacity, false)), 10);
    assert_eq!(fields.fitting(&INTEGER, 95, (capacity, false)), 5, "the rest");
    let chunk = fields.chunk(&INTEGER, 20, (10, false)).unwrap();
    assert!(chunk.len() <= capacity);
    assert_eq!(Chunk::parse(&chunk).unwrap().value(0, &INTEGER).unwrap().to_string(), "20");
    let long = Fields::of(&TEXT, [Value::Char("x".repeat(90)), Value::Char("y".into())].into_iter()).unwrap();
    assert_eq!(long.fitting(&TEXT, 0, (16, false)), 1, "at least one");
    assert!(!long.is_empty());
}

/// Verifies: [DESIGN] RULES.md D31, column chunks: bytes that are not a
/// chunk are refused with SQLCODE -916: empty, of another kind, too short
/// for their count, nulls or values; and a value past the end is.
#[test]
fn malformed_chunks_are_refused() {
    let code = |bytes: &[u8]| Chunk::parse(bytes).err().map(|e| e.code);
    assert_eq!(code(b""), Some(Code::Corrupt));
    assert_eq!(code(&[9, 1, 0, 0, 0, 0]), Some(Code::Corrupt), "another kind");
    assert_eq!(code(&[FIXED, 1, 0]), Some(Code::Corrupt), "no count");
    assert_eq!(code(&[FIXED, 9, 0, 0, 0]), Some(Code::Corrupt), "no nulls");
    assert_eq!(code(&[FIXED, 1, 0, 0, 0, 0, 8, 0, 0, 0, 1]), Some(Code::Corrupt), "a value too short");
    assert_eq!(code(&[VARYING, 1, 0, 0, 0, 0, 0, 0]), Some(Code::Corrupt), "offsets too short");
    assert_eq!(code(&[DICTIONARY, 1, 0, 0, 0, 0, 17, 1, 0, 0, 0]), Some(Code::Corrupt), "codes wider than 16 bits");
    let bytes = Fields::of(&INTEGER, [int(1)].into_iter()).unwrap().chunk(&INTEGER, 0, (1, false)).unwrap();
    let chunk = Chunk::parse(&bytes).unwrap();
    assert!(!chunk.is_empty());
    assert_eq!(chunk.value(1, &INTEGER).err().map(|e| e.code), Some(Code::Corrupt));
}

/// The kind and length of the chunk of all of `values`, compressed, and the
/// values read back.
fn compressed(t: &DataType, values: &[Value]) -> (u8, usize, Vec<String>) {
    let fields = Fields::of(t, values.iter().cloned()).unwrap();
    let bytes = fields.chunk(t, 0, (fields.len(), true)).unwrap();
    let chunk = Chunk::parse(&bytes).unwrap();
    let back = (0..chunk.len()).map(|n| chunk.value(n, t).unwrap().to_string()).collect();
    (bytes[0], bytes.len(), back)
}

/// Verifies: [DESIGN] RULES.md D31, compressed chunks: the values a chunk
/// holds are written in the encoding that takes fewest bytes, and read
/// back as written, nulls among them: one value throughout, a constant;
/// numbers near one another, a frame of reference, each in the bits their
/// span needs; few texts, a dictionary, each code in the bits its
/// entries need, of more than 256 entries too; values every one apart, and
/// numbers whose span needs 64 bits, as they are; and more values fit a
/// page compressed.
#[test]
fn chunks_are_compressed() {
    let check = |t: &DataType, values: Vec<Value>, kind: u8| {
        let (found, len, back) = compressed(t, &values);
        assert_eq!(found, kind, "{values:?}");
        assert_eq!(back, values.iter().map(ToString::to_string).collect::<Vec<_>>());
        len
    };
    let same: Vec<Value> = (0..100_i128).map(|n| if n % 9 == 0_i128 { Value::Null } else { int(42) }).collect();
    assert!(check(&INTEGER, same, CONSTANT) < 40);
    let near: Vec<Value> = (0..100).map(|n| int(1_000_000 + n * 2)).chain([Value::Null]).collect();
    assert!(check(&INTEGER, near, FRAME) < 140, "a byte each");
    let wider: Vec<Value> = (0..100).map(|n| int(-5000 + n * 100)).collect();
    check(&INTEGER, wider, FRAME);
    let few: Vec<Value> = (0..300).map(|n| Value::Char(["red", "green", "blue"][n % 3].to_owned())).collect();
    assert!(check(&TEXT, few, DICTIONARY) < 160, "two bits each");
    let hundreds: Vec<Value> = (0..3000_u32).map(|n| Value::Char(format!("v{}", n.checked_rem(300).unwrap()))).collect();
    // 3,375 bytes of codes, 300 entries' offsets and texts.
    assert!(check(&TEXT, hundreds, DICTIONARY) < 3375 + 300 * 9, "nine bits each");
    let apart: Vec<Value> = (0..50_u32).map(|n| Value::Char(format!("value {n}"))).collect();
    check(&TEXT, apart, VARYING);
    let spread: Vec<Value> = (0..50).map(|n| int(n * 10_000_000_000)).collect();
    assert!(check(&INTEGER, spread, FRAME) < 50 * 5 + 40, "39 bits each");
    let ends: Vec<Value> =
        (0..50_i128).map(|n| int(if n % 2 == 0 { -9_000_000_000_000_000_000 - n } else { 9_000_000_000_000_000_000 + n })).collect();
    check(&INTEGER, ends, FIXED);
    let many = Fields::of(&INTEGER, (0..10_000).map(|n| int(n % 200))).unwrap();
    assert!(many.fitting(&INTEGER, 0, (4096, true)) > 3 * many.fitting(&INTEGER, 0, (4096, false)), "more fit compressed");
}

/// The integers a chunk of `values`, compressed if `compress`, gives of
/// its positions `from` to `to`, and its values there as they read one at
/// a time, if it holds integers.
fn ints_of(values: &[Value], compress: bool, (from, to): (usize, usize)) -> Option<(Vec<String>, Vec<String>)> {
    let fields = Fields::of(&INTEGER, values.iter().cloned()).unwrap();
    let bytes = fields.chunk(&INTEGER, 0, (fields.len(), compress)).unwrap();
    let chunk = Chunk::parse(&bytes).unwrap();
    let (mut units, mut nulls) = (Vec::new(), Vec::new());
    chunk.ints((from, to), (&mut units, &mut nulls))?;
    let made = units.iter().zip(&nulls).map(|(u, &null)| if null { "NULL".to_owned() } else { u.to_string() }).collect();
    let read = (from..to).map(|n| chunk.value(n, &INTEGER).unwrap().to_string()).collect();
    Some((made, read))
}

/// Verifies: [DESIGN] RULES.md D31, a chunk's integers: every encoding of
/// integers gives, of a range of its positions, the values read one at a
/// time give, nulls as nulls: a frame of reference whose differences take
/// 1, 2, 3, 4, 5 and 8 bytes, of negative numbers too; fixed fields; a
/// constant, and one of nulls only. A dictionary gives none, nor a range
/// past the chunk's end.
#[test]
fn chunks_give_their_integers() {
    let with_nulls = |values: Vec<i128>| -> Vec<Value> {
        values.into_iter().enumerate().map(|(n, v)| if n % 7 == 3 { Value::Null } else { int(v) }).collect()
    };
    let spans: [i128; 6] = [200, 60_000, 16_000_000, 4_000_000_000, 1_000_000_000_000, 9_000_000_000_000_000_000];
    for span in spans {
        let (step, half) = (span.checked_div(40).unwrap(), span.checked_div(2).unwrap());
        let values = with_nulls((0..40_i128).map(|n| (n * 37 % 40) * step - half).collect());
        let (made, read) = ints_of(&values, true, (0, 40)).unwrap();
        assert_eq!(made, read, "a frame of span {span}");
        let (part, read_part) = ints_of(&values, true, (5, 17)).unwrap();
        assert_eq!(part, read_part, "a part of span {span}");
    }
    let fixed = with_nulls((0..20_i128).map(|n| n * 1_000_000_000_007 - 3).collect());
    let fixed_read = ints_of(&fixed, false, (0, 20)).unwrap();
    assert_eq!(fixed_read.0, fixed_read.1, "fixed fields");
    let constant = with_nulls(vec![42; 30]);
    let constant_read = ints_of(&constant, true, (0, 30)).unwrap();
    assert_eq!(constant_read.0, constant_read.1, "a constant");
    let nulls = vec![Value::Null; 12];
    let nulls_read = ints_of(&nulls, true, (0, 12)).unwrap();
    assert_eq!(nulls_read.0, nulls_read.1, "nulls only");
    let few = with_nulls((0..300_i128).map(|n| [5, 900_000_000_000, -7][usize::try_from(n % 3).unwrap()]).collect());
    assert_eq!(ints_of(&few, true, (0, 300)), None, "a dictionary");
    assert_eq!(ints_of(&fixed, false, (0, 21)), None, "past the end");
}

/// The texts a chunk of `values` of `t`, compressed, gives of its positions
/// `from` to `to`, and its values there as they read one at a time, if it
/// holds text.
fn texts_of(t: &DataType, values: &[Value], (from, to): (usize, usize)) -> Option<(Vec<String>, Vec<String>)> {
    let fields = Fields::of(t, values.iter().cloned()).unwrap();
    let bytes = fields.chunk(t, 0, (fields.len(), true)).unwrap();
    let chunk = Chunk::parse(&bytes).unwrap();
    let (mut text, mut ends, mut nulls) = (String::new(), Vec::new(), Vec::new());
    chunk.texts((from, to), (&mut text, &mut ends, &mut nulls))?;
    let texts = crate::storage::Texts::new(text, ends, nulls.contains(&true).then_some(nulls));
    let made = (0..texts.len()).map(|n| texts.get(n).map_or_else(|| "NULL".to_owned(), str::to_owned)).collect();
    let read = (from..to).map(|n| chunk.value(n, t).unwrap().to_string()).collect();
    Some((made, read))
}

/// Verifies: [DESIGN] RULES.md D31, a chunk's texts: every encoding of
/// strings gives, of a range of its positions, the values read one at a
/// time give, nulls as nulls: of varying fields, some not ASCII, some
/// empty, some with a trailing space; a dictionary; a constant. Of
/// integers it gives none, nor past its end.
#[test]
fn chunks_give_their_texts() {
    let varying: Vec<Value> = (0..30_u32)
        .map(|n| {
            if n % 7 == 2 {
                Value::Null
            } else {
                Value::Char(
                    ["", "é", "a ", "text"][usize::try_from(n % 4).unwrap()].repeat(usize::try_from(n % 3 + 1).unwrap()) + &n.to_string(),
                )
            }
        })
        .collect();
    let few: Vec<Value> =
        (0..300).map(|n| if n % 11 == 0 { Value::Null } else { Value::Char(["red", "grün", "blue"][n % 3].to_owned()) }).collect();
    let constant: Vec<Value> = (0_u32..20_u32).map(|n| if n % 5 == 0 { Value::Null } else { Value::Char("same".to_owned()) }).collect();
    for (values, kind) in [(&varying, "varying"), (&few, "a dictionary"), (&constant, "a constant")] {
        let whole = texts_of(&TEXT, values, (0, values.len())).unwrap();
        assert_eq!(whole.0, whole.1, "{kind}");
        let part = texts_of(&TEXT, values, (3, 11)).unwrap();
        assert_eq!(part.0, part.1, "{kind}, a part");
    }
    let numbers: Vec<Value> = (0_i128..10_i128).map(int).collect();
    assert_eq!(texts_of(&INTEGER, &numbers, (0, 10)), None, "integers");
    assert_eq!(texts_of(&TEXT, &constant, (0, 21)), None, "past the end");
}

/// The texts a chunk of `values`, compressed, gives of `positions` as a
/// dictionary's codes, and its values there as they read one at a time,
/// if it is a dictionary.
fn coded_of(values: &[Value], positions: &[usize]) -> Option<(Vec<String>, Vec<String>)> {
    let fields = Fields::of(&TEXT, values.iter().cloned()).unwrap();
    let bytes = fields.chunk(&TEXT, 0, (fields.len(), true)).unwrap();
    let chunk = Chunk::parse(&bytes).unwrap();
    let (mut text, mut ends, mut codes, mut nulls) = (String::new(), Vec::new(), Vec::new(), Vec::new());
    chunk.coded(positions, (&mut text, &mut ends, &mut codes, &mut nulls))?;
    let dictionary = crate::storage::Coded { codes, entries: crate::storage::Texts::new(text, ends, None) };
    let texts = crate::storage::Texts::coded(dictionary, nulls.contains(&true).then_some(nulls));
    let made = (0..texts.len()).map(|n| texts.get(n).map_or_else(|| "NULL".to_owned(), str::to_owned)).collect();
    let read = positions.iter().map(|&n| chunk.value(n, &TEXT).unwrap().to_string()).collect();
    Some((made, read))
}

/// Verifies: [DESIGN] RULES.md D31, a dictionary's codes: a chunk of few
/// strings, some not ASCII, nulls among them, gives of any of its
/// positions, all or some, its entries and each one's entry, which read
/// as the values one at a time; a chunk not a dictionary gives none, nor
/// a position past its end.
#[test]
fn chunks_give_their_codes() {
    let few: Vec<Value> = (0_usize..300_usize)
        .map(|n| if n % 11 == 0 { Value::Null } else { Value::Char(["red", "grün", "blue"][n % 3].to_owned()) })
        .collect();
    let all: Vec<usize> = (0..300).collect();
    let (made, read) = coded_of(&few, &all).unwrap();
    assert_eq!(made, read, "all");
    let some = [0_usize, 7, 11, 150, 299];
    let (made_some, read_some) = coded_of(&few, &some).unwrap();
    assert_eq!(made_some, read_some, "some");
    let varying: Vec<Value> = (0_u32..30_u32).map(|n| Value::Char(format!("value {n}"))).collect();
    assert_eq!(coded_of(&varying, &[0, 1]), None, "varying fields");
    assert_eq!(coded_of(&few, &[300]), None, "past the end");
}

/// Verifies: [DESIGN] RULES.md D31, runs of one value: a sorted column of
/// few values, a null leading and nulls among them, is written as runs,
/// in fewer bytes than a frame of reference, and reads back as written,
/// one at a time and of any range of its positions; a run's bytes cut
/// short, or of differences wider than 64 bits, are refused with SQLCODE
/// -916.
#[test]
fn sorted_chunks_are_runs() {
    let sorted: Vec<Value> = [Value::Null]
        .into_iter()
        .chain((0..2000_i128).map(|n| if n % 97 == 5 { Value::Null } else { int(n.checked_div(250).unwrap() * 1000 - 3000) }))
        .collect();
    let (kind, len, back) = compressed(&INTEGER, &sorted);
    assert_eq!(kind, RUNS);
    assert!(len < 400, "8 runs and nulls, not 2,001 values: {len}");
    assert_eq!(back, sorted.iter().map(ToString::to_string).collect::<Vec<_>>());
    for range in [(0, 2001), (0, 1), (249, 252), (1700, 2001), (7, 7)] {
        let (made, read) = ints_of(&sorted, true, range).unwrap();
        assert_eq!(made, read, "{range:?}");
    }
    assert_eq!(ints_of(&sorted, true, (0, 2002)), None, "past the end");
    let fields = Fields::of(&INTEGER, sorted.iter().cloned()).unwrap();
    let bytes = fields.chunk(&INTEGER, 0, (fields.len(), true)).unwrap();
    let code = |b: &[u8]| Chunk::parse(b).err().map(|e| e.code);
    assert_eq!(code(&bytes[..bytes.len().saturating_sub(1)]), Some(Code::Corrupt), "cut short");
    let at = HEADER.saturating_add(nulls_len(fields.len())).saturating_add(WORD).saturating_add(8);
    let mut wide = bytes.clone();
    wide[at] = 65;
    assert_eq!(code(&wide), Some(Code::Corrupt), "wider than 64 bits");
}
