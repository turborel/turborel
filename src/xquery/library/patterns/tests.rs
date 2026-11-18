//! Tests of the functions that use pattern matching.

use super::super::testing::{call, code, empty, s, text, texts};

/// The poem of [XQueryFO §7.6.2.1].
const POEM: &str =
    "\nKaum hat dies der Hahn gesehen,\nFängt er auch schon an zu krähen:\n«Kikeriki! Kikikerikih!!»\nTak, tak, tak! - da kommen sie.\n";

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.6.1.1,
/// §7.6.2.1] the examples of `fn:matches` and of the flags: a substring
/// matched unless anchored; `.` no line feed but under `s`; `^` and `$` of
/// each line under `m`; `i` caseless; `x` stripping white space but in a
/// class.
#[test]
fn matches_examples() {
    let truth = |args: &[crate::xml::Sequence]| text(call("matches", args));
    assert_eq!(truth(&[s("abracadabra"), s("bra")]), "true");
    assert_eq!(truth(&[s("abracadabra"), s("^a.*a$")]), "true");
    assert_eq!(truth(&[s("abracadabra"), s("^bra")]), "false");
    assert_eq!(truth(&[s(POEM), s("Kaum.*krähen")]), "false");
    assert_eq!(truth(&[s(POEM), s("Kaum.*krähen"), s("s")]), "true");
    assert_eq!(truth(&[s(POEM), s("^Kaum.*gesehen,$"), s("m")]), "true");
    assert_eq!(truth(&[s(POEM), s("^Kaum.*gesehen,$")]), "false");
    assert_eq!(truth(&[s(POEM), s("kiki"), s("i")]), "true");
    assert_eq!(truth(&[s("helloworld"), s("hello world"), s("x")]), "true");
    assert_eq!(truth(&[s("helloworld"), s("hello[ ]world"), s("x")]), "false");
    assert_eq!(truth(&[s("hello world"), s("hello\\ sworld"), s("x")]), "true");
    assert_eq!(truth(&[s("hello world"), s("hello world"), s("x")]), "false");
    assert_eq!(truth(&[empty(), s("^$")]), "true");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.6.1] a
/// newline is a line feed alone: `.` matches a carriage return, `$` of a
/// line does not precede one, and `\s` is XML Schema's four.
#[test]
fn newlines_are_line_feeds() {
    let truth = |args: &[crate::xml::Sequence]| text(call("matches", args));
    assert_eq!(truth(&[s("a\rb"), s("^a.b$")]), "true");
    assert_eq!(truth(&[s("a\u{85}b"), s("a.b")]), "true");
    assert_eq!(truth(&[s("a\r\nb"), s("a$"), s("m")]), "false");
    assert_eq!(truth(&[s("a\nb"), s("a$"), s("m")]), "true");
    assert_eq!(truth(&[s("a\u{2028}b"), s("a\\sb")]), "false");
    assert_eq!(truth(&[s("a\r\nb"), s("a\\s\\sb")]), "true");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.6.1,
/// §7.6.2] a flag none of `smix`, `q` among them, err:FORX0001; a pattern
/// that is none err:FORX0002, of `(?:` too, which XQuery 1.0 does not
/// have.
#[test]
fn flags_and_patterns_refused() {
    assert_eq!(code(call("matches", &[s("a"), s("a"), s("q")])), "FORX0001");
    assert_eq!(code(call("matches", &[s("a"), s("a"), s("g")])), "FORX0001");
    assert_eq!(code(call("matches", &[s("a"), s("(a")])), "FORX0002");
    assert_eq!(code(call("matches", &[s("a"), s("(?:a)")])), "FORX0002");
    assert_eq!(code(call("matches", &[s("a"), s("[a")])), "FORX0002");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.6.3.1]
/// the examples of `fn:replace`: greedy and reluctant matches, `$N` of a
/// group, the first alternative preferred; a pattern matching the
/// zero-length string err:FORX0003; a bad replacement err:FORX0004.
#[test]
fn replace_examples() {
    let replaced = |input: &str, pattern: &str, with: &str| text(call("replace", &[s(input), s(pattern), s(with)]));
    let cases = [
        (replaced("abracadabra", "bra", "*"), "a*cada*"),
        (replaced("abracadabra", "a.*a", "*"), "*"),
        (replaced("abracadabra", "a.*?a", "*"), "*c*bra"),
        (replaced("abracadabra", "a", ""), "brcdbr"),
        (replaced("abracadabra", "a(.)", "a$1$1"), "abbraccaddabbra"),
        (replaced("AAAA", "A+", "b"), "b"),
        (replaced("AAAA", "A+?", "b"), "bbbb"),
        (replaced("darted", "^(.*?)d(.*)$", "$1c$2"), "carted"),
        (replaced("abcd", "(ab)|(a)", "[1=$1][2=$2]"), "[1=ab][2=]cd"),
        (replaced("a", "a", "\\$\\\\"), "$\\"),
        (text(call("replace", &[s("AbA"), s("a"), s("x"), s("i")])), "xbx"),
        (text(call("replace", &[empty(), s("a"), s("x")])), ""),
        (code(call("replace", &[s("abracadabra"), s(".*?"), s("$1")])), "FORX0003"),
        (code(call("replace", &[s("a"), s("a"), s("$")])), "FORX0004"),
        (code(call("replace", &[s("a"), s("a"), s("\\x")])), "FORX0004"),
    ];
    for (got, want) in cases {
        assert_eq!(got, want);
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.6.4.1]
/// the examples of `fn:tokenize`: zero-length tokens at the ends and
/// between adjacent separators, the first alternative preferred, the
/// empty sequence of the zero-length string; a pattern matching the
/// zero-length string err:FORX0003.
#[test]
fn tokenize_examples() {
    let tokens = |input: &str, pattern: &str| texts(&call("tokenize", &[s(input), s(pattern)]).unwrap());
    assert_eq!(tokens("The cat sat on the mat", "\\s+"), ["The", "cat", "sat", "on", "the", "mat"]);
    assert_eq!(tokens("1, 15, 24, 50", ",\\s*"), ["1", "15", "24", "50"]);
    assert_eq!(tokens("1,15,,24,50,", ","), ["1", "15", "", "24", "50", ""]);
    assert_eq!(tokens("abracadabra", "(ab)|(a)"), ["", "r", "c", "d", "r", ""]);
    let html = call("tokenize", &[s("Some unparsed <br> HTML <BR> text"), s("\\s*<br>\\s*"), s("i")]).unwrap();
    assert_eq!(texts(&html), ["Some unparsed", "HTML", "text"]);
    assert!(call("tokenize", &[s(""), s(",")]).unwrap().items().is_empty());
    assert!(call("tokenize", &[empty(), s(",")]).unwrap().items().is_empty());
    assert_eq!(code(call("tokenize", &[s("abba"), s(".?")])), "FORX0003");
}
