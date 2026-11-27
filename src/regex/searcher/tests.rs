//! Unit tests of the searcher over a corpus made here (RULES.md D1188):
//! patterns put together from atoms, quantifiers, alternation, groups and
//! anchors under each flag, searched in every short text of an alphabet
//! with both newline characters and a character of two octets, from every
//! position. What a searcher kept across the texts finds, with its
//! automaton, its marks and its scratch, is held to the matchers D61
//! began with, the one of threads and the one that counts its steps. So
//! is what it finds in each text read where it lies, by the octets its
//! characters begin at (RULES.md D1189).

use super::super::bounded::tests::PATTERNS;
use super::super::{Ascii, Emit, Matcher, Replacement, Utf8, backtrack, dfa, pike};
use super::*;

/// What a pattern is made of: something that matches a character, or a
/// group of them, a back-reference after its group among them.
const ATOMS: [&str; 24] = [
    "a", "b", "[ab]", "[^a]", ".", "\\s", "\\S", "(a)", "(b)", "(a|b)", "(ab|a)", "(a|ab)", "()", "(|a)", "é", "[a-b\\r]", "\\r", "\\n",
    "(?:a|b)", "(a*)", "(.)", "(a)\\1", "(a|b)\\1", "(a*)\\1",
];

/// How often an atom is taken: once, greedily or reluctantly any number of
/// times, or a bounded number.
const QUANTITIES: [&str; 13] = ["", "", "", "*", "+", "?", "*?", "+?", "??", "{2}", "{1,2}", "{0,2}?", "{2,}"];

/// The flags the patterns are read under, the wildcard's and the lines'
/// among them.
const FLAGS: [&str; 5] = ["", "s", "m", "i", "sm"];

/// Patterns whose match, searched again as a string of its own, has other
/// groups, by `^`, by `$` and by the wildcard beside a CR.
const BESIDE: [(&str, &str); 5] = [("(^a)|(a)", ""), ("(a$)|(a)", ""), ("(.)|(\\r)", "s"), ("(^b)|(b)", "m"), ("b(^a)?a", "m")];

/// Patterns that end by taking the rest of the text, and ones like them
/// that do not: under the flag `s` and without, of each line, in a group.
const TAILS: [(&str, &str); 8] =
    [("a.*$", "s"), ("(a)(.*)$", "s"), ("^(a|b)(.+)$", "s"), ("b.*$", "sm"), ("a.*$", ""), (".*$", "s"), ("(b.*$)", "s"), ("\\r.*$", "s")];

/// The characters of the short texts: two letters, both characters of a
/// CRLF, and a letter of two octets.
const ALPHABET: [char; 5] = ['a', 'b', '\r', '\n', 'é'];

/// Longer texts: every newline sequence of UTS #18 RL1.6, blanks, an
/// address, runs that a counted quantifier takes.
const LONGER: [&str; 12] = [
    "a\r\nb\r\n",
    "\r\r\n\n",
    "ab\u{85}a\u{2028}b\u{2029}a",
    "a\u{b}b\u{c}",
    " \t a b",
    "aaaaab",
    "ababab",
    "bbbaaa",
    "http://www.example.test/a/b",
    "éaéb",
    "aab aab",
    "abba",
];

/// The next number of a fixed sequence (xorshift), so the corpus is the
/// same every run.
fn next(state: &mut u64) -> usize {
    *state ^= *state << 13_u32;
    *state ^= *state >> 7_u32;
    *state ^= *state << 17_u32;
    usize::try_from(*state >> 33_u32).unwrap()
}

