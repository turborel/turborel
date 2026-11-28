//! Unit tests of MAX and MIN (`extremes.rs`): by a collation, their
//! states merged and their values given by vector; and of either, a
//! vector of character strings compared where it lies.

use std::sync::Arc;

use super::*;
use crate::storage::{Coded, Mask};
use crate::value::collation::{CASE_INSENSITIVE, Pad, UCS_BASIC, builtin};

fn text(s: &str) -> Value {
    Value::Char(s.into())
}

fn insensitive(max: bool) -> CollatedExtreme {
    let collation = builtin().into_iter().find(|c| c.name() == CASE_INSENSITIVE).unwrap();
    CollatedExtreme { max, collation: CollationRef { collation, pad: Pad::Space } }
}

/// Verifies: [DESIGN] RULES.md D53, MAX
/// and MIN of a collated column order by the collation: of 'b', 'C' and
/// 'a' case-insensitive MAX is 'C', MIN 'a'; one accumulator merging the
/// state of another fed the rest gives the same, wherever the values are
/// split; nulls and the rows not asked of a vector are passed over; a
/// vector of integers or doubles adds nothing; a state not one written is
/// refused as corrupt.
#[test]
fn collated_extremes_merge_and_take_vectors() {
    let values = [text("b"), Value::Null, text("C"), text("a")];
    for (max, want) in [(true, "C"), (false, "a")] {
        let f = insensitive(max);
        assert_eq!(f.name(), if max { "MAX" } else { "MIN" });
        let t = DataType::varying(5);
        for at in 0..=values.len() {
            let (mut before, mut after) = (f.accumulator(t.clone()), f.accumulator(t.clone()));
            before.update_many(&values[..at], None).unwrap();
            after.update_many(&values[at..], None).unwrap();
            let mut state = Vec::new();
            assert!(after.write_state(&mut state));
            before.merge(&state).unwrap();
            assert_eq!(before.finish().unwrap().to_string(), want, "{max} at {at}");
        }
        let mut some = f.accumulator(t.clone());
        some.update_many(&values, Some(&[0, 1])).unwrap();
        some.update_ints(&Ints::new(vec![9], 0, None), None, &crate::kernels::Portable).unwrap();
        some.update_floats(&Floats::new(vec![9.0_f64], None), None).unwrap();
        assert_eq!(some.finish().unwrap().to_string(), "b");
        assert_eq!(some.merge(&[0xff_u8]).unwrap_err().code, Code::Corrupt);
    }
    assert_eq!(insensitive(true).result_type(None).unwrap_err().code, Code::SyntaxRule);
}

/// MAX or MIN by `UCS_BASIC` of the pad characteristic `pad`.
fn basic(max: bool, pad: Pad) -> CollatedExtreme {
    let collation = builtin().into_iter().find(|c| c.name() == UCS_BASIC).unwrap();
    CollatedExtreme { max, collation: CollationRef { collation, pad } }
}

/// The strings of the vectors below, a null where none: 'a' and 'a ' are
/// equal padded and apart unpadded, and 'a' followed by a tab is below 'a'
/// padded and above it unpadded.
const STRINGS: [Option<&str>; 9] = [Some("b"), Some("a "), None, Some("a"), Some("a\t"), Some("é"), Some(""), Some("b "), Some("B")];

/// `STRINGS` as a vector of texts one after another.
fn plain() -> Texts {
    let (mut text, mut ends, mut nulls) = (String::new(), Vec::new(), Mask::none(STRINGS.len()));
    for (n, s) in STRINGS.iter().enumerate() {
        text.push_str(s.unwrap_or_default());
        ends.push(text.len());
        if s.is_none() {
            nulls.set(n);
        }
    }
    Texts::new(text, ends, Some(nulls))
}

/// `STRINGS` as codes of a dictionary whose entries are in another order,
/// one of them read by no row.
fn coded() -> Texts {
    let entries = ["zz", "é", "B", "b ", "", "a\t", "a", "a ", "b"];
    let ends = (1..=entries.len()).map(|n| entries[..n].concat().len()).collect();
    let code = |s: &str| u32::try_from(entries.iter().position(|e| *e == s).unwrap()).unwrap();
    let codes = STRINGS.iter().map(|s| s.map_or(0, code)).collect();
    let nulls = Mask::from_bools(&STRINGS.map(|s| s.is_none()));
    Texts::coded(Coded { codes, entries: Arc::new(Texts::new(entries.concat(), ends, None)) }, Some(nulls))
}

/// The orders a vector of texts is taken by.
const ORDERS: [&str; 4] = ["no collation", "octets", "padded keys", "folded keys"];

