//! Tests of XQuery regular expressions: what is one, and what matches.

use super::*;

fn matches(pattern: &str, flag: &str, text: &str) -> bool {
    Regex::new(pattern, flag).unwrap().is_match(text).unwrap()
}

fn state(pattern: &str, flag: &str) -> String {
    Regex::new(pattern, flag).map_or_else(|e| e.sqlstate().as_str().to_owned(), |_| String::new())
}

fn spans(pattern: &str, text: &str) -> Vec<Option<(usize, usize)>> {
    let r = Regex::new(pattern, "").unwrap();
    let chars: Vec<char> = text.chars().collect();
    let found = r.find_at(&chars, 0).unwrap().unwrap();
    (0..=r.groups()).map(|g| found.get(g)).collect()
}

/// Verifies: [EXTENSION] [F&O 3.1 §5.6.3] fn:matches's examples: a
/// substring matches unless `^` or `$` anchors the pattern; `s` lets `.`
/// match a newline, `m` anchors at each line, `i` ignores case.
#[test]
fn the_examples_of_fn_matches() {
    assert!(matches("bra", "", "abracadabra"));
    assert!(matches("^a.*a$", "", "abracadabra"));
    assert!(!matches("^bra", "", "abracadabra"));
    let poem = "\nKaum hat dies der Hahn gesehen,\nFängt er auch schon an zu krähen:\nKikeriki! Kikikerikih!!\n";
    assert!(!matches("Kaum.*krähen", "", poem));
    assert!(matches("Kaum.*krähen", "s", poem));
    assert!(matches("^Kaum.*gesehen,$", "m", poem));
    assert!(!matches("^Kaum.*gesehen,$", "", poem));
    assert!(matches("kiki", "i", poem));
}

/// Verifies: [EXTENSION] [F&O 3.1 §5.6.2] the flags' examples: `x` drops
/// white space outside a class; `q` makes every character itself, beside
/// `i` too.
#[test]
fn the_examples_of_the_flags() {
    assert!(matches("hello world", "x", "helloworld"));
    assert!(!matches("hello[ ]world", "x", "helloworld"));
    assert!(matches("hello\\ sworld", "x", "hello world"));
    assert!(!matches("hello world", "x", "hello world"));
    assert!(!matches(".*", "q", "abcd"));
    assert!(matches("B. OBAMA", "iq", "Mr. B. Obama"));
    assert!(matches("[", "q", "a[b"));
}

/// Verifies: [EXTENSION] [F&O 3.1 §5.6.2] under `i` a character matches
/// its case variants, a range too, before a negation or subtraction; a
/// back-reference compares case-blind; a category stays as it is.
#[test]
fn case_insensitive_matching() {
    assert!(matches("z", "i", "Z"));
    assert!(matches("^[A-Z]$", "i", "k") && matches("^[A-Z]$", "i", "\u{212A}"));
    assert!(matches("^[A-Z-[IO]]$", "i", "b") && !matches("^[A-Z-[IO]]$", "i", "i") && !matches("^[A-Z-[IO]]$", "i", "o"));
    assert!(!matches("^[^Q]$", "i", "q") && matches("^[^Q]$", "i", "r"));
    for word in ["Mum", "mom", "Dad", "DUD"] {
        assert!(matches("^([md])[aeiou]\\1$", "i", word), "{word}");
    }
    assert!(!matches("^([md])[aeiou]\\1$", "", "Mum"));
    assert!(!matches("\\p{Lu}", "i", "a"));
    assert!(matches("STRASSE", "i", "strasse") && matches("ß", "i", "ẞ"));
}

/// Patterns no XQuery regular expression.
const REFUSED: [&str; 35] = [
    "[",
    "(",
    ")",
    "]",
    "{",
    "}",
    "\\",
    "*a",
    "a**",
    "a|*",
    "a{2,1}",
    "a{",
    "a{x}",
    "a{,2}",
    "\\b",
    "\\0",
    "[]",
    "[^]",
    "[--z]",
    "[a--]",
    "[z-a]",
    "[a-\\d]",
    "[[a]",
    "[\\1]",
    "\\1",
    "(a\\1)",
    "(?=a)",
    "(?a)",
    "\\p{Xx}",
    "\\p{Lx}",
    "\\p{Cs}",
    "\\p{IsNoSuchBlock}",
    "\\p{L",
    "\\pL",
    "\\p{Is}",
];

