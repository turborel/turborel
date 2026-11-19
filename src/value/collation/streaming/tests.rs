// Unit tests: each level's weights and the places a collation may be cut
// at, against sort keys, and equality of large objects a piece at a time
// (RULES.md D60, D79).

use std::sync::Arc;

use super::*;
use crate::value::collation::{Collation, UNICODE, builtin};
use crate::value::lob::{Chunk, Directory, Kept, LobKind, LobSource};

/// The collations held to the contract: UNICODE, and tailorings of
/// contractions (`ch`), of `[backwards 2]`, of `[caseFirst upper]`, of
/// prefixes and `[strength 3]`, and of letters after `z`.
const NAMES: [&str; 6] = [UNICODE, "UNICODE_SV", "UNICODE_FR_CA", "UNICODE_CS", "UNICODE_DA", "UNICODE_JA"];

/// Strings of contractions, combining marks in and out of order, Hangul,
/// variable and ignorable characters, case, and several scripts.
const CORPUS: [&str; 58] = [
    "",
    " ",
    "   ",
    "a",
    "a ",
    "abc",
    "résumé",
    "re\u{301}sume\u{301}",
    "Résumé",
    "\u{e9}",
    "Zebra",
    "de luge",
    "delta",
    "a\tb",
    "a-b",
    "ab",
    "a\u{0}b",
    "-\u{301}a",
    "a-\u{301}",
    "\u{ac00}\u{ac01}",
    "\u{1100}\u{1161}\u{11a8}",
    "\u{ac01}",
    "\u{439}",
    "\u{438}\u{306}",
    "\u{438}\u{306}\u{334}",
    "\u{438}\u{334}\u{306}",
    "ch",
    "chata",
    "Chata",
    "CHATA",
    "hrad",
    "cote",
    "côte",
    "coté",
    "côté",
    "åäö",
    "a\u{30a}a\u{308}o\u{308}",
    "\u{4e00}\u{3400}",
    "日本語",
    "カタカナー",
    "かたかなー",
    "ｶﾀｶﾅｰ",
    "l·l",
    "ll",
    "s\u{323}\u{307}",
    "\u{1e69}",
    "x\u{301}\u{316}",
    "x\u{316}\u{301}",
    "\u{344}",
    "\u{f73}",
    "👍🏽",
    "Straße",
    "strasse",
    "Ελληνικά",
    "ру́сский",
    "\u{10ffff}",
    "þh",
    "th",
];

/// Tails a text may go on with, each able to change how the end of what
/// is before it weighs: marks, a contraction's second character, an
/// ignorable, a variable one, a prolonged sound mark.
const TAILS: [&str; 10] = ["", "\u{301}", "\u{306}\u{334}", "h", "H", "\u{0}", "-\u{301}", "ー", "\u{316}x", "a"];

/// A source of chunks in memory.
#[derive(Debug)]
struct Chunks {
    directory: Directory,
    chunks: Vec<Arc<[u8]>>,
}

impl LobSource for Chunks {
    fn totals(&self) -> (u64, u64) {
        self.directory.totals()
    }
    fn directory(&self) -> Result<&Directory> {
        Ok(&self.directory)
    }
    fn chunk(&self, k: usize) -> Result<Arc<[u8]>> {
        self.chunks.get(k).cloned().ok_or_else(|| broken("no such chunk"))
    }
    fn kept(&self) -> Option<Kept> {
        None
    }
}

/// The character large object of `text`, in chunks of `per` characters.
fn lob(text: &str, per: usize) -> Lob {
    let chars: Vec<char> = text.chars().collect();
    let pieces: Vec<String> = chars.chunks(per.max(1)).map(|c| c.iter().collect()).collect();
    let sizes = pieces.iter().map(|p| Chunk { octets: u32::try_from(p.len()).unwrap(), chars: u32::try_from(p.chars().count()).unwrap() });
    let chunks = pieces.iter().map(|p| Arc::from(p.as_bytes())).collect();
    Lob::of_source(LobKind::Character, Arc::new(Chunks { directory: Directory::new(sizes.collect()), chunks }))
}

