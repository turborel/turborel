//! Unit tests of `compare_strings`: the shorter string padded with spaces
//! [X3.135-1986 §5.11 GR5], scalars in Unicode order (RULES.md R-TEST-1,
//! R-TEST-8).

use std::cmp::Ordering;

use super::compare_strings;

/// The order the padded scalars give, as `compare_strings` defined it before
/// it compared bytes: the reference it is checked against.
fn by_scalars(left: &str, right: &str) -> Ordering {
    let length = left.chars().count().max(right.chars().count());
    let padded = |s: &str| s.chars().chain(std::iter::repeat(' ')).take(length).collect::<Vec<char>>();
    padded(left).cmp(&padded(right))
}

/// Verifies: [X3.135-1986 §5.11 GR5] the shorter string is padded with
/// spaces: trailing spaces are not significant, a character after the
/// common part is compared with a space, below it for a control
/// character, above it for any other, of one byte or of several; and
/// both argument orders give reversed answers.
#[test]
fn shorter_string_padded_with_spaces() {
    let cases = [
        ("abc", "abc", Ordering::Equal),
        ("abc", "abc   ", Ordering::Equal),
        ("", "  ", Ordering::Equal),
        ("abc", "abc\t", Ordering::Greater),
        ("abc", "abc!", Ordering::Less),
        ("abc", "abcé", Ordering::Less),
        ("abc", "abc  \u{1}", Ordering::Greater),
        ("abd", "abc  ", Ordering::Greater),
        ("ab", "abc", Ordering::Less),
        ("", "", Ordering::Equal),
    ];
    for (left, right, expected) in cases {
        assert_eq!(compare_strings(left, right), expected, "{left:?} {right:?}");
        assert_eq!(compare_strings(right, left), expected.reverse(), "{right:?} {left:?}");
    }
}

/// Verifies: [X3.135-1986 §5.11 GR6] the collating sequence is Unicode
/// scalar order, as before strings were compared byte by byte: for
/// strings of one, two, three and four byte characters, padded or not,
/// the byte comparison gives the scalars' order.
#[test]
fn bytes_compare_in_scalar_order() {
    let words = ["", " ", "a", "a ", "a\u{7f}", "aé", "a€", "a😀", "é", "é ", "€", "😀", "\u{1}", "z\t", "ab", "a b", "€a", "a\u{a0}"];
    for left in words {
        for right in words {
            assert_eq!(compare_strings(left, right), by_scalars(left, right), "{left:?} {right:?}");
        }
    }
}
