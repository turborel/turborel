//! Tests of reading a character large object by position, and matching
//! it where matches cross a chunk's end.

use super::*;
use crate::regex::Regex;
use crate::value::lob::fixtures::{Temp, chunked, text, wide};

/// A text whose first chunk ends within `<` and four characters of `per`
/// octets, then `>`, after characters of `per` octets, and as many after.
fn straddling(per: usize) -> String {
    let w = wide(per);
    let filler = |n: usize| std::iter::repeat_n(w, n).collect::<String>();
    let before = CHUNK_OCTETS.saturating_sub(per.saturating_add(1)).div_euclid(per);
    format!("{}<{w}{w}{w}{w}>{}", filler(before), filler(before))
}

/// The characters of the first chunk of `lob`'s source.
fn first_chunk_chars(lob: &Lob) -> usize {
    let Some(Part::Slice(s)) = lob.parts().first() else { panic!("not chunked") };
    usize::try_from(s.source.directory().unwrap().chunks()[0].chars).unwrap()
}

/// Verifies: [DESIGN] RULES.md D79: each character of a character large
/// object of characters of 1 to 4 octets is read where it is, forward,
/// backward and across a chunk's end, and none past its end.
#[test]
fn each_character_is_read_where_it_is() {
    for per in 1..=4 {
        let temp = Temp::default();
        let s = straddling(per);
        let lob = chunked(&temp, &s);
        let chars: Vec<char> = s.chars().collect();
        let t = LobText::new(&lob).unwrap();
        assert_eq!(t.len(), chars.len());
        for (i, c) in chars.iter().enumerate() {
            assert_eq!(t.at(i).unwrap(), Some(*c), "{per} {i}");
        }
        let edge = first_chunk_chars(&lob);
        for i in (edge - 3..edge + 3).rev() {
            assert_eq!(t.at(i).unwrap(), Some(chars[i]), "{per} {i} backward");
        }
        assert_eq!(t.at(chars.len()).unwrap(), None);
    }
}

/// Verifies: [DESIGN] RULES.md D61, D79: a match crossing a chunk's end,
/// of characters of 1 to 4 octets, is found where it is in the string.
#[test]
fn a_match_crosses_a_chunks_end() {
    for per in 1..=4 {
        let temp = Temp::default();
        let s = straddling(per);
        let lob = chunked(&temp, &s);
        let chars: Vec<char> = s.chars().collect();
        let t = LobText::new(&lob).unwrap();
        let r = Regex::new(&format!("<{}+>", wide(per)), "").unwrap();
        let found = r.find_in(&t, 0).unwrap().unwrap().get(0).unwrap();
        assert_eq!(Some(found), r.find_at(&chars, 0).unwrap().unwrap().get(0));
        let edge = first_chunk_chars(&lob);
        assert!(found.0 < edge && edge < found.1, "{per}: {found:?} crosses {edge}");
        let dot = Regex::new("<(.)(.)\\2", "").unwrap();
        assert_eq!(dot.find_in(&t, 0).unwrap(), dot.find_at(&chars, 0).unwrap(), "{per}");
    }
}

/// Verifies: [DESIGN] RULES.md D61, D79 [F&O 3.1 §5.6.1.4]: a
/// back-reference to a capture before a chunk's end matches its repeat
/// after it, as it does in the string, and under the flag `i` a case
/// variant; one that does not repeat it matches nothing.
#[test]
fn a_back_reference_reaches_across_a_chunks_end() {
    let temp = Temp::default();
    let word = "Zebra€Crossing😀";
    let before = CHUNK_OCTETS - word.len() - 4;
    let s = format!("{}#{word}#{}{}", "a".repeat(before), word.to_lowercase(), "b".repeat(1000));
    let lob = chunked(&temp, &s);
    let chars: Vec<char> = s.chars().collect();
    let t = LobText::new(&lob).unwrap();
    let edge = first_chunk_chars(&lob);
    let caseless = Regex::new("#([^#]+)#\\1", "i").unwrap();
    let found = caseless.find_in(&t, 0).unwrap();
    assert_eq!(found, caseless.find_at(&chars, 0).unwrap());
    let (capture, whole) = (found.as_ref().unwrap().get(1).unwrap(), found.unwrap().get(0).unwrap());
    assert!(capture.1 <= edge && edge < whole.1, "{capture:?} before {edge}, its repeat after");
    let exact = Regex::new("#([^#]+)#\\1", "").unwrap();
    assert_eq!(exact.find_in(&t, 0).unwrap(), None);
}