/// MAX's accumulator, or MIN's, of the order `order` of `ORDERS`: of no
/// collation; by UCS_BASIC with NO PAD, whose key is the text; by
/// UCS_BASIC with PAD SPACE or by a collation that folds case, which make
/// sort keys.
fn acc(order: &str, max: bool) -> Box<dyn Accumulator> {
    let t = DataType::varying(5);
    if order == "no collation" {
        Extreme { max }.accumulator(t)
    } else if order == "octets" {
        basic(max, Pad::None).accumulator(t)
    } else if order == "padded keys" {
        basic(max, Pad::Space).accumulator(t)
    } else {
        insensitive(max).accumulator(t)
    }
}

/// Checks that MAX's accumulator, or MIN's, of the order `order`, given
/// `before` and then the rows `rows`, or all, of `texts`, gives and
/// writes what one given `before` and then each of those rows' strings as
/// a value does.
fn taken_as_its_values((order, max): (&str, bool), texts: &Texts, rows: Option<&[u32]>, before: Option<&Value>) {
    let (mut by_vector, mut by_value) = (acc(order, max), acc(order, max));
    for one in [&mut by_vector, &mut by_value] {
        before.iter().try_for_each(|v| one.update(v)).unwrap();
    }
    by_vector.update_texts(texts, rows).unwrap();
    let all: Vec<u32> = (0..9).collect();
    for &r in rows.unwrap_or(&all) {
        if let Some(s) = STRINGS[usize::try_from(r).unwrap()] {
            by_value.update(&text(s)).unwrap();
        }
    }
    let (found, wanted) = (by_vector.finish().unwrap(), by_value.finish().unwrap());
    assert_eq!(format!("{found:?}"), format!("{wanted:?}"), "{order} max={max} rows={rows:?} before={before:?}");
    let (mut written, mut expected) = (Vec::new(), Vec::new());
    assert_eq!(by_vector.write_state(&mut written), by_value.write_state(&mut expected));
    assert_eq!(written, expected, "{order} max={max} written");
}

/// Verifies: [DESIGN] (RULES.md D1290) [X3.135-1986 §5.8 GR4d] MAX and MIN
/// given a vector of character strings, as texts one after another or as
/// codes of a dictionary, whole or the rows asked of it in any order, give
/// what they give of each of those strings' values in that order, and
/// write the same state. With no collation 'a' and 'a ' are equal and the
/// one met first is kept; by UCS_BASIC with NO PAD they are apart; by
/// UCS_BASIC with PAD SPACE and by a collation that folds case sort keys
/// are made. So too after a string was kept before the vector, a better
/// one or a worse, and after a null alone.
#[test]
fn extremes_take_a_vector_of_texts_where_it_lies() {
    let selections: [Option<Vec<u32>>; 5] =
        [None, Some(vec![]), Some(vec![2]), Some(vec![3, 1, 4, 0]), Some(vec![8, 7, 6, 5, 4, 3, 2, 1, 0])];
    let befores: [Option<Value>; 4] = [None, Some(Value::Null), Some(text("a")), Some(text("zz"))];
    for (order, max) in ORDERS.iter().flat_map(|order| [(*order, true), (*order, false)]) {
        for texts in [plain(), coded()] {
            for (rows, before) in selections.iter().flat_map(|rows| befores.iter().map(move |before| (rows, before))) {
                taken_as_its_values((order, max), &texts, rows.as_deref(), before.as_ref());
            }
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D1290) [X3.135-1986 §5.11 GR5] [ISO/IEC
/// 9075-2:2023 §4.3.2] the order a vector of texts is taken in where it
/// lies. With no collation the shorter string is padded, so of 'a ', 'a'
/// and 'a' before a tab the last is the least, and the greatest is the
/// first met of the two that are equal. By UCS_BASIC with NO PAD a prefix
/// is the less, so 'a' is the least and 'a ' the greatest wherever they
/// are met. Of the whole vector the least is the empty string and the
/// greatest the string of the greatest scalar.
#[test]
fn a_vector_of_texts_is_ordered_padded_or_by_its_octets() {
    let best = |order: &str, max: bool, rows: Option<&[u32]>| {
        let mut one = acc(order, max);
        one.update_texts(&plain(), rows).unwrap();
        format!("{:?}", one.finish().unwrap())
    };
    let shown = |s: &str| format!("{:?}", text(s));
    assert_eq!(best("no collation", false, None), shown(""));
    assert_eq!(best("no collation", true, None), shown("é"));
    assert_eq!(best("no collation", false, Some(&[1, 3, 4])), shown("a\t"));
    assert_eq!(best("no collation", true, Some(&[1, 3, 4])), shown("a "));
    assert_eq!(best("no collation", true, Some(&[3, 1, 4])), shown("a"));
    assert_eq!(best("octets", false, Some(&[1, 3, 4])), shown("a"));
    assert_eq!(best("octets", true, Some(&[1, 3, 4])), shown("a "));
    assert_eq!(best("octets", true, Some(&[3, 4, 1])), shown("a "));
    assert_eq!(best("octets", false, Some(&[2])), format!("{:?}", Value::Null));
}
