use super::fixtures::{Temp, text, texts};
use super::*;

/// Verifies: [DESIGN] RULES.md D79: a directory finds the chunk of an octet
/// or a character, at each chunk's first and last and past the end.
#[test]
fn a_directory_finds_chunks_by_octet_and_character() {
    let d = Directory::new(vec![Chunk { octets: 4, chars: 2 }, Chunk { octets: 3, chars: 3 }]);
    assert_eq!(d.totals(), (7, 5));
    assert_eq!(d.by_octet(0), Some((0, 0, 0)));
    assert_eq!(d.by_octet(3), Some((0, 0, 0)));
    assert_eq!(d.by_octet(4), Some((1, 4, 2)));
    assert_eq!(d.by_octet(7), None);
    assert_eq!(d.by_char(1), Some((0, 0, 0)));
    assert_eq!(d.by_char(2), Some((1, 4, 2)));
    assert_eq!(d.by_char(5), None);
}

/// Verifies: [DESIGN] RULES.md D79 [ISO/IEC 9075-2:2023 §6.32 GR2, §6.33]
/// concatenation joins parts, and slicing by characters across chunks and
/// parts keeps characters of more than one octet whole.
#[test]
fn slices_and_concatenations_copy_no_octets_of_a_source() {
    let a = texts(&["añb", "çde", "f"]);
    assert_eq!((a.octets(), a.chars()), (9, 7));
    let joined = a.concat(&Lob::of_text("gé"));
    assert_eq!(text(&joined), "añbçdefgé");
    assert_eq!(joined.chars(), 9);
    let mid = joined.slice_chars(2, 5).unwrap();
    assert_eq!(text(&mid), "bçdef");
    assert_eq!(mid.chars(), 5);
    assert!(mid.parts().iter().all(|p| matches!(p, Part::Slice(_))), "a slice of a source is a slice");
    assert_eq!(joined.reader().text(6, 10).unwrap(), "fgé");
    assert_eq!(joined.slice_chars(9, 3).unwrap().octets(), 0);
}

/// Verifies: [DESIGN] RULES.md D79: a builder keeps a chunk's worth in
/// memory and writes the rest to the temporary store in chunks cut at
/// characters' boundaries; without a store, past a chunk is refused.
#[test]
fn a_builder_writes_chunks_to_its_temporary_store() {
    let temp = Temp::default();
    let mut b = LobBuilder::new(LobKind::Character, Some(&temp));
    let piece = "é".repeat(1000);
    for _ in 0..300_u16 {
        b.push_str(&piece).unwrap();
    }
    let lob = b.finish().unwrap();
    assert_eq!(lob.chars(), 300_000);
    assert_eq!(lob.octets(), 600_000);
    assert_eq!(temp.made(), 1);
    let Part::Slice(s) = &lob.parts()[0] else { panic!("a slice") };
    assert!(
        s.source
            .directory()
            .unwrap()
            .chunks()
            .iter()
            .all(|c| usize::try_from(c.octets).unwrap() <= CHUNK_OCTETS && c.octets == c.chars * 2)
    );
    assert_eq!(lob.reader().text(299_998, 5).unwrap(), "éé");
    let mut chars = LobChars::new(&lob, 131_071).unwrap();
    assert_eq!(chars.next_char().unwrap(), Some('é'));
    let mut none = LobBuilder::new(LobKind::Binary, None);
    assert_eq!(none.push_bytes(&vec![0; CHUNK_OCTETS]).unwrap_err().code, Code::Unsupported);
}

/// Verifies: [DESIGN] RULES.md D79: a chunk of a character large object
/// ends at a character's end, whether `most` falls within a character or
/// the octets end within one, the rest of it yet to come.
#[test]
fn chunks_are_cut_between_characters() {
    let text = "ab€😀".as_bytes();
    assert_eq!(char_cut(text, 3), 2, "within the three octets of €");
    assert_eq!(char_cut(text, 5), 5, "at €'s end");
    assert_eq!(char_cut(text, 100), text.len(), "all whole");
    assert_eq!(char_cut(&text[..7], 100), 5, "😀 split by the end");
    assert_eq!(char_cut(&text[..1], 100), 1);
    assert_eq!(char_cut(&text[5..6], 100), 0, "one partial character");
}
