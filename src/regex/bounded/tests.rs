//! Unit tests of the matcher that marks each instruction at each position
//! (RULES.md D1188): what it finds against what the matcher of threads
//! finds, and which programs and texts it is given.

use super::super::{Matcher, Regex, pike};
use super::*;

/// What `search` finds, as the matcher of threads gives it. Of a match
/// that is the slots it left in `kept`, and of none it is none.
fn found(program: &Program, text: &[char], at: (usize, usize), capture: bool, kept: &mut Scratch) -> Result<Option<Slots>> {
    Ok(search(program, text, at, capture, kept)?.then(|| kept.slots().to_vec()))
}

/// Patterns of every kind of instruction: alternation in its order, greedy
/// and reluctant quantifiers, counted ones, groups nested and repeated,
/// iterations that match nothing, anchors of the string and of lines,
/// classes, the wildcard and CRLF.
pub(in crate::regex) const PATTERNS: [(&str, &str); 36] = [
    ("a", ""),
    ("abc|ab|a", ""),
    ("a|ab|abc", ""),
    ("(a|ab)(c|bcd)(d*)", ""),
    ("a*", ""),
    ("a*?", ""),
    ("(a*)*", ""),
    ("(a*)+b", ""),
    ("(a|b)*?c", ""),
    ("(a+)(b+)?", ""),
    ("(a{2,3}){1,2}", ""),
    ("x*(y?)z??", ""),
    ("^a", ""),
    ("a$", ""),
    ("^$", ""),
    ("^a.*a$", "s"),
    ("^b", "m"),
    ("b$", "m"),
    ("[a-c]+[^a-c]", ""),
    ("\\s+\\S", ""),
    (".", ""),
    (".+", "s"),
    ("(.*)(b)", ""),
    ("(.*?)(b)", ""),
    ("((a)|(b))+", ""),
    ("()", ""),
    ("(|a)+", ""),
    ("^https?://(www\\.)?([^/]+)/.*$", ""),
    ("A", "i"),
    ("(a|ab)*c|.*", ""),
    ("x(|a)", ""),
    ("a(bc)?", ""),
    ("(a*)b", ""),
    ("a+a", ""),
    ("[ab]+?\\r", ""),
    ("(a|aa)[^b]+\\r", ""),
];

/// Texts of what the patterns read: runs of one letter, mixes, newlines of
/// each kind, and none.
pub(in crate::regex) const TEXTS: [&str; 19] = [
    "",
    "a",
    "b",
    "ab",
    "abc",
    "abcd",
    "aaa",
    "aaab",
    "aabcbcd",
    "xyz",
    "a\nb",
    "a\r\nb\n",
    " \t a",
    "http://www.example.test/a/b",
    "https://example.test/",
    "ccbaAab aab",
    "xa",
    "ya",
    "aba",
];