/// One of `from`, as the sequence's next number says.
fn pick(state: &mut u64, from: &[&'static str]) -> &'static str {
    from[next(state).checked_rem(from.len()).unwrap()]
}

/// Counts one more in `n` if `counted`.
fn count(n: &mut usize, counted: bool) {
    *n = n.saturating_add(usize::from(counted));
}

/// One to three atoms, each with a quantity.
fn pieces(state: &mut u64) -> String {
    let mut made = String::new();
    for _ in 0..=next(state) % 3 {
        made.push_str(pick(state, &ATOMS));
        made.push_str(pick(state, &QUANTITIES));
    }
    made
}

/// A branch: pieces, anchored at its start, at its end, or neither.
fn branch(state: &mut u64) -> String {
    let (start, end) = (next(state).is_multiple_of(4), next(state).is_multiple_of(4));
    format!("{}{}{}", if start { "^" } else { "" }, pieces(state), if end { "$" } else { "" })
}

/// The corpus's patterns and flags: the fixed ones, and `made` put
/// together of branches, alone, in alternation, or in a group taken a
/// quantity of times before more pieces.
fn patterns(made: usize) -> Vec<(String, &'static str)> {
    let mut all: Vec<(String, &'static str)> = PATTERNS.iter().chain(&BESIDE).chain(&TAILS).map(|(p, f)| ((*p).to_owned(), *f)).collect();
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    for flag in FLAGS.iter().cycle().take(made) {
        let pattern = match next(&mut state) % 6 {
            0 | 1 => format!("{}|{}", branch(&mut state), branch(&mut state)),
            2 => format!("({}){}{}", branch(&mut state), pick(&mut state, &QUANTITIES), pieces(&mut state)),
            _ => branch(&mut state),
        };
        all.push((pattern, *flag));
    }
    all
}

/// The corpus's texts: every string of `ALPHABET` of at most three
/// characters, and the longer ones.
fn texts() -> Vec<Vec<char>> {
    let mut all: Vec<Vec<char>> = vec![Vec::new()];
    let mut from = 0;
    for _ in 0..3_usize {
        let to = all.len();
        for n in from..to {
            for c in ALPHABET {
                let mut longer = all[n].clone();
                longer.push(c);
                all.push(longer);
            }
        }
        from = to;
    }
    all.extend(LONGER.iter().map(|t| t.chars().collect::<Vec<char>>()));
    all
}

/// The match the matchers D61 began with find.
fn first<T: Text + ?Sized>(regex: &Regex, text: &T, from: usize) -> Option<pike::Slots> {
    if regex.back_references {
        return backtrack::search(&regex.program, text, from, regex.caseless.then_some(&regex.table)).unwrap();
    }
    pike::search(&regex.program, text, from, true).unwrap()
}

/// What `slots` are as a match's groups.
fn groups(slots: Option<pike::Slots>) -> Option<Captures> {
    let spans = slots?.chunks(2).map(|pair| if let [Some(a), Some(b)] = pair { Some((*a, *b)) } else { None }).collect();
    Some(Captures { spans })
}

/// What the corpus counted, so it is known to hold each kind.
#[derive(Default)]
struct Counted {
    patterns: usize,
    automata: usize,
    back_references: usize,
    lines: usize,
    found: usize,
    none: usize,
    empty: usize,
    alone: usize,
    again: usize,
}

/// The octet each character of `string` begins at, and its length after
/// them, which is where a position that counts characters is in the
/// string read where it lies.
fn octets_of(string: &str) -> Vec<usize> {
    string.char_indices().map(|(at, _)| at).chain([string.len()]).collect()
}

/// `found`, spans of characters, as spans of the octets `octets` gives.
fn in_octets(found: &Captures, octets: &[usize]) -> Captures {
    Captures { spans: found.spans.iter().map(|span| span.map(|(a, b)| (octets[a], octets[b]))).collect() }
}

/// Holds `kept`, a searcher of `regex` kept across the texts, to the
/// first matchers over `text` from each position: the match, its groups,
/// whether there is one, the same of the text read as ASCII where it is
/// and of any text read where it lies, by its octets, into a match
/// vector kept from one search to the next, and of a search of the text
/// alone.
fn searched(regex: &Regex, kept: &mut Searcher<'_>, text: &[char], counted: &mut Counted) {
    let what = |from: usize| format!("{:?} in {text:?} from {from}", regex.program.insts);
    let string: String = text.iter().collect();
    let (octets, mut again) = (octets_of(&string), Captures::default());
    for from in 0..=text.len() {
        let wanted = groups(first(regex, text, from));
        let lying = wanted.as_ref().map(|found| in_octets(found, &octets));
        assert_eq!(
            kept.find_into(&Utf8::of(&string), octets[from], &mut again).unwrap(),
            wanted.is_some(),
            "where it lies, {}",
            what(from)
        );
        assert_eq!(wanted.is_some().then_some(&again), lying.as_ref(), "where it lies, {}", what(from));
        assert_eq!(regex.find_in(&Utf8::of(&string), octets[from]).unwrap(), lying, "where it lies, alone, {}", what(from));
        assert_eq!(groups(first(regex, &Utf8::of(&string), octets[from])), lying, "where it lies, the first matchers, {}", what(from));
        count(&mut counted.found, wanted.is_some());
        count(&mut counted.none, wanted.is_none());
        count(&mut counted.empty, wanted.as_ref().and_then(|c| c.get(0)).is_some_and(|(a, b)| a == b));
        assert_eq!(kept.find_in(text, from).unwrap(), wanted, "kept, {}", what(from));
        assert_eq!(regex.find_in(text, from).unwrap(), wanted, "alone, {}", what(from));
        if let Some(ascii) = Ascii::of(&string) {
            assert_eq!(kept.find_in(&ascii, from).unwrap(), wanted, "in place, {}", what(from));
        }
        assert_eq!(kept.search(text, from, false).unwrap(), wanted.is_some(), "whether, {}", what(from));
    }
    assert_eq!(kept.is_match(&string).unwrap(), first(regex, text, 0).is_some(), "whether, of the string, {}", what(0));
}

/// What `regex` makes of the match at `span` of `text` replaced by
/// `with` as fn:replace replaces it, `cut` giving what two positions of
/// the text span.
fn made_of<T: Text + ?Sized>(
    regex: &Regex,
    (text, span): (&T, (usize, usize)),
    with: &Replacement,
    cut: &dyn Fn(usize, usize) -> String,
) -> String {
    let mut made = String::new();
    let put = |piece: Emit<'_>| {
        match piece {
            Emit::Text(t) => made.push_str(t),
            Emit::Span(a, b) => made.push_str(&cut(a, b)),
        }
        Ok(())
    };
    regex.replace_in(text, span, with, put).unwrap();
    made
}

/// Holds each match of `regex` in `text` read where it lies, one after
/// another to its end, a match of no characters passed by a character
/// of however many octets, and what replaces it by `with`, if given, to
/// the matches of the text's characters and what replaces them.
fn lying(regex: &Regex, text: &[char], with: Option<&Replacement>) {
    let string: String = text.iter().collect();
    let (octets, utf8) = (octets_of(&string), Utf8::of(&string));
    let (mut wanted, mut found) = (Vec::new(), Vec::new());
    regex
        .each(text, 0, usize::MAX, |mv| {
            let made = mv.get(0).zip(with).map(|(span, with)| made_of(regex, (text, span), with, &|a, b| text[a..b].iter().collect()));
            wanted.push((in_octets(&mv, &octets), made));
            Ok(true)
        })
        .unwrap();
    regex
        .each(&utf8, 0, usize::MAX, |mv| {
            let made = mv.get(0).zip(with).map(|(span, with)| made_of(regex, (&utf8, span), with, &|a, b| utf8.span(a, b).to_owned()));
            found.push((mv, made));
            Ok(true)
        })
        .unwrap();
    assert_eq!(found, wanted, "where it lies, {:?} in {text:?}", regex.program.insts);
}

/// Holds what replaces each match of `regex` in `text`, where the match
/// is replaced from its own groups, to fn:replace of the match searched as
/// a string of its own.
fn replaced(regex: &Regex, text: &[char], counted: &mut Counted) {
    lying(regex, text, regex.replacement("[$1|$2|$0]").ok().as_ref());
    let Ok(with) = regex.replacement("[$1|$2|$0]") else { return };
    regex
        .each(text, 0, usize::MAX, |found| {
            let span = found.get(0).unwrap();
            let mut again = Vec::new();
            let pushed = |into: &mut Vec<String>, piece: Emit<'_>| {
                into.push(format!("{piece:?}"));
                Ok(())
            };
            regex.replace_in(text, span, &with, |piece| pushed(&mut again, piece)).unwrap();
            if regex.matches_alone(text.len(), span) {
                let mut alone = Vec::new();
                with.of_match(&found, |piece| pushed(&mut alone, piece)).unwrap();
                assert_eq!(alone, again, "{:?} in {text:?} at {span:?}", regex.program.insts);
                count(&mut counted.alone, true);
            } else {
                count(&mut counted.again, true);
            }
            Ok(true)
        })
        .unwrap();
}

/// Verifies: [DESIGN] RULES.md D1188, D61 [F&O 3.1 §5.6; ISO/IEC
/// 9075-2:2023 §9.28 GR9a, §9.29]: over a corpus of several hundred
/// patterns, of alternation, greedy, reluctant and bounded quantifiers,
/// groups, classes, anchors and back-references under the flags `s`, `m`
/// and `i`, in every text of at most three characters of two letters, CR,
/// LF and a letter of two octets, and longer ones, from every position, a
/// searcher kept across the texts finds the match and the groups the
/// matcher of threads finds, or of a back-reference the matcher that
/// counts its steps; so does a search of each text alone, one of the
/// text of ASCII read where it lies, and (D1189) one of any text read
/// where it lies, its positions the octets its characters begin at, into
/// a match vector kept from search to search; each match after another
/// of a text read so, and what replaces it, are those of its characters;
/// and a match replaced from its own
/// groups is replaced as the match searched as a string of its own is.
/// A program with a back-reference runs on the matcher that counts, and
/// it and one with `^` or `$` of each line have no automaton.
#[test]
fn over_a_corpus_a_searcher_finds_what_the_first_matchers_find() {
    let (texts, mut counted) = (texts(), Counted::default());
    for (pattern, flag) in patterns(640) {
        let Ok(regex) = Regex::new(&pattern, flag) else { continue };
        let mut kept = regex.searcher();
        // A `^` after `[` is of a class, no anchor.
        let lines = flag.contains('m') && pattern.replace("[^", "[").contains(['^', '$']);
        assert_eq!(kept.dfa.is_some(), !regex.back_references && dfa::fits(&regex.program), "{pattern:?} {flag:?}");
        assert!(!(kept.dfa.is_some() && lines), "{pattern:?} {flag:?} has an automaton");
        assert_eq!(regex.matcher(3, 0) == Matcher::Backtrack, regex.back_references, "{pattern:?} {flag:?}");
        count(&mut counted.patterns, true);
        count(&mut counted.automata, kept.dfa.is_some());
        count(&mut counted.back_references, regex.back_references);
        count(&mut counted.lines, lines);
        for text in &texts {
            searched(&regex, &mut kept, text, &mut counted);
            replaced(&regex, text, &mut counted);
        }
    }
    assert!(counted.patterns >= 600, "patterns: {}", counted.patterns);
    assert!(counted.automata >= 300 && counted.back_references >= 40 && counted.lines >= 40, "kinds of pattern");
    assert!(counted.found >= 100_000 && counted.none >= 100_000 && counted.empty >= 10_000, "kinds of search");
    assert!(counted.alone >= 10_000 && counted.again >= 10_000, "kinds of replacing");
}

/// Verifies: [DESIGN] RULES.md D1188 [ISO/IEC 9075-2:2023 §9.29]: a match
/// is replaced from its own groups if it is the whole text or its program
/// reads nothing beside a character, and searched again as a string of its
/// own if not, where that finds other groups: after `b`, `(^a)|(a)`
/// matches `a` by its second group, and that `a` alone by its first.
///
/// MC/DC of `matches_alone`: the whole text of a program that reads
/// beside, part of it, and part of a program that does not.
#[test]
fn a_match_is_searched_again_only_where_that_may_find_other_groups() {
    let beside = Regex::new("(^a)|(a)", "").unwrap();
    assert!(beside.matches_alone(1, (0, 1)));
    assert!(!beside.matches_alone(2, (1, 2)));
    assert!(!beside.matches_alone(2, (0, 1)), "its end is not the text's");
    assert!(Regex::new("(a)|(b)", "").unwrap().matches_alone(2, (1, 2)));
    for (pattern, flag) in [("^a", ""), ("a$", ""), (".", "s"), ("(a)\\1", "")] {
        assert!(!Regex::new(pattern, flag).unwrap().matches_alone(3, (1, 2)), "{pattern:?} reads beside");
    }
    let (chars, mut counted) = ("ba".chars().collect::<Vec<char>>(), Counted::default());
    replaced(&beside, chars.as_slice(), &mut counted);
    assert_eq!((counted.alone, counted.again), (0, 1));
    let mut made = String::new();
    let with = beside.replacement("[$1|$2]").unwrap();
    let put = |piece: Emit<'_>| {
        match piece {
            Emit::Text(t) => made.push_str(t),
            Emit::Span(a, b) => made.extend(&chars[a..b]),
        }
        Ok(())
    };
    beside.replace_in(chars.as_slice(), (1, 2), &with, put).unwrap();
    assert_eq!(made, "[a|]", "fn:replace of the match alone");
}

