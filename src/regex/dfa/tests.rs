//! Unit tests of the automaton made as texts are read (RULES.md D1188):
//! the end it finds against the matcher of threads', which programs have
//! one, and what a searcher that keeps it finds.

use super::super::bounded::tests::{PATTERNS, TEXTS};
use super::super::{Regex, pike};
use super::*;

/// Verifies: [DESIGN] RULES.md D1188 [F&O 3.1 §5.6]: over each pattern
/// that has an automaton and each text, from each position, one automaton
/// kept across all the texts, the end it finds is the end of the match
/// the matcher of threads finds, and none where that finds none; asked
/// only whether there is a match, it says so of the same texts; and a
/// searcher that keeps it finds the same match and the same groups as a
/// search of each text alone.
#[test]
fn it_ends_where_the_matcher_of_threads_does() {
    let mut with = 0_usize;
    for (pattern, flag) in PATTERNS {
        let regex = Regex::new(pattern, flag).unwrap();
        let mut searcher = regex.searcher();
        let mut automaton = Dfa::default();
        with += usize::from(fits(&regex.program));
        for text in TEXTS {
            let chars: Vec<char> = text.chars().collect();
            for from in 0..=chars.len() {
                let theirs = pike::search(&regex.program, chars.as_slice(), from, true).unwrap();
                let end = theirs.as_ref().map(|slots| slots[1].unwrap());
                if fits(&regex.program) {
                    let wanted = end.map_or(Scan::None, Scan::Ends);
                    assert_eq!(
                        automaton.scan(&regex.program, chars.as_slice(), from, false).unwrap(),
                        wanted,
                        "{pattern:?} {flag:?} {text:?} {from}"
                    );
                    let first = automaton.scan(&regex.program, chars.as_slice(), from, true).unwrap();
                    assert_eq!(first == Scan::None, end.is_none(), "{pattern:?} {flag:?} {text:?} {from}, whether");
                }
                let kept = searcher.find_in(chars.as_slice(), from).unwrap();
                assert_eq!(kept, regex.find_in(chars.as_slice(), from).unwrap(), "{pattern:?} {flag:?} {text:?} {from}, kept");
                assert_eq!(searcher.is_match_in(chars.as_slice()).unwrap(), regex.is_match(text).unwrap());
            }
        }
    }
    assert!(with >= 24, "most of the patterns have an automaton: {with}");
}

/// Verifies: [DESIGN] RULES.md D1188 [ISO/IEC 9075-2:2023 §9.28 GR9a i]:
/// a program with `^` or `$` of each line or with a back-reference has
/// no automaton, and a searcher of it finds what a search of each text
/// alone finds; one of the whole string's `^` and `$` has one, and so
/// has the wildcard of the flag `s`, which reads a CR before an LF as a
/// character of its own.
#[test]
fn programs_that_ask_more_of_a_position_have_none() {
    for (pattern, flag, has) in [("^a$", "", true), ("^a$", "m", false), ("a.b", "s", true), ("(a)\\1", "", false), ("a.b", "", true)] {
        let regex = Regex::new(pattern, flag).unwrap();
        assert_eq!(fits(&regex.program), has, "{pattern:?} {flag:?}");
        assert_eq!(regex.searcher().dfa.is_some(), has, "{pattern:?} {flag:?}");
        let mut searcher = regex.searcher();
        for text in ["a", "b\na\n", "a\r\nb", "aa", "axb", "a\rb", "a\r\r\nb", "a\nb"] {
            let chars: Vec<char> = text.chars().collect();
            assert_eq!(searcher.find_in(chars.as_slice(), 0).unwrap(), regex.find_in(chars.as_slice(), 0).unwrap(), "{pattern:?} {text:?}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1188: an automaton past `OCTETS_MOST`
/// octets of states gives up, the searcher drops it, and the search goes on
/// without it to the same answers; robustness: a state number past the
/// states is no state.
#[test]
fn past_its_states_it_gives_up_and_the_search_goes_on() {
    // Each count of `a` before the end is a state of its own.
    let regex = Regex::new("(a|b)*a(a|b){12}$", "").unwrap();
    let mut searcher = regex.searcher();
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut gave_up = false;
    for _ in 0..400_usize {
        let text: String = (0_usize..40_usize)
            .map(|_| {
                state ^= state << 13_u32;
                state ^= state >> 7_u32;
                state ^= state << 17_u32;
                if state & 1 == 0 { 'a' } else { 'b' }
            })
            .collect();
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(searcher.find_in(chars.as_slice(), 0).unwrap(), regex.find_in(chars.as_slice(), 0).unwrap(), "{text}");
        gave_up |= searcher.dfa.is_none();
    }
    assert!(gave_up, "the automaton outgrew its states");
    assert!(!Dfa::default().accepts(7));
}

/// Verifies: [DESIGN] RULES.md D1188: a state's threads are cut at the
/// first that matches, and the forks left to try behind it are dropped
/// with them, so they are not followed at the next position judged: after
/// a text that matches by its first alternative, a text with no match
/// still has none.
#[test]
fn the_ways_left_behind_a_match_are_dropped() {
    let regex = Regex::new("x(|$)", "").unwrap();
    let mut automaton = Dfa::default();
    let text = |t: &str| t.chars().collect::<Vec<char>>();
    assert_eq!(automaton.scan(&regex.program, text("x").as_slice(), 0, false).unwrap(), Scan::Ends(1));
    assert_eq!(automaton.scan(&regex.program, text("").as_slice(), 0, false).unwrap(), Scan::None);
    assert_eq!(automaton.scan(&regex.program, text("y").as_slice(), 0, false).unwrap(), Scan::None);
}

/// Verifies: [DESIGN] RULES.md D1188: a searcher asks its automaton
/// before a search for captures while at least half of the last window
/// of such searches found no match, and not while fewer did, the answers
/// the same either way; a search that asks only whether there is a match
/// is not counted.
///
/// MC/DC of the window's decision: a window of matches alone (not asked
/// after it), one of none alone (asked), and one of half and half (asked).
#[test]
fn a_searcher_asks_its_automaton_while_half_its_texts_have_no_match() {
    let regex = Regex::new("^a+(b)$", "").unwrap();
    let mut searcher = regex.searcher();
    let (yes, no): (Vec<char>, Vec<char>) = ("aab".chars().collect(), "aac".chars().collect());
    let asks = |s: &super::super::Searcher<'_>| format!("{:?}", s.found).contains("asks: true");
    assert!(asks(&searcher), "at first");
    for _ in 0..64_usize {
        assert!(searcher.is_match_in(yes.as_slice()).unwrap());
        assert_eq!(searcher.find_in(yes.as_slice(), 0).unwrap(), regex.find_in(yes.as_slice(), 0).unwrap());
    }
    assert!(!asks(&searcher), "after a window of matches");
    for _ in 0..64_usize {
        assert_eq!(searcher.find_in(no.as_slice(), 0).unwrap(), None);
    }
    assert!(asks(&searcher), "after a window of none");
    for n in 0..64_usize {
        let text = if n % 2 == 0 { &yes } else { &no };
        assert_eq!(searcher.find_in(text.as_slice(), 0).unwrap(), regex.find_in(text.as_slice(), 0).unwrap());
    }
    assert!(asks(&searcher), "after a window of half and half");
}