/// Verifies: [DESIGN] RULES.md D1188 [F&O 3.1 §5.6]: over each pattern and
/// text, from the text's start and from each position after, with what its
/// groups captured and without, the matcher that marks finds what the
/// matcher of threads finds: the same leftmost match, of the same
/// preference among alternatives and quantifiers, each group's span the
/// same, and none where it finds none; what it keeps from one search to
/// the next, its marks among them, changes nothing a later search finds;
/// and a program anchored at the string's start is tried from there
/// alone, one anchored at each line's from every start.
#[test]
fn it_finds_what_the_matcher_of_threads_finds() {
    let mut kept = Scratch::default();
    for (pattern, flag) in PATTERNS {
        let regex = Regex::new(pattern, flag).unwrap();
        for text in TEXTS {
            let chars: Vec<char> = text.chars().collect();
            for from in 0..=chars.len() {
                let reads = positions(&regex.program, chars.len(), from).unwrap();
                for capture in [true, false] {
                    let ours = found(&regex.program, chars.as_slice(), (from, reads), capture, &mut kept).unwrap();
                    let theirs = pike::search(&regex.program, chars.as_slice(), from, capture).unwrap();
                    assert_eq!(ours, theirs, "{pattern:?} under {flag:?} in {text:?} from {from}, capture {capture}");
                }
                // Given where the match ends, it reads no further and finds the same.
                let theirs = pike::search(&regex.program, chars.as_slice(), from, true).unwrap();
                if let Some(end) = theirs.as_ref().and_then(|slots| slots[1]) {
                    let within = positions(&regex.program, end, from).unwrap();
                    assert_eq!(
                        found(&regex.program, chars.as_slice(), (from, within), true, &mut kept).unwrap(),
                        theirs,
                        "{pattern:?} {text:?} to {end}"
                    );
                }
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1188, D61: a program with a back-reference
/// never runs on the matcher that marks, whatever the text, as what a
/// back-reference matches depends on what was captured; one without runs
/// on it while its marks are within `MARKS_MOST`, the text's positions
/// from where the search begins counted, and on the matcher of threads
/// past them; and both give the same answer either side of that bound.
///
/// MC/DC of `back_references` and of the marks within the bound: a
/// back-reference with few marks, none with few, none with too many.
#[test]
fn it_is_given_programs_of_no_back_reference_and_texts_within_its_marks() {
    let again = Regex::new("(a)\\1", "").unwrap();
    assert_eq!(again.matcher(2, 0), Matcher::Backtrack);
    assert_eq!(again.matcher(usize::MAX, 0), Matcher::Backtrack);
    let plain = Regex::new("a+b", "").unwrap();
    let insts = plain.program.insts.len();
    let most = MARKS_MOST.div_euclid(insts) - 1;
    assert_eq!(plain.matcher(most, 0), Matcher::Bounded(most + 1));
    assert_eq!(plain.matcher(most + 1, 0), Matcher::Pike);
    assert_eq!(plain.matcher(most + 1, 1), Matcher::Bounded(most + 1), "positions from where it begins");
    assert_eq!(positions(&plain.program, 3, 4), None, "a start past the end");
    for len in [most, most + 1] {
        let mut text = "a".repeat(len - 1);
        assert!(!plain.is_match(&text).unwrap());
        text.push('b');
        assert!(plain.is_match(&text).unwrap());
    }
}

/// Verifies: [DESIGN] RULES.md D1188: an instruction at a position is
/// followed once in a search, so a pattern whose ways to fail grow as a
/// power of the text's length is given up in a number of steps within the
/// product of the two; and the marks kept from one start to the next are
/// right, a later start finding the match an earlier one could not.
#[test]
fn it_follows_each_instruction_at_a_position_once() {
    let regex = Regex::new("(a|aa)*c", "").unwrap();
    let chars: Vec<char> = "a".repeat(2000).chars().collect();
    let reads = positions(&regex.program, chars.len(), 0).unwrap();
    assert_eq!(found(&regex.program, chars.as_slice(), (0, reads), true, &mut Scratch::default()).unwrap(), None);
    let later: Vec<char> = "ababac".chars().collect();
    let later_found = found(&Regex::new("(a)c", "").unwrap().program, later.as_slice(), (0, 7), true, &mut Scratch::default()).unwrap();
    assert_eq!(later_found, Some(vec![Some(4), Some(6), Some(4), Some(5)]));
}

/// Verifies: [DESIGN] RULES.md D1188 [ISO/IEC 9075-2:2023 §9.28 GR9a i]:
/// a program has a loop that takes the rest of any text if it ends with a
/// greedy loop of the wildcard of the flag `s` and then `$` of the whole
/// string, groups closing around them or not, and of XQuery 1.0's
/// wildcard too; not of a wildcard that stops at a newline, of `$` of
/// each line, of a reluctant loop, of a loop of a class, of anything
/// asked after the loop, nor of a branch. From such a loop the matcher
/// that marks goes to the string's end and marks no character between,
/// finds what the matcher of threads finds, and finds nothing where its
/// match is known to end before the string does.
#[test]
fn the_rest_of_a_text_is_taken_without_walking_it() {
    let has = [("a.*$", "s"), ("(a)(.+)$", "s"), ("^(.*)$", "s"), ("(a.*$)", "s")];
    let none = [(".*$", ""), (".*$", "sm"), (".*?$", "s"), (".*a$", "s"), (".*$|b", "s"), ("[ab]*$", "s"), (".*", "s"), ("a$|b.*$", "s")];
    for (pattern, flag) in has {
        assert!(Regex::new(pattern, flag).unwrap().program.passed.rest.is_some(), "{pattern:?} {flag:?}");
    }
    for (pattern, flag) in none {
        assert!(Regex::new(pattern, flag).unwrap().program.passed.rest.is_none(), "{pattern:?} {flag:?}");
    }
    assert!(Regex::xquery("a.*$", "s").unwrap().program.passed.rest.is_some());
    assert!(Regex::xquery("a.*$", "").unwrap().program.passed.rest.is_none());
    let regex = Regex::new("^a(.*)$", "s").unwrap();
    let text: Vec<char> = format!("a{}", "x\r\n".repeat(400)).chars().collect();
    let mut kept = Scratch::default();
    let reads = positions(&regex.program, text.len(), 0).unwrap();
    let whole = found(&regex.program, text.as_slice(), (0, reads), true, &mut kept).unwrap();
    assert_eq!(whole, pike::search(&regex.program, text.as_slice(), 0, true).unwrap());
    let marked: u32 = kept.marks.iter().map(|word| word.count_ones()).sum();
    assert!(marked < 16, "the characters between are not marked: {marked}");
    assert_eq!(found(&regex.program, text.as_slice(), (0, 5), true, &mut kept).unwrap(), None, "known to end before the end");
}

/// Verifies: [DESIGN] RULES.md D1190: a program runs through a greedy
/// loop of one instruction that matches a character where the next
/// character asked for after it, positions kept for groups between or
/// not, is one that instruction never matches. It does not where the
/// next may be one the loop takes, where the loop is reluctant or of
/// more than one instruction, of the wildcard that looks beside it, or
/// where an assertion, a fork or the match follows. Through such a loop
/// the matcher that marks keeps no fork to try and marks the loop's
/// fork at each position passed, so a second way into the loop stops at
/// once, and it finds what the matcher of threads finds, of a text with
/// a match and of one with none.
#[test]
fn a_loop_of_a_class_before_a_character_outside_it_is_run_through() {
    let runs = |pattern: &str, flag: &str| Regex::new(pattern, flag).unwrap().program.passed.runs.iter().filter(|run| **run).count();
    for (pattern, flag) in [("[^/]+/", ""), ("a*b", ""), ("(a+)b", ""), ("[ab]+c", ""), ("((a*))(b)", ""), ("[0-9]+\\.", "")] {
        assert_eq!(runs(pattern, flag), 1, "{pattern:?} {flag:?}");
    }
    assert_eq!(runs("a*b[^c]*c", ""), 2, "each loop of two");
    let none = [
        ("a+a", ""),
        ("[ab]+b", ""),
        ("a+?b", ""),
        ("(ab)+c", ""),
        ("a+", ""),
        ("a+$", ""),
        (".+a", "s"),
        ("a+(b|c)", ""),
        ("a{2,3}b", ""),
    ];
    for (pattern, flag) in none {
        assert_eq!(runs(pattern, flag), 0, "{pattern:?} {flag:?}");
    }
    let regex = Regex::new("^(x|xa)[^c]+c", "").unwrap();
    let mut kept = Scratch::default();
    for (end, what) in [("", "no match"), ("c", "a match")] {
        let text: Vec<char> = format!("x{}{end}", "a".repeat(2000)).chars().collect();
        let reads = positions(&regex.program, text.len(), 0).unwrap();
        let ours = found(&regex.program, text.as_slice(), (0, reads), true, &mut kept).unwrap();
        assert_eq!(ours, pike::search(&regex.program, text.as_slice(), 0, true).unwrap(), "{what}");
        assert_eq!(ours.is_some(), !end.is_empty(), "{what}");
    }
    let marked: u32 = kept.marks.iter().map(|word| word.count_ones()).sum();
    assert!(marked >= 2000, "the fork is marked at each position passed: {marked}");
    assert!(kept.stack.capacity() < 64, "no fork is kept a character: {}", kept.stack.capacity());
}