/// Verifies: [DESIGN] RULES.md D1189 [F&O 3.1 §5.6.1.4, §5.6.2]: in a
/// text of characters of one to four octets read where it lies, each
/// match from each position and what replaces it are those of the text's
/// characters. The cases are a class taken over characters of several octets, a match
/// of no characters passed by one of them, the rest of a text taken, and
/// a back-reference under the flag `i` matched by a case variant of
/// other octets than what was captured, the Kelvin sign for `k`.
#[test]
fn a_text_of_longer_characters_read_where_it_lies_matches_as_its_characters() {
    let cases = [
        ("([^/]+)/", "", "ü€/\u{10348}/x"),
        ("x*", "", "é€\u{10348}"),
        ("(.)(.*)$", "s", "\u{10348}é\r\n€"),
        ("(k)\\1", "i", "k\u{212A}k"),
        ("(é+)b\\1", "i", "éÉbÉéb"),
    ];
    for (pattern, flag, text) in cases {
        let regex = Regex::new(pattern, flag).unwrap();
        let chars: Vec<char> = text.chars().collect();
        searched(&regex, &mut regex.searcher(), &chars, &mut Counted::default());
        lying(&regex, &chars, regex.replacement("[$1|$0]").ok().as_ref());
    }
    let again = Regex::new("(k)\\1", "i").unwrap();
    let found = again.find_in(&Utf8::of("k\u{212A}k"), 0).unwrap().unwrap();
    assert_eq!((found.get(0), found.get(1)), (Some((0, 4)), Some((0, 1))), "a sign of three octets after a letter of one");
}
