//! Tests of the lexer's default state: tokens, separators, literals,
//! references, comments, pragmas and line ends.

use super::*;
use crate::error::SqlState;

/// The tokens of `text` read by XML `version`'s rules, the end not
/// included, or the first error.
fn tokens_by(text: &str, version: Version) -> Result<Vec<Tok>> {
    let mut lexer = Lexer::new(text, version)?;
    let mut tokens = Vec::new();
    loop {
        let tok = lexer.token().tok.map_err(|e| *e)?;
        if tok == Tok::End {
            return Ok(tokens);
        }
        tokens.push(tok);
    }
}

/// The tokens of `text` by XML 1.1's rules.
fn tokens(text: &str) -> Vec<Tok> {
    tokens_by(text, Version::V11).unwrap_or_else(|e| panic!("{text}: {e}"))
}

/// The message of the syntax error `text` raises, by `version`'s rules.
fn error_by(text: &str, version: Version) -> String {
    let error = tokens_by(text, version).unwrap_err();
    assert_eq!(error.sqlstate(), SqlState::XQUERY_ERROR, "{error}");
    assert!(error.msg.contains("err:XPST0003"), "{error}");
    error.to_string()
}

/// The message of the syntax error `text` raises by XML 1.1's rules.
fn error(text: &str) -> String {
    error_by(text, Version::V11)
}

/// An unprefixed name token.
fn name(local: &str) -> Tok {
    Tok::Name(QName::local(local))
}

/// A symbol token.
fn sym(sym: Sym) -> Tok {
    Tok::Sym(sym)
}