fn named(name: &str, pad: Pad) -> CollationRef {
    let collation = builtin().into_iter().find(|c| c.name() == name).unwrap();
    CollationRef { collation, pad }
}

fn key(c: &dyn Collation, text: &str, pad: Pad) -> Vec<u8> {
    let mut out = Vec::new();
    c.sort_key(text, pad, &mut out);
    out
}

/// Each level's weights of `text`.
fn weights(c: &dyn Collation, text: &str) -> Vec<Vec<u8>> {
    (1..=c.levels())
        .map(|level| {
            let mut out = Vec::new();
            c.level_weights(text, level, &mut out);
            out
        })
        .collect()
}

/// Each level's weights of `pieces`, joined.
fn joined(c: &dyn Collation, pieces: &[&str]) -> Vec<Vec<u8>> {
    let mut all = vec![Vec::new(); c.levels()];
    for piece in pieces {
        for (to, from) in all.iter_mut().zip(weights(c, piece)) {
            to.extend(from);
        }
    }
    all
}

/// The corpus, and each pairing of a few of its strings, so a contraction
/// or a mark meets what is before it.
fn strings() -> Vec<String> {
    let mut all: Vec<String> = CORPUS.iter().map(ToString::to_string).collect();
    for (n, a) in CORPUS.iter().enumerate().step_by(3) {
        for b in CORPUS.iter().skip(n % 5).step_by(7) {
            all.push(format!("{a}{b}"));
        }
    }
    all
}

/// The octets of `s` from `from` to `to`, both a character's boundary.
fn part(s: &str, from: usize, to: usize) -> &str {
    s.get(from..to).unwrap()
}

/// A small pseudo-random sequence, the same each run.
struct Random(u64);

impl Random {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13_u32;
        self.0 ^= self.0 >> 7_u32;
        self.0 ^= self.0 << 17_u32;
        usize::try_from(self.0.checked_rem(u64::try_from(n.max(1)).unwrap()).unwrap()).unwrap()
    }
}

/// Verifies: [EXTENSION] RULES.md D60, D79 \[UTS #10 S3\]: for UNICODE and
/// tailorings of contractions, `[backwards 2]`, `[caseFirst upper]`,
/// prefixes and `[strength 3]`, two strings' weights are the same at every
/// level exactly when their sort keys are, over strings that are equal
/// only canonically, only ignoring what is ignorable, or not at all.
#[test]
fn level_weights_agree_with_sort_keys() {
    let all = strings();
    for name in NAMES {
        let c = named(name, Pad::None);
        let keys: Vec<Vec<u8>> = all.iter().map(|s| key(&*c.collation, s, Pad::None)).collect();
        let levels: Vec<Vec<Vec<u8>>> = all.iter().map(|s| weights(&*c.collation, s)).collect();
        assert!(levels.iter().all(|l| l.len() == c.collation.levels()), "{name}");
        let mut equal = 0;
        for (a, (ka, la)) in all.iter().zip(keys.iter().zip(&levels)) {
            for (b, (kb, lb)) in all.iter().zip(keys.iter().zip(&levels)) {
                assert_eq!(ka == kb, la == lb, "{name} {a:?} {b:?}");
                equal += usize::from(ka == kb && a != b);
            }
        }
        assert!(equal > 0, "{name}: some different strings compare equal");
    }
}