/// Patterns XQuery regular expressions, near those refused.
const TAKEN: [&str; 21] = [
    "",
    "[a-]",
    "[-a]",
    "[a-z-[aeiou]]",
    "[^^]",
    "[a^]",
    "a{0}",
    "a{2,}",
    "a{2,3}?",
    "a??",
    "(?:a)",
    "\\p{IsBasicLatin}",
    "\\p{L}",
    "\\P{Nd}",
    "[\\p{L}-[\\p{Lu}]]",
    "^*",
    "\\$",
    "[\\--z]",
    "[a\\-z]",
    "a|",
    "()",
];

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.28 GR4; F&O 3.1 §5.6.1;
/// XSD 1.1 Part 2 §G] patterns the grammar refuses are SQLSTATE 2201S:
/// unbalanced, a quantifier with no atom or after another, a quantity
/// whose most is less than its least, an unknown escape, an empty class,
/// a hyphen beginning a range, a range reversed, a back-reference to no
/// group or within its own, a category or block that is none.
#[test]
fn patterns_that_are_none_9_28() {
    for p in REFUSED {
        assert_eq!(state(p, ""), "2201S", "{p}");
    }
    for p in TAKEN {
        assert_eq!(state(p, ""), "", "{p}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.28 GR4, GR5] a flag
/// that is none is SQLSTATE 2201T, unless the pattern is none too, which
/// is reported first.
#[test]
fn a_pattern_is_judged_before_its_flag_9_28() {
    assert_eq!(state("a", "z"), "2201T");
    assert_eq!(state("[", "z"), "2201S");
    assert_eq!(state("[", "q"), "");
}

/// Verifies: [EXTENSION] [F&O 3.1 §5.6.1.2, §5.6.1.3; ISO/IEC 9075-2:2023
/// §9.28 GR10, NOTE 461] the match is the leftmost, its alternatives and
/// greed as preferred, a reluctant quantifier matching as few as it can; a
/// group that matched nothing has no position; a group repeated keeps its
/// last capture; one that captures nothing is not counted.
#[test]
fn captures_9_28() {
    assert_eq!(spans("(a)|(b)", "a"), [Some((0, 1)), Some((0, 1)), None]);
    assert_eq!(spans("(a|ab)(c|bcd)(d*)", "abcd"), [Some((0, 4)), Some((0, 1)), Some((1, 4)), Some((4, 4))]);
    assert_eq!(spans("a+", "baaa"), [Some((1, 4))]);
    assert_eq!(spans("a+?", "baaa"), [Some((1, 2))]);
    assert_eq!(spans("(a|b)+", "abba"), [Some((0, 4)), Some((3, 4))]);
    assert_eq!(spans("(?:a)(b)", "ab"), [Some((0, 2)), Some((1, 2))]);
    assert_eq!(spans("a{2,3}", "aaaa"), [Some((0, 3))]);
    assert_eq!(spans("a{2,3}?", "aaaa"), [Some((0, 2))]);
    assert_eq!(spans("x*", "ab"), [Some((0, 0))]);
}

/// Verifies: [EXTENSION] [F&O 3.1 §5.6.1.4] a back-reference matches what
/// its group captured, nothing if it captured nothing; a second digit is
/// its own only if that many groups were opened before it; a loop that
/// matched nothing is not repeated.
#[test]
fn back_references() {
    assert!(matches("^('|\").*\\1$", "", "'abc'") && !matches("^('|\").*\\1$", "", "'abc\""));
    assert!(matches("^(b)?a\\1$", "", "a"));
    assert!(matches("^(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)\\10$", "", "abcdefghijj"));
    assert!(matches("^(a)\\10$", "", "aa0") && !matches("^(a)\\10$", "", "aa"));
    assert!(!matches("^(x*)*\\1y$", "", "b") && matches("^(x*)+\\1y$", "", "y"));
    let r = Regex::new("(a)(b)\\2", "").unwrap();
    let text: Vec<char> = "xabb".chars().collect();
    assert_eq!(r.find_at(&text, 0).unwrap().unwrap().get(0), Some((1, 4)));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.28 GR9a i] `.` matches no
/// newline of UTS #18 RL1.6 but under `s`, which takes CRLF as one
/// character; `^` and `$` under `m` are at each line, never between CR and
/// LF, `^` not after a newline ending the string; without `m` at the
/// string's ends alone.
#[test]
fn newlines_9_28() {
    for newline in ["\n", "\r", "\u{B}", "\u{C}", "\u{85}", "\u{2028}", "\u{2029}"] {
        assert!(!matches("^.$", "", newline) && matches("^.$", "s", newline), "{newline:?}");
    }
    assert!(matches("^.$", "s", "\r\n") && matches("a.b", "s", "a\r\nb") && !matches("a..b", "s", "a\r\nb"));
    for text in ["a\nb", "a\u{2028}b", "a\rb", "a\r\nb", "a\u{85}b"] {
        assert!(matches("^b", "m", text) && matches("a$", "m", text) && !matches("^b", "", text), "{text:?}");
    }
    assert!(!matches("^$", "m", "a\r\nb") && matches("^$", "m", "a\n\rb"));
    assert!(!matches("^$", "m", "a\n") && matches("a$", "m", "a\n") && !matches("a$", "", "a\n"));
    assert!(!matches("^\\n", "m", "a\r\nb") && !matches("\\n$", "m", "a\n") && !matches("\\n^", "m", "a\n"));
    assert!(matches("^$", "", ""));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.28 GR9a ii, iii] `\s` is a
/// space, a tab or a newline sequence, CRLF one; `\S` any one character no
/// `\s`.
#[test]
fn spaces_9_28() {
    assert!(matches("^\\s$", "", "\r\n") && matches("^\\s$", "", "\u{2029}") && matches("^\\s$", "", "\t"));
    assert!(matches("^\\s\\n$", "", "\r\n"), "CR alone, as the LF follows");
    assert!(!matches("\\S", "", "\u{85}") && matches("^\\S$", "", "a"));
    assert!(matches("^[\\s]$", "", "\u{B}") && !matches("^[\\s]$", "", "\r\n"));
}

/// Verifies: [EXTENSION] [XSD 1.1 Part 2 §G.4.2] categories, blocks and the
/// multi-character escapes hold Unicode 13.0's characters: `\d` any
/// decimal digit, `\w` none of punctuation, separators and others, `\i`
/// and `\c` XML's name characters.
#[test]
fn character_class_escapes() {
    assert!(matches("^\\p{Lu}$", "", "Ω") && !matches("^\\p{Lu}$", "", "ω"));
    assert!(matches("^\\d$", "", "\u{663}") && !matches("^\\D$", "", "7"));
    assert!(matches("^\\w$", "", "é") && !matches("^\\w$", "", "!") && matches("^\\W$", "", " "));
    assert!(matches("^\\p{IsGreek}$", "", "α") && matches("^\\p{IsGreekandCoptic}$", "", "α") && !matches("\\p{IsGreek}", "", "a"));
    assert!(matches("^\\P{IsBasicLatin}+$", "", "αβ") && matches("^\\i\\c*$", "", "_a-1") && !matches("^\\i", "", "1"));
    assert!(matches("^[\\p{L}-[\\p{Lu}]]+$", "", "abc") && !matches("^[\\p{L}-[\\p{Lu}]]+$", "", "aBc"));
    assert!(matches("^\\p{N}$", "", "½") && matches("^\\p{Sc}$", "", "€") && matches("^\\p{Cn}$", "", "\u{378}"));
}

/// Verifies: [EXTENSION] [XSD 1.1 Part 2 §G.4.1] a class's hyphen is a
/// range between two characters, a character before ']' or "-[", and a
/// subtraction before '['; a first '^' negates, a later one is itself.
#[test]
fn character_groups() {
    assert!(matches("^[a-]+$", "", "a-") && matches("^[-a]+$", "", "-a"));
    assert!(matches("^[a-c-[b]]+$", "", "ac") && !matches("^[a-c-[b]]$", "", "b"));
    assert!(matches("^[a--[a]]$", "", "-"));
    assert!(matches("^[^^]$", "", "a") && !matches("^[^^]$", "", "^") && matches("^[a^]+$", "", "^a"));
    assert!(matches("^[\\--/]+$", "", "-./") && matches("^[\\n\\]]+$", "", "]\n"));
    assert!(matches("^[^a-[b]]$", "", "c") && !matches("^[^a-[b]]$", "", "b"));
}

/// Verifies: [DESIGN] RULES.md D18, D61 the limits: a program past
/// `MAX_INSTRUCTIONS`, groups nested past their limit and a search with
/// back-references past `MAX_STEPS` are SQLCODE -900; a pattern that
/// would take a backtracking matcher exponential time but has no
/// back-reference is matched at once.
#[test]
fn limits() {
    let code = |p: &str| Regex::new(p, "").err().map(|e| e.code);
    assert_eq!(code("a{9000}"), Some(Code::Unsupported));
    assert_eq!(code("a{99999999999}"), Some(Code::Unsupported));
    assert_eq!(code(&format!("{}a{}", "(".repeat(70), ")".repeat(70))), Some(Code::Unsupported));
    assert_eq!(code(&format!("{}a{}", "(".repeat(30), ")".repeat(30))), None);
    let hostile = Regex::new("(a|aa)*\\1c", "").unwrap();
    assert_eq!(hostile.is_match(&"a".repeat(60)).err().map(|e| e.code), Some(Code::Unsupported));
    assert!(!matches("^(a*)*b$", "", &"a".repeat(5000)));
    assert!(!matches("^(a|a)*b$", "", &"a".repeat(5000)));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.28 GR11] a search from a
/// position finds the first match at or after it, `^` still the string's
/// start.
#[test]
fn searches_from_a_position() {
    let r = Regex::new("^a|b", "").unwrap();
    let text: Vec<char> = "aab".chars().collect();
    assert_eq!(r.find_at(&text, 0).unwrap().unwrap().get(0), Some((0, 1)));
    assert_eq!(r.find_at(&text, 1).unwrap().unwrap().get(0), Some((2, 3)));
    assert!(r.find_at(&text, 3).unwrap().is_none());
}
