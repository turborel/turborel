//! Unit tests of a chunk's octet lengths read with no text made
//! (`lengths.rs`, RULES.md D1267).

use super::super::super::Fields;
use super::super::{CONSTANT, DICTIONARY, FIXED, FSST, HEADER, VARYING};
use super::*;
use crate::value::{DataType, Decimal, Spelled, Value};

const INTEGER: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
const TEXT: DataType = DataType::varying(200);

/// The chunk of `values`, compressed if `compress`, of the kind `kind`.
fn chunk_of(values: &[Value], compress: bool, kind: u8) -> Vec<u8> {
    let fields = Fields::of(&TEXT, values.iter().cloned()).unwrap();
    let bytes = fields.chunk(&TEXT, 0, (values.len(), compress)).unwrap();
    assert_eq!(bytes.first(), Some(&kind));
    bytes
}

/// The octet length of each of `values` at `places`, and whether it is
/// null, as its text gives them.
fn wanted(values: &[Value], places: &[usize]) -> (Vec<i64>, Vec<bool>) {
    let length = |n: &usize| if let Value::Char(s) = &values[*n] { i64::try_from(s.len()).unwrap() } else { 0 };
    (places.iter().map(length).collect(), places.iter().map(|&n| values[n].is_null()).collect())
}

/// What `Chunk::octets` gives of `bytes` at `places`, after one unit
/// and one null already held.
fn octets(bytes: &[u8], places: &[usize]) -> Option<(Vec<i64>, Vec<bool>)> {
    let (mut units, mut nulls) = (vec![-1], Mask::from_bools(&[false]));
    Chunk::parse(bytes).unwrap().octets(places, (&mut units, &mut nulls))?;
    assert_eq!(units[0], -1, "what was held stays");
    Some((units[1..].to_vec(), (1..units.len()).map(|n| nulls.get(n)).collect()))
}

/// One of a few words a row, of one octet and of many a character, the
/// empty string among them, every seventh null.
fn word(n: usize) -> Value {
    if n % 7 == 6 {
        return Value::Null;
    }
    Value::Char(["a", "", "h\u{e9}llo", "\u{6f22}\u{5b57}", "green ", "a"][n % 6].into())
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR5] RULES.md D1267: the octet
/// length read of a varying, a constant and a dictionary chunk and of
/// texts coded by symbols is the length of each value's text in octets,
/// a character of many octets counted as many and the empty string 0; a
/// null gives a null and 0; of every row, of every third, and of two
/// rows far apart, which a dictionary reads each by its own entry. MC/DC
/// of a dictionary's codes: places dense in their span against sparse.
#[test]
fn each_encoding_gives_its_values_octet_lengths() {
    let words: Vec<Value> = (0..200).map(word).collect();
    let same: Vec<Value> = (0..200_usize).map(|n| if n % 7 == 6 { Value::Null } else { Value::Char("h\u{e9}llo".into()) }).collect();
    let alike: Vec<Value> = (0..200_usize)
        .map(|n| {
            if n % 7 == 6 { Value::Null } else { Value::Char(format!("http://site{}.example/p\u{e9}ge{}", n % 30, n * 7919 % 5000).into()) }
        })
        .collect();
    let chunks = [
        ("varying", chunk_of(&words, false, VARYING), &words),
        ("a dictionary", chunk_of(&words, true, DICTIONARY), &words),
        ("a constant", chunk_of(&same, true, CONSTANT), &same),
        ("coded by symbols", chunk_of(&alike, true, FSST), &alike),
    ];
    let asked: [Vec<usize>; 4] = [(0..200).collect(), (0..200).step_by(3).collect(), vec![3, 190], Vec::new()];
    for (what, bytes, values) in &chunks {
        for places in &asked {
            assert_eq!(octets(bytes, places), Some(wanted(values, places)), "{what}, {} places", places.len());
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1267, what gives no length: a chunk of
/// fixed fields, as integers are kept; a place past a chunk's end; a
/// varying chunk and a dictionary one of whose ends lies past the
/// octets held; and texts coded by symbols whose codes name no symbol.
/// Robustness: each gives none, never a length guessed.
#[test]
fn a_chunk_that_cannot_be_measured_gives_none() {
    let numbers = Fields::of(&INTEGER, (0..9).map(|n| Value::Exact(Decimal::new(n, 0)))).unwrap().chunk(&INTEGER, 0, (9, false)).unwrap();
    assert_eq!(numbers.first(), Some(&FIXED));
    assert_eq!(octets(&numbers, &[0, 1]), None, "fixed fields");
    let words: Vec<Value> = (0..40).map(word).collect();
    let all: Vec<usize> = (0..40).collect();
    let varying = chunk_of(&words, false, VARYING);
    assert_eq!(octets(&varying, &[39, 40]), None, "varying: past its end");
    // Its last octets cut off, so its last values end past what it holds.
    assert_eq!(octets(&varying[..varying.len() - 4], &all), None, "varying: cut short");
    let mut dictionary = chunk_of(&words, true, DICTIONARY);
    assert_eq!(octets(&dictionary, &[39, 40]), None, "a dictionary: past its end");
    assert!(octets(&dictionary, &all).is_some());
    // Its first entry's end, the second word of its ends, set past its entries.
    let ends = HEADER + super::super::super::nulls_len(40) + 1 + WORD;
    dictionary[ends + WORD..ends + 2 * WORD].copy_from_slice(&0xFFFF_u32.to_le_bytes());
    assert_eq!(octets(&dictionary, &all), None, "a dictionary: an entry past its octets");
    assert_eq!(octets(&dictionary, &[1, 30]), None, "and of rows of its first entries each read alone");
    let alike: Vec<Value> =
        (0..200_usize).map(|n| Value::Char(format!("http://site{}.example/page{}", n % 30, n * 7919 % 5000).into())).collect();
    let mut symbols = chunk_of(&alike, true, FSST);
    // Two, so the last is a code whether or not an escape came before them.
    let last = symbols.len() - 2;
    symbols[last..].copy_from_slice(&[200, 200]);
    assert_eq!(octets(&symbols, &[199]), None, "a code of no symbol");
    assert!(octets(&symbols, &[0, 1, 2]).is_some(), "the rows before it are whole");
}
