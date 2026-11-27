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

/// Seven subjects as a vector of texts, the fourth null: with a match,
/// with several, with none, the empty string, one not of ASCII.
const SUBJECTS: [&str; 7] =
    ["http://www.example.test/a/b", "https://b.test/", "ftp://x", "", "", "http://ü.test/é/x", "http://a/http://b/"];

/// `SUBJECTS` as a vector of texts.
fn subjects() -> Column {
    let (joined, ends) = SUBJECTS.iter().fold((String::new(), Vec::new()), |(mut all, mut ends), s| {
        all.push_str(s);
        ends.push(all.len());
        (all, ends)
    });
    let nulls = Mask::from_bools(&[false, false, false, true, false, false, false]);
    Column::Texts(Texts::new(joined, ends, Some(nulls)))
}

/// The operands of a TRANSLATE_REGEX of `pattern` with `replacement`
/// beside its subject: the units, ALL or OCCURRENCE, the pattern, the
/// flag, the replacement, the start position and the occurrence.
fn operands(pattern: &str, replacement: &str, every: &str, occurrence: i128) -> [Value; 7] {
    [chars("CHARACTERS"), chars(every), chars(pattern), chars(""), chars(replacement), number(1), number(occurrence)]
}

/// `consts` and the vector `column` in the subject's place, as a call
/// over vectors takes them.
fn over<'a>(consts: &'a [Value; 7], column: &'a Column) -> [Arg<'a>; 8] {
    let [units, all, p, flag, with, from, nth] = consts;
    [
        Arg::Const(units),
        Arg::Const(all),
        Arg::Const(p),
        Arg::Const(flag),
        Arg::Column(column),
        Arg::Const(with),
        Arg::Const(from),
        Arg::Const(nth),
    ]
}

/// `consts` and `subject` in the subject's place, as a call takes them.
fn beside(consts: &[Value; 7], subject: Value) -> [Value; 8] {
    let [units, all, p, flag, with, from, nth] = consts.clone();
    [units, all, p, flag, subject, with, from, nth]
}

