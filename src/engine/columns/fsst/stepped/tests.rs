//! Unit tests of a LIKE pattern's machine stepped over codes
//! (`stepped.rs`).

use super::super::{MOST_SYMBOLS, Table};
use super::*;
use crate::value::LikePattern;
use crate::value::like::fixtures::{PATTERNS, strings};

/// `table` as a chunk reads it.
fn read(table: &Table) -> Symbols {
    let mut written = Vec::new();
    table.write(&mut written);
    Symbols::read(&written).unwrap().0
}

/// The codes of `text` by `table`.
fn coded(table: &Table, text: &[u8]) -> Vec<u8> {
    let mut codes = Vec::new();
    table.encode(text, &mut codes);
    codes
}

/// The steps of `pattern`, with `escape` if given, by `symbols`.
fn steps_of(symbols: &Symbols, pattern: &str, escape: Option<char>) -> Steps {
    symbols.steps(&LikePattern::new(pattern, escape).unwrap().machine().unwrap())
}

/// A table whose symbols the pieces of `PATTERNS` straddle, begin inside
/// and end inside, with no symbol of `a` alone nor of `c`, so those are
/// escaped where no longer symbol takes them.
fn straddled() -> Table {
    Table::of([&b"ab"[..], b"ba", b"aab", b"bb", b"b", b"abab", b"bca"].map(<[u8]>::to_vec).to_vec())
}

/// Verifies: [X3.135-1986 §5.14 GR5] (RULES.md D1251) a pattern's steps
/// match every string of three octets, of seven at most, by its codes
/// alone, as the pattern matches the string: by a table of symbols the
/// pieces straddle, with octets escaped, and by a table chosen of the
/// strings themselves, of symbols of several octets. Each shape of
/// pattern is among them, a piece that begins again inside itself and a
/// last piece found more than once too.
#[test]
fn steps_match_codes_as_the_pattern_matches_their_text() {
    let values = strings(b"abc", 7);
    let texts: Vec<&[u8]> = values.iter().map(Vec::as_slice).collect();
    let chosen = Table::chosen(&texts);
    assert!(chosen.symbols.iter().any(|s| s.len() > 2), "symbols of several octets: {:?}", chosen.symbols);
    let hand = straddled();
    let escapes = values.iter().filter(|v| coded(&hand, v).contains(&ESCAPE)).count();
    assert!(escapes.saturating_mul(2) > values.len(), "most strings have an octet escaped: {escapes}");
    for (table, what) in [(&hand, "straddled"), (&chosen, "chosen")] {
        let symbols = read(table);
        for p in PATTERNS {
            let pattern = LikePattern::new(p, None).unwrap();
            let steps = symbols.steps(&pattern.machine().unwrap());
            for v in &values {
                let text = std::str::from_utf8(v).unwrap();
                assert_eq!(steps.matches(&coded(table, v)), Some(pattern.matches(text)), "{what}: {text:?} LIKE {p:?}");
            }
        }
    }
}

/// A table of a symbol pieces end inside, a start of it and its first
/// octet: `b` and `c` alone are escaped.
fn nested() -> Table {
    Table::of([&b"xabcx"[..], b"ab", b"x"].map(<[u8]>::to_vec).to_vec())
}

/// Verifies: [X3.135-1986 §5.14 GR5] (RULES.md D1251) a piece that ends
/// inside a symbol is found, the octets after it in the symbol stepping
/// on: into the state that accepts whatever follows, into the piece
/// after it, which may end in the same symbol, and into a last piece,
/// which the symbol's own octets past it then leave; a first piece and
/// a pattern of no `%` inside one symbol too.
#[test]
fn a_piece_is_found_inside_a_symbol() {
    let table = nested();
    let symbols = read(&table);
    let matched = |pattern: &str, text: &[u8]| steps_of(&symbols, pattern, None).matches(&coded(&table, text));
    assert_eq!(coded(&table, b"xabcx"), [0], "one symbol");
    assert_eq!(matched("%ab%", b"xabcx"), Some(true), "ended inside, the rest accepted");
    assert_eq!(matched("%a%c%", b"xabcx"), Some(true), "two pieces in one symbol");
    assert_eq!(matched("%a%c", b"xabcx"), Some(false), "the last piece left by the symbol's last octet");
    assert_eq!(matched("%a%cx", b"xabcx"), Some(true));
    assert_eq!(matched("xab%", b"xabcx"), Some(true), "a first piece inside");
    assert_eq!(matched("xabcx", b"xabcx"), Some(true));
    assert_eq!(matched("xabc", b"xabcx"), Some(false), "an octet more than the pattern");
}

