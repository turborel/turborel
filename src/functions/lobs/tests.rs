//! Unit tests of the streamed operations on large objects: values of many
//! chunks, characters of one to four octets crossing chunks' ends, and
//! needles and trimmed runs crossing them (RULES.md R-TEST-1, R-TEST-2).

use super::ends::{leading_chars, leading_octets, trailing_chars, trailing_octets};
use super::ops::{chars_before, fold, within, within_octets};
use super::search::{Octets, find};
use crate::error::{Code, SqlState};
use crate::value::LikePattern;
use crate::value::lob::fixtures::{Temp, chunked_by as chunked, octets_by as chunked_octets, text};
use crate::value::lob::{CHUNK_OCTETS, Lob, LobKind};

/// A text of characters of one to four octets in turn, `n` of them.
fn mixed(n: usize) -> String {
    ['a', 'é', '€', '😀', 'b', 'c'].iter().cycle().take(n).collect()
}

/// Where `needle` first occurs in `hay` by the streamed search, each held
/// or in chunks of 5 octets.
fn found(needle: &[u8], hay: &[u8]) -> Vec<Option<u64>> {
    let (n_lob, h_lob) = (chunked_octets(needle, 5), chunked_octets(hay, 5));
    let mut all = Vec::new();
    for (mut n, mut h) in [
        (Octets::Held(needle), Octets::of_lob(&h_lob)),
        (Octets::of_lob(&n_lob), Octets::of_lob(&h_lob)),
        (Octets::of_lob(&n_lob), Octets::Held(hay)),
        (Octets::Held(needle), Octets::Held(hay)),
    ] {
        all.push(find(&mut n, &mut h).unwrap());
    }
    all
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 GR2, GR3] the Two-Way
/// search finds the first occurrence a naive search does, of needles periodic
/// and not, empty, longer than the haystack, crossing chunks' ends, held in
/// memory or in a large object, either or both.
#[test]
fn the_two_way_search_finds_the_first_occurrence() {
    let hay = b"abaabaabbabaabababcabcababcabaaaabaaab".to_vec();
    let mut needles: Vec<Vec<u8>> = vec![b"".to_vec(), b"zzz".to_vec(), hay.clone(), [hay.as_slice(), b"x"].concat()];
    for from in 0..hay.len() {
        for len in 1..12 {
            needles.push(hay[from..(from + len).min(hay.len())].to_vec());
        }
    }
    needles.extend([b"aaaa".to_vec(), b"abab".to_vec(), b"abcabd".to_vec(), b"baaab".to_vec(), b"aab".to_vec()]);
    for needle in &needles {
        let naive = if needle.is_empty() {
            Some(0)
        } else {
            hay.windows(needle.len()).position(|w| w == needle.as_slice()).map(|p| u64::try_from(p).unwrap())
        };
        assert!(found(needle, &hay).iter().all(|f| *f == naive), "{:?}: {naive:?}", String::from_utf8_lossy(needle));
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 GR2b] of a large object of
/// 700000 octets in chunks of 256 KiB, a needle crossing a chunk's end, and one
/// crossing a window's, is found where it is; one not in it is not.
#[test]
fn a_needle_crossing_a_chunk_is_found_in_a_long_large_object() {
    let mut octets: Vec<u8> = (0..700_000_u32).map(|i| b'a' + u8::try_from(i % 7).unwrap()).collect();
    let at = CHUNK_OCTETS - 3;
    octets.splice(at..at + 6, *b"XYZXYZ");
    octets.splice(65_534..65_538, *b"QRST");
    let lob = chunked_octets(&octets, CHUNK_OCTETS);
    let mut hay = Octets::of_lob(&lob);
    assert_eq!(find(&mut Octets::Held(b"XYZXYZ"), &mut hay).unwrap(), Some(u64::try_from(at).unwrap()));
    assert_eq!(find(&mut Octets::Held(b"QRST"), &mut hay).unwrap(), Some(65_534));
    assert_eq!(find(&mut Octets::Held(b"XYZXYZX"), &mut hay).unwrap(), None);
    let needle = chunked_octets(&octets[299_990..400_010], 4096);
    let first = octets.windows(100_020).position(|w| w == &octets[299_990..400_010]).unwrap();
    assert_eq!(find(&mut Octets::of_lob(&needle), &mut hay).unwrap(), Some(u64::try_from(first).unwrap()), "a needle of many chunks");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 GR12e, GR13e] the
/// characters trimmed from each end of a character large object, of one to four
/// octets, crossing chunks' ends: counted from the start and from the end, the
/// end's no more than are left.
#[test]
fn trimmed_characters_are_counted_from_each_end() {
    let body = mixed(50);
    let value = format!("€€😀€{body}😀€€😀😀");
    let trimmed = |c: char| c == '€' || c == '😀';
    for size in [4, 5, 7, 64] {
        let lob = chunked(&value, size);
        assert_eq!(leading_chars(&lob, &trimmed).unwrap(), 4, "chunks of {size}");
        assert_eq!(trailing_chars(&lob, &trimmed, u64::MAX).unwrap(), 5, "chunks of {size}");
        assert_eq!(trailing_chars(&lob, &trimmed, 3).unwrap(), 3, "at most 3");
    }
    let all = chunked(&"😀".repeat(40), 9);
    assert_eq!(leading_chars(&all, &trimmed).unwrap(), 40);
    assert_eq!(trailing_chars(&all, &trimmed, 0).unwrap(), 0, "none left");
    let long = format!("{}{}", mixed(200_000), "é".repeat(70_000));
    let lob = chunked(&long, CHUNK_OCTETS);
    assert_eq!(trailing_chars(&lob, &|c| c == 'é', u64::MAX).unwrap(), 70_001, "across windows and a chunk, one é of the body too");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 GR18e] the octets equal to
/// a trim octet at each end of a binary large object, across chunks' ends, the
/// end's no more than are left.
#[test]
fn trimmed_octets_are_counted_from_each_end() {
    let mut octets = vec![0_u8; 13];
    octets.extend([1, 0, 2]);
    octets.extend(vec![0_u8; 70_000]);
    let lob = chunked_octets(&octets, 6);
    assert_eq!(leading_octets(&lob, 0).unwrap(), 13);
    assert_eq!(trailing_octets(&lob, 0, u64::MAX).unwrap(), 70_000);
    assert_eq!(trailing_octets(&lob, 0, 9).unwrap(), 9);
    let zeros = chunked_octets(&[7; 20], 3);
    assert_eq!(leading_octets(&zeros, 7).unwrap(), 20);
    assert_eq!(trailing_octets(&zeros, 7, 0).unwrap(), 0);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.32 GR2b ii 3A, GR3b i] a
/// joined large object past the longest is cut to it if what is past is spaces,
/// or X'00' of a binary one, else refused with SQLSTATE 22001 (here of a
/// longest of a few characters).
#[test]
fn a_concatenation_past_the_longest_is_cut_or_refused() {
    let joined = chunked("aé€", 3).concat(&chunked("😀  ", 4));
    assert_eq!(text(&within(joined.clone(), 6).unwrap()), "aé€😀  ");
    assert_eq!(text(&within(joined.clone(), 4).unwrap()), "aé€😀");
    let refused = within(joined, 3).unwrap_err();
    assert_eq!((refused.code, refused.state), (Code::Data, Some(SqlState::STRING_DATA_RIGHT_TRUNCATION)));
    let octets = chunked_octets(&[1, 2, 0, 0], 3);
    assert_eq!(within(octets.clone(), 2).unwrap().reader().read(0, 9).unwrap(), [1, 2]);
    assert_eq!(within(octets, 1).unwrap_err().code, Code::Data);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 GR3a] SUBSTRING USING
/// OCTETS of a character large object takes the characters wholly within the
/// octets (UV074), across chunks' ends; the characters before an octet counted.
#[test]
fn octets_of_a_character_large_object_take_whole_characters() {
    let lob = chunked("aé€😀b", 4);
    for ((from, to), expected) in
        [((0, 11), "aé€😀b"), ((1, 11), "é€😀b"), ((2, 11), "€😀b"), ((0, 6), "aé€"), ((0, 5), "aé"), ((2, 5), ""), ((10, 11), "b")]
    {
        assert_eq!(text(&within_octets(&lob, from, to).unwrap()), expected, "{from}..{to}");
    }
    assert_eq!(chars_before(&lob, 6).unwrap(), 3);
    assert_eq!(chars_before(&lob, 7).unwrap(), 4, "within the 😀");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 GR7] UPPER and LOWER of a
/// character large object of many chunks fold its simple Latin letters alone,
/// written to the temporary store past a chunk; without one, past a chunk is
/// refused.
#[test]
fn a_fold_is_written_a_chunk_at_a_time() {
    let value = format!("{}{}", "aBé€😀".repeat(60_000), "zZ");
    let lob = chunked(&value, CHUNK_OCTETS);
    let temp = Temp::default();
    let upper = fold(&lob, true, Some(&temp)).unwrap();
    assert_eq!((upper.octets(), upper.chars()), (lob.octets(), lob.chars()));
    assert_eq!(text(&upper), value.to_ascii_uppercase());
    assert_eq!(text(&fold(&lob, false, Some(&temp)).unwrap()), value.to_ascii_lowercase());
    assert_eq!(temp.made(), 2);
    assert_eq!(text(&fold(&chunked("aÉb", 2), true, None).unwrap()), "AÉB", "short, held inline");
    assert_eq!(fold(&lob, true, None).unwrap_err().code, Code::Unsupported);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.5 GR3d] LIKE of a character
/// large object read a chunk at a time matches as of its text, by patterns of
/// `%` and `_` at the ends and between, characters of one to four octets
/// crossing chunks' ends.
#[test]
fn like_of_a_large_object_matches_as_of_its_text() {
    let value = format!("ab€{}😀zé", mixed(40));
    let patterns = [
        "%",
        "_%",
        "%_",
        "ab%",
        "%zé",
        "ab€%😀zé",
        "%€%",
        "%😀%",
        "%😀_é",
        "a_€%",
        "%cab%",
        "%é€😀bc%z_",
        "ab",
        "%x%",
        "ab€%%%zé",
        "%a%a%a%a%a%a%a%a%a%",
        "_b€%",
        "%€😀bca%",
        "%zz",
        "",
    ];
    for size in [4, 6, 9] {
        let lob = chunked(&value, size);
        for p in patterns {
            let expected = crate::value::like(&value, p, None).unwrap();
            assert_eq!(LikePattern::new(p, None).unwrap().matches_lob(&lob).unwrap(), expected, "{p} in chunks of {size}");
        }
        assert!(LikePattern::new(&value, None).unwrap().matches_lob(&lob).unwrap(), "itself");
    }
    let empty = Lob::empty(LobKind::Character);
    assert!(LikePattern::new("%", None).unwrap().matches_lob(&empty).unwrap());
    assert!(!LikePattern::new("_", None).unwrap().matches_lob(&empty).unwrap());
}
