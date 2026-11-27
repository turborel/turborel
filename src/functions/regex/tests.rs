//! Unit tests of LIKE_REGEX over a vector of values (RULES.md D1188).

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