/// Verifies: [EXTENSION] RULES.md D60, D79 \[UTS #10 S2.1, §4.4\]: a place
/// `safe_cut` gives of any prefix of a string is a character's boundary
/// before the prefix's end, and each level's weights of the prefix, gone on
/// with any tail, are those of the two pieces it is cut into, joined:
/// marks, contractions and Shifted's state do not reach across it.
#[test]
fn a_safe_cut_keeps_each_levels_weights() {
    let all = strings();
    for name in NAMES {
        let c = &*named(name, Pad::None).collation;
        let mut cuts = 0;
        for s in &all {
            for (q, _) in s.char_indices().skip(1).chain([(s.len(), ' ')]) {
                let prefix = part(s, 0, q);
                let Some(p) = c.safe_cut(prefix) else { continue };
                assert!(p < prefix.len() && prefix.is_char_boundary(p), "{name} {prefix:?} {p}");
                cuts += 1;
                for tail in TAILS {
                    let rest = format!("{}{tail}", part(prefix, p, prefix.len()));
                    let whole = format!("{prefix}{tail}");
                    assert_eq!(joined(c, &[part(prefix, 0, p), &rest]), weights(c, &whole), "{name} {whole:?} cut at {p}");
                }
            }
        }
        assert!(cuts > all.len(), "{name}: strings are cut");
        assert_eq!(c.safe_cut(""), None, "{name}");
        assert_eq!(c.safe_cut("\u{301}\u{301}"), None, "{name}: marks only");
    }
}

/// Verifies: [EXTENSION] [XQueryFO §7.5] RULES.md D60, D106: the collation
/// units of a string begin at its first character and go forward, a
/// character each at most, none of the empty string, each weighed at
/// every level, and their weights joined at each level are the string's,
/// marks, contractions, prefixes and Shifted's state reaching across
/// them: over the corpus and pairs of it, for UNICODE, its tailorings and
/// the collations of one character at a time.
#[test]
fn units_join_to_a_strings_weights() {
    let all = strings();
    for name in NAMES.into_iter().chain(["UCS_BASIC", "CASE_INSENSITIVE"]) {
        let c = &*named(name, Pad::None).collation;
        for s in &all {
            let units = c.units(s).unwrap();
            let starts: Vec<usize> = units.iter().map(|u| u.start).collect();
            let length = s.chars().count();
            assert!(starts.first().is_none_or(|&f| f == 0) && starts.windows(2).all(|w| w[0] < w[1]), "{name} {s:?} {starts:?}");
            assert!(starts.last().is_none_or(|&l| l < length) && units.is_empty() == s.is_empty(), "{name} {s:?} {starts:?}");
            let mut joined = vec![Vec::new(); c.levels()];
            for unit in &units {
                assert_eq!(unit.weights.len(), c.levels(), "{name} {s:?}");
                for (to, from) in joined.iter_mut().zip(&unit.weights) {
                    to.extend_from_slice(from);
                }
            }
            assert_eq!(joined, weights(c, s), "{name} {s:?}");
        }
    }
}

/// Verifies: [EXTENSION] RULES.md D60, D79: a string cut again and again,
/// at random lengths each cut at the last safe place, as `lob_equal` reads
/// it, has the weights of the whole at every level.
#[test]
fn many_safe_cuts_keep_each_levels_weights() {
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    let text: String = (0_u32..400_u32).map(|_| CORPUS[random.below(CORPUS.len())]).collect::<Vec<_>>().join("");
    for name in NAMES {
        let c = &*named(name, Pad::None).collation;
        let mut pieces = Vec::new();
        let (mut at, mut grow) = (0, 0);
        while at < text.len() {
            let mut end = (at + 1 + random.below(40) + grow).min(text.len());
            while !text.is_char_boundary(end) {
                end += 1;
            }
            match c.safe_cut(part(&text, at, end)).filter(|&p| p > 0 && end < text.len()) {
                Some(p) => {
                    pieces.push(part(&text, at, at + p));
                    (at, grow) = (at + p, 0);
                }
                None if end == text.len() => {
                    pieces.push(part(&text, at, text.len()));
                    at = end;
                }
                None => grow += 8,
            }
        }
        assert!(pieces.len() > 50, "{name}: {} pieces", pieces.len());
        assert_eq!(joined(c, &pieces), weights(c, &text), "{name}");
    }
}

