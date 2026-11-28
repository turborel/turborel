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
    // Not anchored at the string's start, so it keeps its marks (RULES.md D1312).
    let regex = Regex::new("(x|xa)[^c]+c", "").unwrap();
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

/// The bits `first` up to `end` of `marks` set one at a time, as the
/// matcher marks a position at a time: whether none was set before, the
/// marking stopped at the first that was.
fn marked_one_by_one(marks: &mut [u64], first: usize, end: usize) -> bool {
    for bit in first..end {
        let Some(word) = marks.get_mut(bit.div_euclid(WORD)) else { return false };
        if *word & (1 << (bit % WORD)) != 0 {
            return false;
        }
        *word |= 1 << (bit % WORD);
    }
    true
}

/// Verifies: [DESIGN] (RULES.md D1311) the marks of a run of positions
/// set a word at a time are those set one position at a time, and say
/// the same of whether one was set before: of every run of up to 70
/// positions within three words and a little past them, begun and ended
/// at a word's edge and within one, over marks of none set, of every one
/// set alone, and of a few set here and there.
#[test]
fn marks_set_a_word_at_a_time_are_those_set_one_by_one() {
    let mut befores: Vec<[u64; 3]> = vec![[0; 3], [u64::MAX; 3], [1 << 63, 1, 0], [0, 1 << 20 | 1 << 40, 1 << 5]];
    befores.extend((0..192_usize).map(|bit| {
        let mut one = [0_u64; 3];
        one[bit.div_euclid(WORD)] = 1 << (bit % WORD);
        one
    }));
    for before in befores {
        for (first, end) in (0..200_usize).flat_map(|first| (first..202.min(first + 70)).map(move |end| (first, end))) {
            let (mut by_words, mut one_by_one) = (before, before);
            let fresh = marked(&mut by_words, first, end);
            assert_eq!(fresh, marked_one_by_one(&mut one_by_one, first, end), "{before:?} from {first} to {end}");
            assert_eq!(by_words, one_by_one, "{before:?} from {first} to {end}");
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D1311, D1188) marked only where two ways
/// can meet, the matcher that marks still follows each instruction at
/// each position once at most: over the corpus, a search from every
/// start follows no more instructions than the program has times the
/// program and the positions it reads together, the bound of a program
/// of few ways too (RULES.md D1312); and of a pattern whose ways double at every
/// character, of a text with no match, it follows a number within that
/// product where the ways number a power of the text's length.
#[test]
fn marked_where_ways_meet_it_follows_each_instruction_at_a_position_once() {
    let doubling = Regex::new("(a|a)*b", "").unwrap();
    let run: Vec<char> = "a".repeat(18).chars().collect();
    let (mut counted, read) = (Scratch::default(), positions(&doubling.program, run.len(), 0).unwrap());
    assert!(!search(&doubling.program, run.as_slice(), (0, read), true, &mut counted).unwrap());
    assert!(counted.steps <= doubling.program.insts.len() * read, "{} steps", counted.steps);
    for (pattern, flag) in PATTERNS {
        let regex = Regex::new(pattern, flag).unwrap();
        for text in TEXTS {
            let chars: Vec<char> = text.chars().collect();
            let (mut kept, reads) = (Scratch::default(), positions(&regex.program, chars.len(), 0).unwrap());
            search(&regex.program, chars.as_slice(), (0, reads), true, &mut kept).unwrap();
            let bound = regex.program.insts.len() * (regex.program.insts.len() + reads);
            assert!(kept.steps <= bound, "{pattern:?} in {text:?}: {} steps", kept.steps);
        }
    }
}

/// Where each character of `string` begins, and its end: the positions of
/// a text read by its octets, by the positions of its characters.
fn octet_places(string: &str) -> Vec<usize> {
    string.char_indices().map(|(at, _)| at).chain([string.len()]).collect()
}

/// Verifies: [DESIGN] (RULES.md D1311) of a text read by its octets the
/// matcher that marks finds what it finds of the text's characters, each
/// position that of the same character: through a loop ended by one
/// octet and one of a class, in runs longer than a word of marks, of a
/// run that holds characters of several octets, entered a second way at
/// a position the first passed, with a match and with none, the search
/// begun at each of the first positions and cut short of the text's end.
#[test]
fn a_text_read_by_its_octets_is_matched_as_its_characters_are() {
    let patterns = ["^(a|ab)([^/]+)/", "(a|aa)[a-c]+d", "[^/]+/", "(é|éb)[^/]+/(x)?", "[^,]*,[^/]+/", "[a-cé]+d", "(.)[^é]+é"];
    let long = |run: &str, end: &str| format!("ab{}{end}", run.repeat(40));
    let texts = [
        long("bc", "/"),
        long("bc", ""),
        long("cé", "/x"),
        long("c,", "d/"),
        format!("é{}", long("b", "/")),
        long("a", "d"),
        String::new(),
    ];
    let mut kept = Scratch::default();
    for (pattern, string) in patterns.iter().flat_map(|pattern| texts.iter().map(move |string| (pattern, string))) {
        let (regex, chars, places) = (Regex::new(pattern, "").unwrap(), string.chars().collect::<Vec<char>>(), octet_places(string));
        for (from, cut) in [(0, 0), (1, 0), (2, 0), (0, 1), (0, 30)] {
            let (Some(from), Some(end)) = (Some(from).filter(|from| *from <= chars.len()), chars.len().checked_sub(cut)) else { continue };
            let Some(reads) = end.checked_sub(from).map(|n| n + 1) else { continue };
            let of_chars = found(&regex.program, chars.as_slice(), (from, reads), true, &mut kept).unwrap();
            let octets = (places[from], places[end] - places[from] + 1);
            let lying = super::super::Utf8::of(string);
            let of_octets = search(&regex.program, &lying, octets, true, &mut kept).unwrap().then(|| kept.slots().to_vec());
            let wanted = of_chars.map(|slots| slots.iter().map(|slot| slot.map(|at| places[at])).collect::<Vec<_>>());
            assert_eq!(of_octets, wanted, "{pattern:?} in {string:?} from {from}, cut {cut}");
            if let Some(ascii) = super::super::Ascii::of(string) {
                let of_ascii = search(&regex.program, &ascii, octets, true, &mut kept).unwrap().then(|| kept.slots().to_vec());
                assert_eq!(of_ascii, wanted, "{pattern:?} in {string:?} of ASCII from {from}, cut {cut}");
            }
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D1312) a program has few ways if every
/// match of it begins at the string's start, its loops are all run
/// through or skipped as the rest of the text, and two to the power of
/// its forks, times its loops and one, is within its instructions; not
/// one that may begin anywhere, has a loop of another kind, or forks too
/// often. Of such a program the matcher keeps no mark and finds what the
/// matcher of threads finds, from every position, where a second way
/// meets the first in a loop too (`wwwx`), read by its characters and by
/// its octets, its straight runs compared at once.
#[test]
fn a_program_of_few_ways_keeps_no_mark_and_finds_the_same() {
    let few = [("^https?://(www\\.)?([^/]+)/.*$", "s"), ("^(a|ab)(b|bc)[^/]+/", ""), ("^(x|xa)[^c]+c(d)?", ""), ("^abc", "")];
    let texts = [
        "http://wwwx.test/a",
        "http://www.a.test/b",
        "https://www./",
        "http://www",
        "http:/a/",
        "abbc/",
        "abc//",
        "ab",
        "xaac",
        "xaaacd",
        "",
    ];
    let mut kept = Scratch::default();
    for (pattern, flag) in few {
        let regex = Regex::new(pattern, flag).unwrap();
        assert!(regex.program.passed.few_ways, "{pattern:?}");
        for text in texts {
            let chars: Vec<char> = text.chars().collect();
            for from in 0..=chars.len() {
                let reads = positions(&regex.program, chars.len(), from).unwrap();
                let ours = found(&regex.program, chars.as_slice(), (from, reads), true, &mut kept).unwrap();
                assert_eq!(
                    ours,
                    pike::search(&regex.program, chars.as_slice(), from, true).unwrap(),
                    "{pattern:?} in {text:?} from {from}"
                );
                let lying = super::super::Utf8::of(text);
                let of_octets = search(&regex.program, &lying, (from, reads), true, &mut kept).unwrap().then(|| kept.slots().to_vec());
                assert_eq!(of_octets, ours, "{pattern:?} in {text:?} from {from}, by its octets");
                assert!(kept.marks.is_empty(), "{pattern:?}: no mark kept");
            }
        }
    }
    for (pattern, flag) in [("https?://x", ""), ("^(a*)*b", ""), ("^a+a", ""), ("^a.*$", ""), ("^(a|b)(a|b)(a|b)(a|b)(a|b)(a|b)(a|b)c", "")]
    {
        assert!(!Regex::new(pattern, flag).unwrap().program.passed.few_ways, "{pattern:?}");
    }
}
