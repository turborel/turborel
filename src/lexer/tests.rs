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

/// Verifies: [ISO/IEC 9075-2:2023 §5.2] `->`, the `<right arrow>`, and
/// `::`, the `<double colon>`, are `<delimiter token>`s of two characters,
/// longest first; a minus before a greater than sign apart, a single colon
/// and a `<comment>` lex as they did.
#[test]
fn the_right_arrow_and_double_colon_5_2() {
    let d = Tok::Delimiter;
    let id = |s: &str| Tok::Identifier(s.to_owned());
    assert_eq!(toks("R->A"), [id("R"), d(delim::RIGHT_ARROW), id("A")]);
    assert_eq!(toks("T::M"), [id("T"), d(delim::DOUBLE_COLON), id("M")]);
    assert_eq!(toks("A - > B"), [id("A"), d(delim::MINUS), d(delim::GREATER), id("B")]);
    assert_eq!(toks(":V"), [d(delim::COLON), id("V")]);
    assert_eq!(toks("A -->B\n"), [id("A")]);
}

/// Verifies: [EXTENSION] [P1 §4.1] the percent sign, ampersand, tilde and
/// exclamation mark of label expressions and edge patterns are delimiters
/// of one character, each ending the identifier before it, and `->` stays
/// one token beside them; `<->` is one token, the vector distance operator,
/// which an edge pattern takes as `<` then `->`; `!=` is still no token.
#[test]
fn the_graph_pattern_delimiters_4_1() {
    let d = Tok::Delimiter;
    let id = |s: &str| Tok::Identifier(s.to_owned());
    assert_eq!(
        toks("A&!B|%"),
        [id("A"), d(delim::AMPERSAND), d(delim::EXCLAMATION_MARK), id("B"), d(delim::VERTICAL_BAR), d(delim::PERCENT)]
    );
    assert_eq!(
        toks("<~[E]~>"),
        [d(delim::LESS), d(delim::TILDE), d(delim::LEFT_BRACKET), id("E"), d(delim::RIGHT_BRACKET), d(delim::TILDE), d(delim::GREATER)]
    );
    assert_eq!(toks("<->"), [d(delim::EUCLIDEAN_DISTANCE)]);
    assert_eq!(toks("<-"), [d(delim::LESS), d(delim::MINUS)]);
    let refused = lex("A != B").map(|_| panic!("!= lexed")).unwrap_err();
    assert_eq!(refused.code, Code::Lexical);
}

/// Verifies: [EXTENSION] [GS §10.3] after MATCH, "--" glued between ")"
/// or ")<" and "(" or ">(" is two minus signs, Cypher's relationships;
/// anywhere else it begins a `<comment>`, MATCH or no MATCH.
#[test]
fn cypher_relationships_after_match_10_3() {
    let d = Tok::Delimiter;
    let m = Tok::KeyWord(kw::MATCH);
    let (open, close) = (d(delim::LEFT_PAREN), d(delim::RIGHT_PAREN));
    assert_eq!(toks("MATCH)--("), [m.clone(), close.clone(), d(delim::MINUS), d(delim::MINUS), open.clone()]);
    assert_eq!(toks("MATCH)-->("), [m.clone(), close.clone(), d(delim::MINUS), d(delim::RIGHT_ARROW), open.clone()]);
    assert_eq!(toks("MATCH)<--("), [m.clone(), close.clone(), d(delim::LESS), d(delim::MINUS), d(delim::MINUS), open.clone()]);
    assert_eq!(toks("MATCH)<-->("), [m.clone(), close.clone(), d(delim::LESS), d(delim::MINUS), d(delim::RIGHT_ARROW), open]);
    assert_eq!(toks(")--(\n"), std::slice::from_ref(&close), "no MATCH before it");
    assert_eq!(toks("MATCH) --(\n"), [m.clone(), close.clone()]);
    assert_eq!(toks("MATCH)-- (\n"), [m.clone(), close.clone()]);
    assert_eq!(toks("MATCH]--(\n"), [m, d(delim::RIGHT_BRACKET)]);
}
