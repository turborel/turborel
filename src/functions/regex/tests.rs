//! Unit tests of LIKE_REGEX over a vector of values (RULES.md D1188),
//! and of a subject read where it lies (RULES.md D1189).

use super::*;

/// A character literal's value.
fn chars(s: &str) -> Value {
    Value::Char(s.into())
}

/// Values of a column, the third null: with a match, without, the empty
/// string, one not of ASCII.
const VALUES: [&str; 6] = ["http://a.test/", "ftp://a.test/", "", "", "aéb", "a\r\nb"];

/// `VALUES` as a vector of texts.
fn values() -> Column {
    let mut ends = Vec::new();
    let text: String = VALUES
        .iter()
        .map(|v| {
            ends.push(ends.last().copied().unwrap_or(0_usize).saturating_add(v.len()));
            *v
        })
        .collect();
    let mut nulls = Mask::none(VALUES.len());
    nulls.set(2);
    Column::Texts(Texts::new(text, ends, Some(nulls)))
}

/// Verifies: [DESIGN] RULES.md D1188, D363 [ISO/IEC 9075-2:2023 §8.7
/// GR1]: LIKE_REGEX over a vector of values, its pattern and flag
/// constants, gives each row what a call of that row gives, Unknown of a
/// null value, and of every row if the pattern or the flag is null; a
/// pattern or a flag of each row has no call over vectors, and one that
/// is no regular expression fails as a row's call does.
#[test]
fn a_vector_of_values_is_matched_as_each_row_is() {
    let column = values();
    let row = |n: usize| if n == 2 { Value::Null } else { chars(VALUES[n]) };
    for (pattern, flag) in [("^https?://", ""), ("a.b", ""), ("a.b", "s"), ("^$", ""), ("é", ""), ("A", "i")] {
        let (pattern, flag, function) = (chars(pattern), chars(flag), LikeRegex::default());
        let made = function.eval_batch(&[Arg::Column(&column), Arg::Const(&pattern), Arg::Const(&flag)], VALUES.len()).unwrap().unwrap();
        for n in 0..VALUES.len() {
            let alone = function.call(&[row(n), pattern.clone(), flag.clone()]).unwrap();
            assert_eq!(format!("{:?}", made.values()[n]), format!("{alone:?}"), "{pattern:?} {flag:?} row {n}");
        }
    }
    let (pattern, flag, function) = (chars("a"), chars(""), LikeRegex::default());
    let nulled = function.eval_batch(&[Arg::Column(&column), Arg::Const(&Value::Null), Arg::Const(&flag)], 2).unwrap().unwrap();
    assert!(nulled.values().iter().all(Value::is_null) && nulled.values().len() == 2);
    let unknown = function.eval_batch(&[Arg::Column(&column), Arg::Const(&pattern), Arg::Const(&Value::Null)], 2).unwrap().unwrap();
    assert!(unknown.values().iter().all(Value::is_null));
    assert!(function.eval_batch(&[Arg::Column(&column), Arg::Column(&column), Arg::Const(&flag)], 2).is_none(), "a pattern of each row");
    assert!(function.eval_batch(&[Arg::Const(&pattern), Arg::Const(&pattern), Arg::Const(&flag)], 2).is_none(), "a constant value");
    let bad = chars("(");
    let failed = function.eval_batch(&[Arg::Column(&column), Arg::Const(&bad), Arg::Const(&flag)], 2).unwrap().unwrap_err();
    assert_eq!(failed.sqlstate(), function.call(&[row(0), bad.clone(), flag.clone()]).unwrap_err().sqlstate());
}

/// Verifies: [DESIGN] RULES.md D1189 [ISO/IEC 9075-2:2023 §6.31 GR18b
/// ii, §9.28 GR11b]: a subject not of ASCII is read where it lies, its
/// positions the octets its characters begin at. A start position in
/// characters names that character's position; one in octets the
/// character the octet is of, or the next if it is within one, and the
/// string's end within the last; one less than 1 or past the string
/// names none. A position is counted back in the units asked, from 1,
/// and what two positions span is the substring. A subject of ASCII
/// counts the same in either units.
#[test]
fn a_subject_read_where_it_lies_counts_in_its_units() {
    use super::subject::{Body, Subject};
    let value = chars("aébé€");
    let (characters, octets) = (chars(CHARACTERS), chars(OCTETS));
    let by_characters = Subject::new(&value, &characters).unwrap().unwrap();
    let by_octets = Subject::new(&value, &octets).unwrap().unwrap();
    assert!(matches!(by_characters.body, Body::Utf8(_)) && by_characters.len() == 9);
    let starts = |subject: &Subject<'_>, upto: i128| -> Vec<Option<usize>> { (0..=upto).map(|sp| subject.start(sp).unwrap()).collect() };
    assert_eq!(starts(&by_characters, 6), [None, Some(0), Some(1), Some(3), Some(4), Some(6), None]);
    let wanted = [None, Some(0), Some(1), Some(3), Some(3), Some(4), Some(6), Some(6), Some(9), Some(9), None];
    assert_eq!(starts(&by_octets, 10), wanted);
    assert_eq!((by_characters.position(0).unwrap(), by_characters.position(4).unwrap(), by_characters.position(9).unwrap()), (1, 4, 6));
    assert_eq!((by_octets.position(0).unwrap(), by_octets.position(4).unwrap(), by_octets.position(9).unwrap()), (1, 5, 10));
    assert_eq!(format!("{:?}", by_characters.substring(1, 4).unwrap()), format!("{:?}", chars("éb")));
    let plain = chars("abc");
    for units in [&characters, &octets] {
        let subject = Subject::new(&plain, units).unwrap().unwrap();
        assert!(matches!(subject.body, Body::Ascii(_)));
        assert_eq!((starts(&subject, 4), subject.position(2).unwrap()), (vec![None, Some(0), Some(1), Some(2), None], 3));
    }
    let none = chars("");
    assert_eq!(Subject::new(&none, &characters).unwrap().unwrap().start(1).unwrap(), None, "no first character");
}
