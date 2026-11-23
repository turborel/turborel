//! Tests of TRANSLATE_REGEX and SUBSTRING_REGEX of a character large
//! object, made and read a chunk at a time.

use super::*;
use crate::functions::regex::search::Substring;
use crate::value::Decimal;
use crate::value::lob::CHUNK_OCTETS;
use crate::value::lob::fixtures::{Temp, chunked, text};

/// A character literal's value.
fn chars(s: &str) -> Value {
    Value::Char(s.into())
}

/// An exact number's value.
fn number(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// TRANSLATE_REGEX of `subject` by `pattern` with `replacement`, all or
/// the occurrence `occurrence`, its large object made in `temp`.
fn translate(temp: &Temp, subject: &Value, (pattern, replacement): (&str, &str), occurrence: Option<i128>) -> Result<Value> {
    let every = if occurrence.is_some() { OCCURRENCE_WORD } else { kw::ALL };
    let args = [
        chars("CHARACTERS"),
        chars(every),
        chars(pattern),
        chars(""),
        subject.clone(),
        chars(replacement),
        number(1),
        number(occurrence.unwrap_or(1)),
    ];
    Translate::default().call_with(&args, Some(temp))
}

/// What the parser passes for OCCURRENCE.
const OCCURRENCE_WORD: &str = "OCCURRENCE";

/// The text of `v`, a character large object.
fn lob_text(v: &Value) -> String {
    let Value::Lob(lob) = v else { panic!("not a large object: {v:?}") };
    text(lob)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 GR9b iii, SR12f i 2 B]
/// TRANSLATE_REGEX of a character large object of several chunks, each match
/// replaced by a longer string, is a character large object longer than a chunk
/// with the text the string's TRANSLATE_REGEX would have, of few parts, and its
/// groups' captures crossing chunks' ends are put in.
#[test]
fn translate_regex_of_a_large_object_is_one() {
    let temp = Temp::default();
    let s: String = (0..98_304_usize).map(|i| if i % 1000 == 999 { "<b€😀>" } else { "abcdéfgh" }).collect();
    let lob = Value::Lob(chunked(&temp, &s));
    let made = translate(&temp, &lob, ("b€(😀)", "[$1$1]"), None).unwrap();
    let expected = s.replace("b€😀", "[😀😀]");
    assert_eq!(lob_text(&made), expected);
    let Value::Lob(m) = &made else { panic!("not a large object") };
    assert!(m.octets() > u64::try_from(CHUNK_OCTETS).unwrap());
    assert!(m.parts().len() <= 8, "{} parts", m.parts().len());
    let all_b = translate(&temp, &lob, ("b", "xyz"), None).unwrap();
    assert_eq!(lob_text(&all_b), s.replace('b', "xyz"));
    let third = translate(&temp, &lob, ("<b", "!"), Some(3)).unwrap();
    let at = s.match_indices("<b").nth(2).unwrap().0;
    assert_eq!(lob_text(&third), format!("{}!{}", s.get(..at).unwrap(), s.get(at + 2..).unwrap()));
    let none = translate(&temp, &lob, ("zz", "-"), None).unwrap();
    assert_eq!(lob_text(&none), s);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 GR9b; §9.29 GR3, GR5]
/// TRANSLATE_REGEX of a character large object refuses as of a string: a
/// pattern matching the zero-length string, SQLSTATE 2201U; a replacement that
/// is none, 2201V; and an occurrence less than 1 gives the subject.
#[test]
fn translate_regex_of_a_large_object_refused() {
    let temp = Temp::default();
    let lob = Value::Lob(chunked(&temp, &"ab".repeat(CHUNK_OCTETS)));
    let state = |r: Result<Value>| r.unwrap_err().sqlstate().as_str().to_owned();
    assert_eq!(state(translate(&temp, &lob, ("x*", "-"), None)), "2201U");
    assert_eq!(state(translate(&temp, &lob, ("a", "$"), None)), "2201V");
    let kept = translate(&temp, &lob, ("a", "-"), Some(0)).unwrap();
    assert_eq!(lob_text(&kept), "ab".repeat(CHUNK_OCTETS));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 GR4b, SR8f i 2]
/// SUBSTRING_REGEX of a character large object is a slice of it, crossing a
/// chunk's end, counted in characters or octets.
#[test]
fn substring_regex_of_a_large_object_is_a_slice() {
    let temp = Temp::default();
    let before = "é".repeat(CHUNK_OCTETS.div_euclid(2) - 3);
    let s = format!("{before}<{}>{before}", "€".repeat(9));
    let lob = Value::Lob(chunked(&temp, &s));
    let call = |units: &str, from: i128, group: i128| {
        let args = [chars(units), chars("<(€+)>"), chars(""), lob.clone(), number(from), number(1), number(group)];
        Substring::default().call_with(&args, Some(&temp)).unwrap()
    };
    assert_eq!(lob_text(&call("CHARACTERS", 1, 1)), "€".repeat(9));
    assert_eq!(lob_text(&call("OCTETS", 3, 0)), format!("<{}>", "€".repeat(9)), "octet 3 is within the second character");
    assert!(matches!(call("CHARACTERS", i128::try_from(before.chars().count()).unwrap() + 2, 0), Value::Null), "from past the match");
}