/// Pairs of strings that compare equal under some of the collations:
/// canonically, ignoring what is ignorable, or by `[strength 3]`.
const EQUAL: [(&str, &str); 8] = [
    ("résumé", "re\u{301}sume\u{301}"),
    ("\u{ac01}", "\u{1100}\u{1161}\u{11a8}"),
    ("s\u{323}\u{307}", "\u{1e69}"),
    ("x\u{301}\u{316}", "x\u{316}\u{301}"),
    ("\u{438}\u{306}\u{334}", "\u{438}\u{334}\u{306}"),
    ("ab", "a\u{0}b"),
    ("a-b", "ab"),
    ("a", "a   "),
];

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.2 GR3] RULES.md D60, D79:
/// two character large objects of many small chunks compare equal under
/// UNICODE or a tailoring exactly when their sort keys are, under PAD NONE
/// and PAD SPACE, the latter leaving trailing spaces off: each string of
/// the corpus against itself and the next few, and pairs equal under some.
#[test]
fn lob_equal_agrees_with_sort_keys() {
    let near = CORPUS.iter().enumerate().flat_map(|(n, a)| (0..4).map(move |k| (*a, CORPUS[(n + k) % CORPUS.len()])));
    let pairs: Vec<(&str, &str)> = near.chain(EQUAL).collect();
    for name in NAMES {
        for pad in [Pad::None, Pad::Space] {
            let c = named(name, pad);
            for (n, (a, b)) in pairs.iter().enumerate() {
                let expected = key(&*c.collation, a, pad) == key(&*c.collation, b, pad);
                assert_eq!(lob_equal(&lob(a, 1 + n % 3), &lob(b, 2), &c).unwrap(), expected, "{name} {pad:?} {a:?} {b:?}");
            }
        }
    }
}

/// A text of more octets than `lob_equal` reads at once, `spaces` spaces
/// after it: words of several scripts, most of three octets a character
/// so the test reads few characters, with marks, contractions and spaces.
fn large(spaces: usize) -> String {
    let word = "日本語の文章です한국어문장입니다日本語の文章です한국어문장입니다 côte chata résumé ру́сский \u{438}\u{306} a-b ";
    let mut text = word.repeat(CHUNK_OCTETS.div_ceil(word.len()).saturating_add(1));
    text.push_str(&" ".repeat(spaces));
    text
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.2 GR3] RULES.md D60, D79:
/// large objects of more octets than are read at once compare equal under
/// UNICODE and a tailoring of `[backwards 2]` when they differ only
/// canonically, or by trailing spaces under PAD SPACE, and unequal when
/// they differ by an accent at the start, which `[backwards 2]` weighs
/// last, by case at the end, or by trailing spaces under PAD NONE.
#[test]
fn large_objects_compare_a_piece_at_a_time() {
    let text = large(0);
    let decomposed = text.replace('é', "e\u{301}").replace('한', "\u{1112}\u{1161}\u{11ab}");
    let accented = text.replacen("côte", "cote", 1);
    let cased = format!("{}A", part(&text, 0, text.len().saturating_sub(1)));
    let spaced = large(CHUNK_OCTETS.div_ceil(8).saturating_add(3));
    let base = lob(&text, 997);
    for name in [UNICODE, "UNICODE_FR_CA"] {
        let (none, space) = (named(name, Pad::None), named(name, Pad::Space));
        assert!(lob_equal(&base, &lob(&decomposed, 613), &none).unwrap(), "{name}: canonical equivalents");
        assert!(!lob_equal(&base, &lob(&accented, 4099), &none).unwrap(), "{name}: an accent");
        assert!(!lob_equal(&base, &lob(&cased, 101), &none).unwrap(), "{name}: case");
        assert!(!lob_equal(&base, &lob(&spaced, 1500), &none).unwrap(), "{name}: spaces under PAD NONE");
        assert!(lob_equal(&base, &lob(&spaced, 1500), &space).unwrap(), "{name}: spaces under PAD SPACE");
    }
}

/// Verifies: [DESIGN] RULES.md D79: a value no place of which the collation
/// may cut at within the window is refused, not compared wrongly.
#[test]
fn a_value_with_no_cut_is_refused() {
    let marks = format!("a{}", "\u{301}".repeat(MOST_WINDOW.div_ceil(2).saturating_add(1)));
    let c = named(UNICODE, Pad::None);
    let marked = lob(&marks, 4096);
    let error = lob_equal(&marked, &marked, &c).unwrap_err();
    assert!(error.to_string().contains("no place"), "{error}");
}
