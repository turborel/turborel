//! Unit tests of the delimiters row pattern recognition adds [ISO/IEC
//! 9075-2:2023 §5.2], and of the tokens beside them that lex as they did
//! [X3.135-1986 §5.3].

use super::*;

/// The tokens of `src`, its end left off.
fn toks(src: &str) -> Vec<Tok> {
    let mut list: Vec<Tok> = lex(src).unwrap_or_else(|e| panic!("{src}: {e:?}")).into_iter().map(|t| t.tok).collect();
    list.pop();
    list
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §5.1, §5.2] the vertical bar,
/// braces, circumflex and dollar sign are `<SQL special character>`s, `{-`
/// and `-}` `<delimiter token>`s of two characters, each longest first, so
/// `||` stays the concatenation operator; each ends the identifier before
/// it.
#[test]
fn the_row_pattern_delimiters_5_2() {
    let d = Tok::Delimiter;
    let id = |s: &str| Tok::Identifier(s.to_owned());
    assert_eq!(
        toks("^A|B{1,}$"),
        [
            d(delim::CIRCUMFLEX),
            id("A"),
            d(delim::VERTICAL_BAR),
            id("B"),
            d(delim::LEFT_BRACE),
            Tok::ExactNumeric("1".into()),
            d(delim::COMMA),
            d(delim::RIGHT_BRACE),
            d(delim::DOLLAR_SIGN)
        ]
    );
    assert_eq!(toks("{-A-}"), [d(delim::LEFT_BRACE_MINUS), id("A"), d(delim::RIGHT_MINUS_BRACE)]);
    assert_eq!(toks("{ - A - }"), [d(delim::LEFT_BRACE), d(delim::MINUS), id("A"), d(delim::MINUS), d(delim::RIGHT_BRACE)]);
    assert_eq!(toks("A||B"), [id("A"), d(delim::CONCATENATION), id("B")]);
    assert_eq!(toks("A|"), [id("A"), d(delim::VERTICAL_BAR)]);
}

/// Verifies: [X3.135-1986 §5.3] subtraction, a negative number and a
/// `<comment>` lex as they did beside the new delimiters.
#[test]
fn minus_and_comments_lex_as_they_did_5_3() {
    let d = Tok::Delimiter;
    let n = |s: &str| Tok::ExactNumeric(s.to_owned());
    assert_eq!(toks("1-2"), [n("1"), d(delim::MINUS), n("2")]);
    assert_eq!(toks("1 - -2"), [n("1"), d(delim::MINUS), d(delim::MINUS), n("2")]);
    assert_eq!(toks("1 -- }\n"), [n("1")]);
    assert_eq!(toks("1--{-\n-1"), [n("1"), d(delim::MINUS), n("1")]);
    assert!(lex("A ` B").is_err(), "still no token");
}