/// Verifies: [X3.135-1986 §5.14 GR2a, GR5] (RULES.md D1251) an escaped
/// octet steps the machine by itself, the escape stepping nothing, so a
/// piece is found across a symbol and an escaped octet; an escaped
/// octet 255 quotes nothing; and an ESCAPE character's `%` is an octet
/// of a piece like any other.
#[test]
fn an_escaped_octet_steps_by_itself() {
    let table = nested();
    let symbols = read(&table);
    let matched = |pattern: &str, text: &[u8]| steps_of(&symbols, pattern, None).matches(&coded(&table, text));
    assert_eq!(coded(&table, b"abc"), [1, ESCAPE, b'c'], "an octet escaped");
    assert_eq!(matched("%bc%", b"abc"), Some(true), "a piece across a symbol and an escaped octet");
    assert_eq!(matched("%c", b"abc"), Some(true));
    assert_eq!(matched("%b", b"abc"), Some(false));
    let high = [b'a', b'b', 255, 0, 255, 255, b'x'];
    assert_eq!(coded(&table, &high), [1, ESCAPE, 255, ESCAPE, 0, ESCAPE, 255, ESCAPE, 255, 2]);
    let machine = LikePattern::new("%x", None).unwrap().machine().unwrap();
    assert_eq!(symbols.steps(&machine).matches(&coded(&table, &high)), Some(true), "an escaped 255 quotes nothing");
    let escaped = steps_of(&symbols, "%b!%c%", Some('!'));
    assert_eq!(escaped.matches(&coded(&table, b"ab%cx")), Some(true), "an ESCAPE character's `%` is an octet of a piece");
    assert_eq!(escaped.matches(&coded(&table, b"abxcx")), Some(false));
}

/// Verifies: [DESIGN] RULES.md D1251: codes that end in an escape, or
/// hold a code of no symbol, match as none, so their chunk is decoded
/// and raises what is malformed; but not past where the text is known
/// to match or not, where no more of it is read. A table of every
/// symbol a table holds leaves no code but the escape of no symbol.
#[test]
fn malformed_codes_match_as_none() {
    let table = Table::of([&b"ab"[..], b"x"].map(<[u8]>::to_vec).to_vec());
    let symbols = read(&table);
    let within = steps_of(&symbols, "%ab%", None);
    assert_eq!(within.matches(&[1, ESCAPE]), None, "ends in an escape");
    assert_eq!(within.matches(&[1, 2]), None, "no symbol 2");
    assert_eq!(within.matches(&[1, 200, 0]), None, "nor 200");
    assert_eq!(within.matches(&[0, 2, ESCAPE]), Some(true), "known to match before them");
    let first = steps_of(&symbols, "ab%x", None);
    assert_eq!(first.matches(&[1, 2, ESCAPE]), Some(false), "known not to match before them");
    assert_eq!(first.matches(&[0, ESCAPE]), None);
    assert_eq!(within.matches(&[]), Some(false));
    assert_eq!(steps_of(&symbols, "%", None).matches(&[ESCAPE]), Some(true), "`%` reads nothing");
    let full = Table::of((0..MOST_SYMBOLS).map(|n| vec![b'a', u8::try_from(n).unwrap()]).collect());
    let all = steps_of(&read(&full), "%a?%", None);
    assert_eq!(all.matches(&[63]), Some(true), "the last symbol of a full table");
    assert_eq!(all.matches(&[64]), None);
}

/// Verifies: [DESIGN] RULES.md D1251: a scan's steps are made once a
/// LIKE and a table of symbols, found again while its chunks' tables are
/// the same, made again of another table, by which they match as that
/// table codes, and kept apart by the LIKE's place.
#[test]
fn steps_are_made_once_a_pattern_and_a_table() {
    let (one, other) = (Table::of(vec![b"ab".to_vec(), b"c".to_vec()]), Table::of(vec![b"c".to_vec(), b"ab".to_vec()]));
    let (first, second) = (read(&one), read(&other));
    let machine = LikePattern::new("%ab%", None).unwrap().machine().unwrap();
    let ends = LikePattern::new("%c", None).unwrap().machine().unwrap();
    let mut stepped = Stepped::default();
    for _ in 0_u8..3_u8 {
        assert_eq!(stepped.of(4, &first, &machine).unwrap().matches(&[0]), Some(true));
    }
    assert_eq!(stepped.made, 1, "found again");
    assert_eq!(stepped.of(4, &second, &machine).unwrap().matches(&[0]), Some(false), "code 0 is `c` by the other table");
    assert_eq!(stepped.of(4, &second, &machine).unwrap().matches(&[1]), Some(true));
    assert_eq!(stepped.made, 2, "made again of another table");
    assert_eq!(stepped.of(7, &second, &ends).unwrap().matches(&[1, 0]), Some(true), "another LIKE of the scan");
    assert_eq!(stepped.of(4, &second, &machine).unwrap().matches(&[1, 0]), Some(true));
    assert_eq!(stepped.of(7, &second, &ends).unwrap().matches(&[0, 1]), Some(false));
    assert_eq!(stepped.made, 3, "each kept by its place");
}