/// A string literal token.
fn string(value: &str) -> Tok {
    Tok::String(value.to_owned())
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.2.1
/// \[154\], \[155\]] [XQuery Appendix A.1 \[80\]] names are QNames whose
/// colon nothing surrounds [Appendix A.1.2, xml-version], keywords among
/// them as none is reserved [Appendix A.2]; the wildcards `p:*` and
/// `*:l` are read whole; a colon alone is no token.
#[test]
fn names_qnames_and_wildcards() {
    assert_eq!(tokens("for"), vec![name("for")]);
    assert_eq!(tokens("p:local"), vec![Tok::Name(QName { prefix: Some("p".to_owned()), local: "local".to_owned() })]);
    assert_eq!(tokens("p:*"), vec![Tok::PrefixWildcard("p".to_owned())]);
    assert_eq!(tokens("*:l"), vec![Tok::LocalWildcard("l".to_owned())]);
    assert!(error("p : l").contains("':' begins no token"));
    assert!(error("a:b:c").contains("':' begins no token"), "a QName has one colon");
    assert_eq!(tokens("child::a"), vec![name("child"), sym(Sym::Axis), name("a")]);
    assert_eq!(tokens("\u{e9}l\u{e8}ve"), vec![name("\u{e9}l\u{e8}ve")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2a] [XQuery Appendix
/// A.2] [XQuery Appendix A.1.2, xml-version] names are `NCName`s of the
/// XML version whose lexical rules apply: a character XML 1.1 opened to
/// names, as U+0221 that Unicode 2.0 had not assigned, is a name
/// character by XML 1.1's rules and begins no token by XML 1.0's.
#[test]
fn names_follow_the_xml_version() {
    assert_eq!(tokens_by("\u{221}", Version::V11).unwrap(), vec![name("\u{221}")]);
    assert!(error_by("\u{221}", Version::V10).contains("begins no token"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.2.4.1]
/// [XQuery Appendix A.2.2] a hyphen and a full stop are name characters,
/// so `foo-foo` and `a.b` are names and `foo -foo` is a subtraction; a
/// comment separates as whitespace does, so `foo(: c :)- foo` is a
/// subtraction too.
#[test]
fn hyphens_and_full_stops_need_separators_after_names() {
    assert_eq!(tokens("foo-foo"), vec![name("foo-foo")]);
    assert_eq!(tokens("a.b"), vec![name("a.b")]);
    assert_eq!(tokens("foo- foo"), vec![name("foo-"), name("foo")]);
    assert_eq!(tokens("foo -foo"), vec![name("foo"), sym(Sym::Minus), name("foo")]);
    assert_eq!(tokens("foo(: c :)- foo"), vec![name("foo"), sym(Sym::Minus), name("foo")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.2.1
/// \[141\] to \[143\]] [XQuery §3.1.1] numeric literals: integers,
/// decimals with digits on either side of the point or both, doubles
/// with an exponent of digits and an optional sign; `1e` is no double.
#[test]
fn numeric_literals() {
    assert_eq!(tokens("12"), vec![Tok::Integer("12".to_owned())]);
    assert_eq!(tokens("12.5 .5 5."), vec![Tok::Decimal("12.5".to_owned()), Tok::Decimal(".5".to_owned()), Tok::Decimal("5.".to_owned())]);
    assert_eq!(tokens("125E2 1.5e-3 .5e+1 5.E0"), ["125E2", "1.5e-3", ".5e+1", "5.E0"].map(|t| Tok::Double(t.to_owned())).to_vec());
    assert_eq!(tokens("1 to 2"), vec![Tok::Integer("1".to_owned()), name("to"), Tok::Integer("2".to_owned())]);
    assert_eq!(tokens(".."), vec![sym(Sym::DotDot)]);
    assert_eq!(tokens("."), vec![sym(Sym::Dot)]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.2.4.1]
/// [XQuery Appendix A.2.2] a numeric literal and a name or number right
/// after it are two non-delimiting terminals with no separator: `10div 3`
/// and `10div3` are syntax errors, `10 div 3` is not; `1e` is a number
/// and a name together.
#[test]
fn a_numeric_literal_is_separated_from_what_follows() {
    assert!(error("10div 3").contains("separated from a name or number"));
    assert!(error("10div3").contains("separated"));
    assert!(error("1e").contains("separated"));
    assert!(error("1.5.6").contains("separated"));
    assert_eq!(tokens("10 div 3").len(), 3);
    assert_eq!(tokens("10 div3"), vec![Tok::Integer("10".to_owned()), name("div3")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.2.1
/// \[144\] to \[147\]] [XQuery §3.1.1] string literals: in either quote,
/// the quote doubled standing for one, and predefined entity and
/// character references replaced; the other quote and line ends as they
/// are.
#[test]
fn string_literals() {
    assert_eq!(tokens(r#""a""b""#), vec![string("a\"b")]);
    assert_eq!(tokens("'it''s'"), vec![string("it's")]);
    assert_eq!(tokens(r#"'say "hi"'"#), vec![string("say \"hi\"")]);
    assert_eq!(tokens(r#""&lt;&gt;&amp;&quot;&apos;""#), vec![string("<>&\"'")]);
    assert_eq!(tokens(r#""&#8364;&#x20AC;""#), vec![string("\u{20AC}\u{20AC}")]);
    assert_eq!(tokens("\"\""), vec![string("")]);
    assert_eq!(tokens("\"a\nb\""), vec![string("a\nb")]);
    assert_eq!(tokens(r#""(: not a comment :)""#), vec![string("(: not a comment :)")]);
    assert!(error("\"open").contains("not closed by its quote"));
    assert!(error("'a''").contains("not closed"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.2.1
/// \[145\], \[153\]] [XML §4.1 \[66\]] an `&` in a string literal begins a
/// predefined entity reference or a character reference to a `Char` of
/// the version whose rules apply, closed by `;`: XML 1.1 has `&#x1;`,
/// XML 1.0 not; neither has `&#0;`, an undeclared entity, or an `&`
/// alone.
#[test]
fn references_in_string_literals() {
    assert_eq!(tokens_by("'&#x1;'", Version::V11).unwrap(), vec![string("\u{1}")]);
    assert!(error_by("'&#x1;'", Version::V10).contains("'&' begins a reference"));
    assert!(error("'&#0;'").contains("begins a reference"));
    assert!(error("'&#xD800;'").contains("begins a reference"));
    assert!(error("'&#99999999999;'").contains("begins a reference"));
    assert!(error("'&nbsp;'").contains("begins a reference"));
    assert!(error("'&amp'").contains("begins a reference"));
    assert!(error("'a & b'").contains("begins a reference"));
    assert!(error("'&#;'").contains("begins a reference"));
    assert!(error("'&#x;'").contains("begins a reference"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.2.1
/// \[151\]] [XQuery Appendix A.1.3, grammar-note: comments] comments
/// separate tokens, and nest: one ends at the `:)` that balances its
/// `(:`; an unbalanced one is a syntax error, even where a `:)` is in a
/// string literal within it.
#[test]
fn comments_nest() {
    assert_eq!(tokens("(: commenting out a (: comment :) may be confusing, but often helpful :) 1"), vec![Tok::Integer("1".to_owned())]);
    assert_eq!(tokens("(::)1(:a:)"), vec![Tok::Integer("1".to_owned())]);
    assert!(error("(: \"this is just a string :)\" :)").contains("string literal is not closed"));
    assert!(error("(: (: :)").contains("comment is not closed"));
    assert!(error("1 (: open").contains("comment is not closed"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.2.2]
/// [XQuery Appendix A.2] the delimiting terminals, each the longest that
/// matches: `<<` and `<=` before `<`, `::` and `:=` before a colon, `..`
/// before `.`, `//` before `/`, `<!--` and `<?` before `<`.
#[test]
fn delimiting_terminals_longest_first() {
    assert_eq!(
        tokens("<< <= < >> >= > != ="),
        [Sym::Precedes, Sym::Le, Sym::Lt, Sym::Follows, Sym::Ge, Sym::Gt, Sym::Ne, Sym::Eq].map(sym).to_vec()
    );
    assert_eq!(tokens(":= :: .. . // /"), [Sym::Assign, Sym::Axis, Sym::DotDot, Sym::Dot, Sym::SlashSlash, Sym::Slash].map(sym).to_vec());
    assert_eq!(tokens("<!-- <? ( ) [ ] { } , ; $ @ ? + - * |").len(), 17);
    assert_eq!(tokens("<!--"), vec![sym(Sym::CommentStart)]);
    assert!(error("#").contains("'#' begins no token"));
    assert!(error("!").contains("begins no token"));
    for (text, sym) in SYMBOLS {
        assert_eq!(symbol_text(sym), text);
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.1
/// \[66\], \[67\]] [XQuery Appendix A.2.4.2] a pragma is read whole, its
/// whitespace explicit: `(#`, whitespace, a QName, and its contents as
/// written to the first `#)`, comments not recognized in it.
#[test]
fn pragmas() {
    let pragma = |prefix: Option<&str>, local: &str, contents: &str| {
        Tok::Pragma(Pragma { name: QName { prefix: prefix.map(str::to_owned), local: local.to_owned() }, contents: contents.to_owned() })
    };
    assert_eq!(tokens("(# ext:timeout 10 #)"), vec![pragma(Some("ext"), "timeout", " 10 ")]);
    assert_eq!(tokens("(#p#)"), vec![pragma(None, "p", "")]);
    assert_eq!(tokens("(#p (: x :)#)"), vec![pragma(None, "p", " (: x :)")]);
    assert_eq!(tokens("(#a:b:c#)"), vec![pragma(Some("a"), "b", ":c")]);
    assert!(error("(# #)").contains("pragma names itself"));
    assert!(error("(# p").contains("pragma is not closed"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.2.3]
/// line ends are normalized before the text is read: CR LF and CR alone
/// to LF; by XML 1.1's rules NEL and LINE SEPARATOR too, which XML 1.0's
/// keep.
#[test]
fn line_ends_are_normalized() {
    assert_eq!(tokens("'a\r\nb\rc'"), vec![string("a\nb\nc")]);
    assert_eq!(tokens_by("'a\u{85}b\u{2028}c\r\u{85}d'", Version::V11).unwrap(), vec![string("a\nb\nc\nd")]);
    assert_eq!(tokens_by("'a\u{85}b\u{2028}c'", Version::V10).unwrap(), vec![string("a\u{85}b\u{2028}c")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.2.1
/// \[157\]] every character of a query is a `Char` of the version whose
/// rules apply, and an error says where it is, by line and column.
#[test]
fn characters_are_chars_and_errors_say_where() {
    assert!(Lexer::new("'\u{0}'", Version::V11).unwrap_err().to_string().contains("#x0 is no Char of XML 1.1"));
    assert!(Lexer::new("'\u{1}'", Version::V10).unwrap_err().to_string().contains("no Char of XML 1.0"));
    assert!(Lexer::new("'\u{1}'", Version::V11).is_ok());
    assert!(error("1\n  2 #").contains("at line 2, column 5"));
    assert!(error("\r\n#").contains("at line 2, column 1"));
}