/// Verifies: [DESIGN] RULES.md D1188, D363 [ISO/IEC 9075-2:2023 §6.33
/// GR9]: TRANSLATE_REGEX over a vector of subjects, its other operands
/// constants, gives each row what a call of that row gives, the pattern's
/// searcher and replacement kept across the rows: a subject with a match,
/// with several, with none, a null, the empty string, one not of ASCII;
/// of ALL and of an occurrence.
#[test]
fn a_vector_of_subjects_is_translated_as_each_row_is() {
    let column = subjects();
    let row = |n: usize| if n == 3 { Value::Null } else { chars(SUBJECTS[n]) };
    let cases = [
        ("^https?://(www\\.)?([^/]+)/.*$", "$2", kw::ALL, 1_i128),
        ("/[a-z]", "#", kw::ALL, 1),
        ("/[a-z]", "#", OCCURRENCE_WORD, 2),
        ("t+", "<$0>", OCCURRENCE_WORD, 1),
    ];
    for (pattern, replacement, every, occurrence) in cases {
        let consts = operands(pattern, replacement, every, occurrence);
        let function = Translate::default();
        let made = function.eval_batch(&over(&consts, &column), SUBJECTS.len()).unwrap().unwrap();
        for n in 0..SUBJECTS.len() {
            let alone = function.call(&beside(&consts, row(n))).unwrap();
            assert_eq!(format!("{:?}", made.values()[n]), format!("{alone:?}"), "{pattern:?} row {n}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1189 [ISO/IEC 9075-2:2023 §6.33 GR9b;
/// §9.29]: TRANSLATE_REGEX of a subject not of ASCII, read where it
/// lies, alone and in a vector, gives what its characters give. The
/// cases are a group cut out between characters of several octets, each match of ALL of a
/// character of two octets, an occurrence of one, matches of no ASCII
/// character, and a start position in characters and in octets, one
/// within a character naming the next.
#[test]
fn a_subject_not_of_ascii_is_translated_where_it_lies() {
    // Each case is a subject, then the pattern, the replacement, ALL or
    // OCCURRENCE and the units, then the occurrence and the start
    // position, then what is wanted.
    let cases: [(&str, [&str; 4], [i128; 2], &str); 8] = [
        ("http://ü.test/é/x", ["^https?://(www\\.)?([^/]+)/.*$", "$2", kw::ALL, "CHARACTERS"], [1, 1], "ü.test"),
        ("aébé€", ["é", "e", kw::ALL, "CHARACTERS"], [1, 1], "aebe€"),
        ("aébé€", ["é", "<$0>", OCCURRENCE_WORD, "CHARACTERS"], [2, 1], "aéb<é>€"),
        ("aébé€", ["[^a-z]", "-", kw::ALL, "OCTETS"], [1, 1], "a-b--"),
        ("aébé€", ["é", "e", kw::ALL, "CHARACTERS"], [1, 3], "aébe€"),
        ("aébé€", ["é", "e", kw::ALL, "OCTETS"], [1, 3], "aébe€"),
        ("aébé€", ["é", "e", kw::ALL, "OCTETS"], [1, 2], "aebe€"),
        ("aébé€", ["(.)€", "$1$1", kw::ALL, "OCTETS"], [1, 8], "aébé€"),
    ];
    for (subject, [pattern, replacement, every, units], [occurrence, from], wanted) in cases {
        let mut consts = operands(pattern, replacement, every, occurrence);
        (consts[0], consts[5]) = (chars(units), number(from));
        let function = Translate::default();
        let alone = function.call(&beside(&consts, chars(subject))).unwrap();
        assert_eq!(format!("{alone:?}"), format!("{:?}", chars(wanted)), "{subject:?} {pattern:?} {units} from {from}");
        let column = Column::Texts(Texts::new(format!("{subject}{subject}"), vec![subject.len(), subject.len() * 2], None));
        let made = function.eval_batch(&over(&consts, &column), 2).unwrap().unwrap();
        for row in made.values() {
            assert_eq!(format!("{row:?}"), format!("{alone:?}"), "in a vector, {subject:?} {pattern:?}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1188, D363 [ISO/IEC 9075-2:2023 §6.33
/// GR9b i, §9.29 GR3]: over a vector of subjects a null pattern gives
/// nulls; a pattern that matches no characters fails as the call of a
/// row does; and operands that are not constants beside the vector have
/// no call over vectors, so each row is called.
#[test]
fn a_vector_call_gives_nulls_fails_or_gives_way_as_rows_do() {
    let column = subjects();
    let mut nulled = operands("a", "b", kw::ALL, 1);
    nulled[2] = Value::Null;
    let made = Translate::default().eval_batch(&over(&nulled, &column), 2).unwrap().unwrap();
    assert!(made.values()[0].is_null() && made.values()[1].is_null());
    let consts = operands("a", "b", kw::ALL, 1);
    let mut varied = over(&consts, &column);
    varied[2] = Arg::Column(&column);
    assert!(Translate::default().eval_batch(&varied, 2).is_none(), "a pattern of each row");
    let empty = operands("x*", "-", kw::ALL, 1);
    let state = |r: Result<Value>| r.err().map(|e| e.sqlstate().as_str().to_owned());
    let failed = Translate::default().eval_batch(&over(&empty, &column), 3).unwrap().map(|_| Value::Null);
    assert_eq!(state(failed), state(Translate::default().call(&beside(&empty, chars(SUBJECTS[0])))));
    assert!(state(Translate::default().call(&beside(&empty, chars(SUBJECTS[0])))).is_some());
}

/// Verifies: [DESIGN] RULES.md D1188 [ISO/IEC 9075-2:2023 §9.29 GR9a]: a
/// match is replaced as fn:replace replaces the match as a string of its
/// own, whether it is searched again or replaced from its own groups:
/// `(^a)|(a)` matches the `a` after `b` by its second group and that `a`
/// alone by its first, so the first is what is put; a match that is the
/// whole subject, and one of a pattern that reads nothing beside a
/// character, give their own groups.
#[test]
fn a_match_is_replaced_as_the_match_alone_is() {
    let temp = Temp::default();
    let made = |subject: &str, pattern: &str, replacement: &str| {
        format!("{:?}", translate(&temp, &chars(subject), (pattern, replacement), None).unwrap())
    };
    let wanted = |s: &str| format!("{:?}", chars(s));
    assert_eq!(made("ba", "(^a)|(a)", "[$1|$2]"), wanted("b[a|]"));
    assert_eq!(made("a", "(^a)|(a)", "[$1|$2]"), wanted("[a|]"));
    assert_eq!(made("ba", "(c)|(a)", "[$1|$2]"), wanted("b[|a]"));
    assert_eq!(made("http://www.a.test/b", "^https?://(www\\.)?([^/]+)/.*$", "$2"), wanted("a.test"));
}

/// Verifies: [DESIGN] RULES.md D1188 [ISO/IEC 9075-2:2023 §9.29 GR3]:
/// once a match of a subject is replaced, no match is searched for at
/// the subject's end, where only one of no characters could begin and the
/// pattern is known to have none: subjects that each match whole are a
/// search each, every one finding a match, so their searcher stops
/// asking its automaton first; subjects with no match keep it asking; and
/// a pattern that matches no characters still fails at the end of a
/// subject it has not matched before.
#[test]
fn no_match_is_searched_for_at_the_end_of_a_subject_replaced() {
    let (units, every, one) = (chars("CHARACTERS"), chars(kw::ALL), number(1));
    let asked = Asked { units: &units, every: &every, replacement: Cow::Borrowed("$2"), start: 1, occurrence: &one };
    let regex = Regex::new("^https?://(www\\.)?([^/]+)/.*$", "s").unwrap();
    let (mut searcher, mut with, mut out, mut found) = (regex.searcher(), None, String::new(), Captures::default());
    assert!(searcher.asks_first(), "at first");
    for _ in 0..64_usize {
        assert!(asked.of_text("http://www.a.test/b", (&mut searcher, &mut with, &mut found), &mut out).unwrap());
    }
    assert_eq!(out, "a.test".repeat(64));
    assert!(!searcher.asks_first(), "every search found a match");
    for _ in 0..64_usize {
        assert!(asked.of_text("ftp://a.test/b", (&mut searcher, &mut with, &mut found), &mut out).unwrap());
    }
    assert!(searcher.asks_first(), "none found one");
    let empty = Regex::new("b*", "").unwrap();
    let failed = asked.of_text("a", (&mut empty.searcher(), &mut None, &mut found), &mut out).unwrap_err();
    assert_eq!(failed.sqlstate().as_str(), "2201U");
}