/// Verifies: [DESIGN] RULES.md D79: the octet each character begins at,
/// and the character beginning at or after each octet, around a chunk's
/// end, are those of the string.
#[test]
fn octets_and_characters_around_a_chunks_end() {
    for per in 2..=4 {
        let temp = Temp::default();
        let s = straddling(per);
        let lob = chunked(&temp, &s);
        let t = LobText::new(&lob).unwrap();
        let starts: Vec<usize> = s.char_indices().map(|(o, _)| o).collect();
        for o in CHUNK_OCTETS - 12..CHUNK_OCTETS + 12 {
            let expected = starts.iter().filter(|start| **start < o).count();
            assert_eq!(t.char_of_octet(u64::try_from(o).unwrap()).unwrap(), expected, "{per} octet {o}");
        }
        let edge = first_chunk_chars(&lob);
        for (i, start) in starts.iter().enumerate().take(edge + 3).skip(edge - 3) {
            assert_eq!(t.octet_of_char(i).unwrap(), u64::try_from(*start).unwrap(), "{per} char {i}");
        }
        assert_eq!(t.char_of_octet(lob.octets()).unwrap(), starts.len());
    }
}

/// Verifies: [DESIGN] RULES.md D79: a value of a slice beginning within a
/// chunk, a part held inline longer than a chunk, and a short one, is
/// read character by character as the string they make.
#[test]
fn slices_and_parts_held_inline() {
    let temp = Temp::default();
    let s = straddling(3);
    let sliced = chunked(&temp, &s).slice_chars(7, 100_000).unwrap();
    let long: String = std::iter::repeat_n("é😀a", 100_000).collect();
    let lob = sliced.concat(&Lob::of_text(&long)).concat(&Lob::of_text("end"));
    let whole = text(&lob);
    let chars: Vec<char> = whole.chars().collect();
    let t = LobText::new(&lob).unwrap();
    assert_eq!(t.len(), chars.len());
    for i in (0..chars.len()).step_by(7).chain(chars.len() - 10..chars.len()) {
        assert_eq!(t.at(i).unwrap(), Some(chars[i]), "{i}");
    }
    let starts: Vec<usize> = whole.char_indices().map(|(o, _)| o).collect();
    for i in [0, 99_999, 100_000, 100_001, 250_000, chars.len() - 1] {
        let o = u64::try_from(starts[i]).unwrap();
        assert_eq!(t.char_of_octet(o).unwrap(), i);
        assert_eq!(t.char_of_octet(o + 1).unwrap(), i + 1, "within character {i}");
    }
    let r = Regex::new("€+é", "").unwrap();
    assert_eq!(r.find_in(&t, 0).unwrap(), r.find_at(&chars, 0).unwrap());
}

/// Verifies: [DESIGN] RULES.md D79: a search of a value of four chunks
/// that finds nothing reads each chunk about once, holding two at most,
/// and a back-reference going back across a chunk's end reads neither
/// again and again.
#[test]
fn a_search_reads_each_chunk_about_once() {
    let temp = Temp::default();
    let s = "ab".repeat(2 * CHUNK_OCTETS);
    let lob = chunked(&temp, &s);
    let t = LobText::new(&lob).unwrap();
    let before = temp.reads();
    assert!(!Regex::new("ba(c|d)", "").unwrap().is_match_in(&t).unwrap());
    assert!(temp.reads() - before <= 6, "{} reads", temp.reads() - before);
    let again = temp.reads();
    let edge = first_chunk_chars(&lob);
    let found = Regex::new("(ab)\\1\\1", "").unwrap().find_in(&t, edge - 3).unwrap().unwrap();
    assert_eq!(found.get(0), Some((edge - 2, edge + 4)));
    assert!(temp.reads() - again <= 3, "{} reads", temp.reads() - again);
}
