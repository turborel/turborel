//! The errors of reading a pattern and their SQLSTATEs.

use super::*;
use crate::error::{Code, SqlState};

/// Asserts `text` is refused with SQLSTATE 2FF01, its message naming
/// character `at`.
fn invalid_at(text: &str, at: usize) {
    let e = err(text);
    assert_eq!((e.code, e.state), (Code::Data, Some(SqlState::INVALID_SEARCH_EXPRESSION)), "{text}: {e:?}");
    assert!(e.msg.contains(&format!("at character {at} ")), "{text}: {}", e.msg);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.2 Description 3a, §11] a pattern
/// not of the grammar is refused with SQLSTATE 2FF01 and SQLCODE -301, the
/// message naming the character where it goes wrong: a missing operand, two
/// primaries with no operator, NOT twice, an unclosed quote or
/// parenthesis, a stray character, an empty word, missing key words.
#[test]
fn syntax_errors_name_the_character() {
    invalid_at(r#""a" & "#, 7);
    invalid_at(r#""a" "b""#, 5);
    invalid_at(r#"NOT NOT "a""#, 5);
    invalid_at(r#""a" | "b"#, 7);
    invalid_at(r#"("a" | "b""#, 11);
    invalid_at(r#""a" # "b""#, 5);
    invalid_at(r#""""#, 1);
    invalid_at("", 1);
    invalid_at(r#""a" NEAR "b" WITHIN WORDS IN ORDER"#, 21);
    invalid_at(r#""a" NEAR "b" WITHIN 1 LINES IN ORDER"#, 23);
    invalid_at(r#""a" NEAR "b" WITHIN 1 WORDS ORDER"#, 29);
    invalid_at(r#""a" IN SAME WORDS AS "b""#, 13);
    invalid_at(r#""a" IN SAME SENTENCE "b""#, 22);
    invalid_at(r#"IS "a""#, 4);
    invalid_at(r#""a" NEAR "b" WITHIN 99999999999999999999 WORDS IN ORDER"#, 21);
    invalid_at(r#"THESAURUS "" EXPAND SYNONYM TERM OF "a""#, 11);
    invalid_at(r#"THESAURUS "t" EXPAND WIDER TERM OF "a""#, 22);
    invalid_at(r#"THESAURUS "t" EXPAND SYNONYM TERM OF "a" FOR 2 LEVELS"#, 42);
    invalid_at(r#"THESAURUS "t" EXPAND BROADER TERM OF "a" FOR 2"#, 47);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3a, 3b] a word of
/// wild cards only, an escape before a character it cannot escape or at
/// the end, an escape specification not of one character, and a phrase of
/// optional words only are refused with SQLSTATE 2FF01.
#[test]
fn wild_cards_and_escapes_refused() {
    invalid_at(r#""%""#, 1);
    invalid_at(r#""_%""#, 1);
    invalid_at(r#""a _ b""#, 1);
    invalid_at(r#""% %""#, 1);
    invalid_at(r#""a!b" ESCAPE "!""#, 1);
    invalid_at(r#""ab!" ESCAPE "!""#, 1);
    invalid_at(r#""a" ESCAPE "!!""#, 12);
    invalid_at(r#""a" ESCAPE """#, 12);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3a, 3d, 3e] a
/// stemmed word with a wild card or an escape, SOUNDS LIKE or FUZZY FORM
/// OF of a word with one or of a phrase, a thesaurus term stemmed or with
/// a wild card, and IS ABOUT a stemmed word are refused with 2FF01.
#[test]
fn expansion_rules_refused() {
    invalid_at(r#"STEMMED FORM OF "a%""#, 17);
    invalid_at(r#"FORM OF "ab" ESCAPE "!""#, 9);
    invalid_at(r#"SOUNDS LIKE "a%""#, 13);
    invalid_at(r#"SOUNDS LIKE "a!%" ESCAPE "!""#, 13);
    invalid_at(r#"FUZZY FORM OF "a b""#, 15);
    invalid_at(r#"THESAURUS "t" EXPAND SYNONYM TERM OF FORM OF "a""#, 38);
    invalid_at(r#"THESAURUS "t" EXPAND SYNONYM TERM OF "a_""#, 38);
    invalid_at(r#"IS ABOUT FORM OF "a""#, 10);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3, §4.2.3.3] a
/// token list holds words, not phrases; a list stands only before NEAR or
/// IN SAME; a parenthesized expression is no list; all refused with 2FF01.
#[test]
fn lists_refused_where_not_allowed() {
    invalid_at(r#""a b" NEAR "c" WITHIN 1 WORDS IN ORDER"#, 1);
    invalid_at(r#""a" NEAR ("b", "c d") WITHIN 1 WORDS IN ORDER"#, 10);
    invalid_at(r#"("a", "b")"#, 1);
    invalid_at(r#"("a" | "b") NEAR "c" WITHIN 1 WORDS IN ORDER"#, 1);
    invalid_at(r#"("a" & "b", "c") IN SAME SENTENCE AS "d""#, 1);
}

/// Verifies: [DESIGN] a pattern nesting deeper than `MAX_DEPTH`, in
/// parentheses or operators, is refused with 2FF01; one at the limit is
/// read.
#[test]
fn nesting_bounded() {
    let deep = format!(r#"{}"x"{}"#, "(".repeat(MAX_DEPTH), ")".repeat(MAX_DEPTH));
    assert!(parse(&deep, Language::English).is_ok());
    let deeper = format!("({deep})");
    assert_eq!(err(&deeper).state, Some(SqlState::INVALID_SEARCH_EXPRESSION));
    let chain = vec![r#""x""#; MAX_DEPTH.saturating_add(2)].join(" | ");
    assert_eq!(err(&chain).state, Some(SqlState::INVALID_SEARCH_EXPRESSION));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3c, §11] a language
/// the tokenizer does not know is refused with SQLSTATE 2FF02, the message
/// naming its character; a key word before a quote is no language.
#[test]
fn unknown_language() {
    let e = err(r#""a" | klingon "b""#);
    assert_eq!((e.code, e.state), (Code::Data, Some(SqlState::INVALID_LANGUAGE_SPECIFICATION)));
    assert!(e.msg.contains("at character 7 "), "{}", e.msg);
    assert_eq!(err(r#""a" & AS "b""#).state, Some(SqlState::INVALID_SEARCH_EXPRESSION));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.4.2 Description 2a i, §6.6.2
/// Description 5c, §4.4.2, §11] a pattern every word of which is a stop
/// word of its language is effectively empty, SQLSTATE 2FF03, words,
/// phrases, stemmed words and expansions alike; one with a word that is
/// not, or a stop word of another language, is read.
#[test]
fn effectively_empty() {
    for text in [r#""the""#, r#""The" & NOT "and""#, r#""this and that""#, r#"FORM OF "the" | SOUNDS LIKE "and""#, r#""the %""#] {
        let e = err(text);
        assert_eq!((e.code, e.state), (Code::Data, Some(SqlState::EFFECTIVELY_EMPTY_SEARCH_EXPRESSION)), "{text}");
    }
    for text in [r#""the" | "cat""#, r#"NONE "the""#, r#""th%""#, r#""the cat""#] {
        assert!(parse(text, Language::English).is_ok(), "{text}");
    }
}
