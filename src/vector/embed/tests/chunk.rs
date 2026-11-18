//! Chunking's boundaries: each unit, overlap, SPLIT_ON and NORMALIZE
//! WHITESPACE, offsets in characters, and the specification's text.

use crate::error::Code;
use crate::vector::chunk::{Chunk, Span, Spec, Unit, words_of};

/// The chunks of `text` by the specification `spec`, words its tokens.
fn chunks(text: &str, spec: &str) -> Vec<Chunk> {
    let words = |t: &str| -> Vec<Span> { words_of(&t.chars().collect::<Vec<_>>()) };
    Spec::parse(spec).unwrap().compiled().unwrap().chunks(text, &words).unwrap()
}

/// Each chunk's offset, length and text.
fn spans(found: &[Chunk]) -> Vec<(usize, usize, &str)> {
    found.iter().map(|c| (c.offset, c.length, c.text.as_str())).collect()
}

/// Verifies: [EXTENSION] (§12.3) BY WORDS MAX 3 OVERLAP 1 makes chunks
/// of three words, each beginning at the last word of the one before;
/// the last ends at the text's last word, with no chunk after it; ids
/// count from 1, offsets are characters from 1, and the space between
/// words is the chunk's.
#[test]
fn words_overlap_at_their_boundaries() {
    let found = chunks("a bb  c d e f g", "BY WORDS MAX 3 OVERLAP 1");
    assert_eq!(spans(&found), [(1, 7, "a bb  c"), (7, 5, "c d e"), (11, 5, "e f g")]);
    assert_eq!(found.iter().map(|c| c.id).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(spans(&chunks("a b c d", "by words, max 2")), [(1, 3, "a b"), (5, 3, "c d")], "key words in any case, commas or not");
    assert_eq!(spans(&chunks("a b c", "BY WORDS MAX 5")), [(1, 5, "a b c")]);
    assert_eq!(chunks("   ", "BY WORDS MAX 5"), [], "an empty text has no chunks");
}

/// Verifies: [EXTENSION] (§12.3) BY CHARS counts Unicode characters,
/// not octets, and offsets count them too.
#[test]
fn characters_count_unicode_scalars() {
    assert_eq!(spans(&chunks("abcdef", "BY CHARS MAX 4 OVERLAP 2")), [(1, 4, "abcd"), (3, 4, "cdef")]);
    assert_eq!(spans(&chunks("héllo wörld", "BY CHARACTERS MAX 6")), [(1, 6, "héllo "), (7, 5, "wörld")]);
}

/// Verifies: [EXTENSION] (§12.3) A sentence ends after '.', '!' or '?'
/// and its closing quotes before white space, not inside a number or a
/// name; a blank line ends a paragraph and a sentence; both are trimmed.
#[test]
fn sentences_and_paragraphs_end_where_they_should() {
    let text = "It costs 3.50 dollars. \"Really?\" Yes!\n  \nNew one.";
    let sentences = chunks(text, "BY SENTENCES MAX 1");
    assert_eq!(
        sentences.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
        ["It costs 3.50 dollars.", "\"Really?\"", "Yes!", "New one."]
    );
    let paragraphs = chunks("One.\nstill one.\n \t\nTwo.\n\n\n\nThree", "BY PARAGRAPHS MAX 1");
    assert_eq!(paragraphs.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["One.\nstill one.", "Two.", "Three"]);
    let pairs = chunks(text, "BY SENTENCES MAX 2 OVERLAP 1");
    assert_eq!(pairs.len(), 3);
    assert_eq!(pairs[1].text, "\"Really?\" Yes!");
}

/// Verifies: [EXTENSION] (§12.3) SPLIT_ON cuts first: no chunk crosses a
/// match, and the match is in none; NORMALIZE WHITESPACE makes each run
/// of white space one space, its offset and length still the text's.
#[test]
fn split_on_and_normalize_whitespace() {
    let found = chunks("a b c --- d  e\nf", "BY WORDS MAX 10 SPLIT_ON '-+' NORMALIZE WHITESPACE");
    assert_eq!(spans(&found), [(1, 5, "a b c"), (11, 6, "d e f")]);
    let quoted = chunks("x'y z", "BY WORDS MAX 1 SPLIT_ON ''''");
    assert_eq!(quoted.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["x", "y", "z"], "a doubled quote is a quote");
}

/// Verifies: [EXTENSION] (§12.3) A specification written out reads back
/// equal; TOKENS names its model; defaults are BY WORDS MAX 256 OVERLAP 0;
/// one naming a part twice, an unknown word, MAX 0 or past 1000000,
/// OVERLAP not less than MAX, NORMALIZE alone, an unclosed quote or a
/// bad SPLIT_ON is refused.
#[test]
fn specifications_read_and_refuse() {
    let spec = Spec::parse("BY TOKENS hash_dense MAX 64 OVERLAP 8 SPLIT_ON '\\n' NORMALIZE WHITESPACE").unwrap();
    assert_eq!((spec.by, spec.model.as_deref(), spec.max, spec.overlap, spec.normalize), (Unit::Tokens, Some("HASH_DENSE"), 64, 8, true));
    assert_eq!(Spec::parse(&spec.written()).unwrap(), spec);
    assert_eq!(Spec::parse("").unwrap(), Spec::default());
    assert_eq!((Spec::default().by, Spec::default().max), (Unit::Words, 256));
    for bad in ["MAX 2 MAX 3", "BY LINES", "MAX 0", "MAX 1000001", "MAX 4 OVERLAP 4", "NORMALIZE", "SPLIT_ON 'x", "MAX many", "BY TOKENS"] {
        assert_eq!(Spec::parse(bad).unwrap_err().code, Code::Data, "{bad}");
    }
    assert!(Spec::parse("SPLIT_ON '('").unwrap().compiled().is_err());
}
