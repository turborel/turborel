//! Unit tests of the string GET DIAGNOSTICS ALL assigns [ISO/IEC
//! 9075-2:2023 §23.1 GR6a]: its presentations joined, those of no
//! characters left out, and its length bounded [§6.32 GR2b ii 3 B].

use super::*;
use crate::value::Decimal;

fn text(s: &str) -> Value {
    Value::Char(s.to_owned())
}

fn number(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1 GR6a] each item is
/// presented as its name, ` = ` and its value as a character string, a
/// number as its digits, joined by semicolons in turn; an item whose
/// value is a zero-length string, or one of spaces alone, which compares
/// equal to it [§8.2 GR3b], is left out, first, last or between; of no
/// item the string is zero-length.
#[test]
fn presentations_joined_and_empty_values_left_out_23_1() {
    let presented = vec![
        ("CATALOG_NAME", text(" ")),
        ("NUMBER", number(1)),
        ("COMMAND_FUNCTION", text("")),
        ("MORE", text("N")),
        ("DYNAMIC_FUNCTION_CODE", number(0)),
        ("COMMAND_FUNCTION_CODE", number(-5)),
        ("MESSAGE_TEXT", text(" a ")),
        ("TABLE_NAME", text("")),
    ];
    let s = concatenated(presented).unwrap().to_string();
    assert_eq!(s, "NUMBER = 1;MORE = N;DYNAMIC_FUNCTION_CODE = 0;COMMAND_FUNCTION_CODE = -5;MESSAGE_TEXT =  a ");
    assert_eq!(concatenated(vec![("CURSOR_NAME", text(""))]).unwrap().to_string(), "");
    assert_eq!(concatenated(Vec::new()).unwrap().to_string(), "");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.32 GR2b ii 3 B] a string
/// as long as the longest character string varying is kept whole; longer
/// by spaces alone, it is cut to that length; longer by another
/// character, SQLSTATE 22001.
#[test]
fn the_longest_string_bounded_6_32() {
    let longest = usize::try_from(MAX_CHARACTER_LENGTH).unwrap();
    let n = longest.checked_sub("X = ".len()).unwrap();
    let fill = |tail: &str| vec![("X", text(&format!("{}{tail}", "é".repeat(n))))];
    let exact = concatenated(fill("")).unwrap();
    assert_eq!(exact.to_string().chars().count(), longest, "in characters, not octets");
    let spaced = concatenated(fill("   b")).err().map(|e| e.sqlstate());
    assert_eq!(spaced, Some(SqlState::STRING_DATA_RIGHT_TRUNCATION));
    let kept = concatenated(fill("   ")).unwrap().to_string();
    assert_eq!((kept.chars().count(), kept.ends_with('é')), (longest, true), "the spaces past it cut");
}
