use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};

use super::*;
use crate::value::{DataType, Value};

/// Half the capacity: the characters of two octets that fill it.
const HALF: usize = INLINE >> 1;

/// A string of `n` octets, one to a character.
fn ascii(n: usize) -> String {
    "abcdefghij".chars().cycle().take(n).collect()
}

/// `text` in the shared form, whatever its length: the form a string that
/// would fit in the value is never given, made here to compare the forms.
fn shared(text: &str) -> CharString {
    CharString(Form::Shared(Arc::from(text)))
}

/// Verifies: [DESIGN] (RULES.md D960) a character string takes no more
/// room than the widest value did before it, so a value is of 48 octets
/// still.
#[test]
fn a_value_does_not_grow() {
    assert_eq!(size_of::<CharString>(), 40);
    assert_eq!(size_of::<Value>(), 48);
}

/// Verifies: [DESIGN] (RULES.md D960) a string of at most `INLINE` octets
/// is held in the value and a longer one in a buffer, and either gives
/// back its text, its octets and its length.
#[test]
fn a_string_is_held_inline_up_to_the_capacity() {
    for n in [0, 1, INLINE - 1, INLINE, INLINE + 1, 200] {
        let text = ascii(n);
        let s = CharString::from(text.as_str());
        assert_eq!(s.is_inline(), n <= INLINE, "{n} octets");
        assert_eq!((s.as_str(), s.as_bytes(), s.len(), s.is_empty()), (text.as_str(), text.as_bytes(), n, n == 0));
        assert_eq!(CharString::from(text.clone()), s, "from a String");
        assert_eq!(String::from(s), text);
    }
    assert!(CharString::new().is_inline() && CharString::default().is_empty());
}

/// Verifies: [DESIGN] (RULES.md D960) the capacity counts octets, not
/// characters: a string whose last character ends at the capacity is held
/// in the value, one whose last character crosses it is not, and neither
/// is cut within a character.
#[test]
fn a_character_of_several_octets_at_the_capacity_is_whole() {
    let fits = "é".repeat(HALF);
    let crosses = format!("a{fits}");
    let (held, long) = (CharString::from(fits.as_str()), CharString::from(crosses.as_str()));
    assert!(held.is_inline() && !long.is_inline());
    assert_eq!((held.as_str(), held.len(), held.char_count()), (fits.as_str(), INLINE, HALF));
    assert_eq!((long.as_str(), long.len(), long.char_count()), (crosses.as_str(), INLINE + 1, HALF + 1));
    let four = format!("{}😀", ascii(INLINE - 4));
    assert_eq!(CharString::from(four.as_str()).as_str(), four);
    assert!(CharString::from(four.as_str()).is_inline());
    // A character NUL is text as any other: the zeros past a string held
    // in the value are not taken for it.
    let nul = CharString::from("a\0");
    assert_eq!((nul.as_str(), nul.len()), ("a\0", 2));
}

/// Verifies: [DESIGN] (RULES.md D960) octets not UTF-8 make no string, in
/// either form.
#[test]
fn octets_not_utf8_make_no_string() {
    assert_eq!(CharString::from_utf8(b"caf\xC3\xA9").as_deref(), Some("café"));
    assert_eq!(CharString::from_utf8(b"caf\xC3"), None);
    let mut long = ascii(INLINE + 8).into_bytes();
    assert!(CharString::from_utf8(&long).is_some_and(|s| !s.is_inline()));
    long.push(0xFF);
    assert_eq!(CharString::from_utf8(&long), None);
}

/// Verifies: [DESIGN] (RULES.md D960) a clone of a long string shares its
/// buffer, copying nothing; a clone of a short one is the value again.
#[test]
fn a_clone_of_a_long_string_shares_its_buffer() {
    let long = CharString::from(ascii(INLINE + 1));
    let clone = long.clone();
    assert!(std::ptr::eq(long.as_str(), clone.as_str()));
    let short = CharString::from("short");
    let again = short.clone();
    assert_eq!(short, again);
    assert!(!std::ptr::eq(short.as_bytes(), again.as_bytes()));
}

/// Verifies: [DESIGN] (RULES.md D960) equal text is equal, orders equal
/// and hashes equal whatever the form holding it, and text orders by its
/// octets across the forms.
#[test]
fn the_forms_compare_and_hash_as_their_text() {
    let (held, buffered) = (CharString::from("same text"), shared("same text"));
    assert!(held.is_inline() && !buffered.is_inline());
    assert_eq!(held, buffered);
    assert_eq!(held.cmp(&buffered), Ordering::Equal);
    let hasher = RandomState::new();
    assert_eq!(hasher.hash_one(&held), hasher.hash_one(&buffered));
    assert_eq!(hasher.hash_one(&held), hasher.hash_one("same text"), "as the str hashes");
    assert_ne!(held, shared("same texu"));
    let (short, long) = (CharString::from(ascii(INLINE)), CharString::from(ascii(INLINE + 1)));
    assert_eq!((short.cmp(&long), long.cmp(&short)), (Ordering::Less, Ordering::Greater));
    let b = CharString::from("b");
    assert_eq!((b.cmp(&long), shared("a").cmp(&b), long.partial_cmp(&b)), (Ordering::Greater, Ordering::Less, Some(Ordering::Less)));
    assert_eq!(CharString::from("é").cmp(&CharString::from("z")), "é".cmp("z"));
}

/// Verifies: [DESIGN] (RULES.md D960) a table keyed by character strings
/// is asked by a `str`, of a key in either form.
#[test]
fn a_table_keyed_by_strings_is_asked_by_a_str() {
    let long = ascii(INLINE + 5);
    let table: HashMap<CharString, u8> = [(CharString::from("k"), 1), (CharString::from(long.as_str()), 2)].into();
    assert_eq!((table.get("k"), table.get(long.as_str()), table.get("other")), (Some(&1), Some(&2), None));
}

/// Verifies: [X3.135-1986 §8.7 GR7b] a string padded with spaces is the
/// text and the spaces, held in the value where the whole fits and in one
/// buffer where it does not, of any length.
#[test]
fn a_string_is_padded_in_either_form() {
    for (text, pad) in [("", 0), ("ab", 0), ("ab", 3), ("", INLINE), ("ab", INLINE - 2), ("ab", INLINE - 1), ("é", INLINE - 2)] {
        let padded = CharString::padded(text, pad);
        assert_eq!(padded.as_str(), format!("{text}{}", " ".repeat(pad)), "{text:?} + {pad}");
        assert_eq!(padded.is_inline(), text.len() + pad <= INLINE, "{text:?} + {pad}");
    }
    for pad in [STACKED - 3, STACKED - 2, STACKED - 1, STACKED, 4 * STACKED] {
        let padded = CharString::padded("ab", pad);
        assert_eq!(padded.as_str(), format!("ab{}", " ".repeat(pad)), "past the stack at {pad}");
    }
    let long = ascii(STACKED + 1);
    assert_eq!(CharString::padded(&long, 2).as_str(), format!("{long}  "));
    assert_eq!(CharString::padded(&long, 0).as_str(), long);
}

/// Verifies: [X3.135-1986 §5.11 GR5] a string's octets without its
/// trailing spaces, and its characters counted over its octets, in either
/// form.
#[test]
fn a_string_is_trimmed_and_counted_over_its_octets() {
    for text in ["", " ", "a", "a  ", " a ", "é  ", "a b"] {
        let long = format!("{}{text}", ascii(INLINE));
        for s in [CharString::from(text), CharString::from(long.as_str())] {
            assert_eq!(s.unpadded(), s.as_str().trim_end_matches(' ').as_bytes(), "{s:?}");
            assert_eq!(s.char_count(), s.as_str().chars().count(), "{s:?}");
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D960) two strings joined are both texts
/// in order, held in the value where they fit together.
#[test]
fn strings_are_joined_in_either_form() {
    let half = ascii(HALF);
    let whole = CharString::from(half.as_str()).joined(&half);
    assert!(whole.is_inline());
    assert_eq!(whole.as_str(), format!("{half}{half}"));
    let over = whole.joined("x");
    assert!(!over.is_inline());
    assert_eq!(over.as_str(), format!("{half}{half}x"));
    assert_eq!(over.joined("yz").as_str(), format!("{half}{half}xyz"));
    assert_eq!(CharString::new().joined("").as_str(), "");
    assert_eq!(CharString::from("é").joined("ü").as_str(), "éü");
}

/// Verifies: [DESIGN] (RULES.md D960) a character string is made of what
/// a `String` is made of, in either form.
#[test]
fn a_string_is_made_of_what_a_string_is() {
    let long = ascii(INLINE + 1);
    for text in ["it's", long.as_str()] {
        let s = CharString::from(text);
        assert_eq!(CharString::from(Box::<str>::from(text)), s);
        assert_eq!(CharString::from(Arc::<str>::from(text)), s);
        assert_eq!(CharString::from(Cow::Borrowed(text)), s);
        assert_eq!(CharString::from(Cow::<str>::Owned(text.to_owned())), s);
        assert_eq!(CharString::from(&text.to_owned()), s);
        assert_eq!(text.chars().collect::<CharString>(), s);
        assert_eq!(text.chars().collect::<Vec<_>>().iter().collect::<CharString>(), s);
        assert_eq!([text, ""].into_iter().collect::<CharString>(), s);
        assert_eq!([text.to_owned()].into_iter().collect::<CharString>(), s);
        assert_eq!((String::from(&s), s.clone().into_string()), (text.to_owned(), text.to_owned()));
    }
    assert_eq!(CharString::from('é').as_str(), "é");
    assert!(CharString::from(Arc::<str>::from("short")).is_inline());
}

/// Verifies: [DESIGN] (RULES.md D960) a character string shows as its
/// text does, equals a `str` and a `String` of its text from either side,
/// and lends its text as a `str`.
#[test]
fn a_string_shows_and_compares_as_its_text() {
    let long = ascii(INLINE + 1);
    for text in ["it's", long.as_str()] {
        let (s, owned) = (CharString::from(text), text.to_owned());
        assert_eq!((format!("{s}"), format!("{s:?}"), format!("{s:>40}")), (owned.clone(), format!("{text:?}"), format!("{text:>40}")));
        assert_eq!(s, text);
        assert_eq!(text, s);
        assert_eq!(s, *text);
        assert_eq!(*text, s);
        assert_eq!(s, owned);
        assert_eq!(owned, s);
        assert_eq!((AsRef::<str>::as_ref(&s), Borrow::<str>::borrow(&s)), (text, text));
        assert_eq!(s.to_uppercase(), text.to_uppercase(), "a str's methods, by Deref");
        assert_ne!(s, "other");
    }
}

/// Verifies: [X3.135-1986 §8.7 GR7b] a CHARACTER read from its record is
/// padded to its length, held in the value where it fits there, and is
/// written without its padding; a CHARACTER VARYING is read as written.
#[test]
fn a_fixed_string_is_read_padded_in_either_form() {
    for length in [5_u32, 32, 33, 300] {
        let (fixed, varying) = (DataType::character(length), DataType::varying(length));
        let width = usize::try_from(length).unwrap();
        let read = fixed.decode_record("é b".as_bytes()).unwrap();
        let Value::Char(s) = &read else { panic!("{read:?}") };
        assert_eq!(s.as_str(), format!("{:<width$}", "é b"), "CHARACTER({length})");
        assert_eq!(s.is_inline(), width < INLINE, "CHARACTER({length})");
        assert_eq!(fixed.kind().encode_record(&read, &fixed).unwrap(), "é b".as_bytes());
        let kept = varying.decode_record(b"ab  ").unwrap();
        assert!(matches!(&kept, Value::Char(s) if s == "ab  "), "{kept:?}");
        assert_eq!(varying.kind().encode_record(&kept, &varying).unwrap(), b"ab  ");
    }
    assert!(DataType::character(5).decode_record(b"\xFF").is_none() && DataType::varying(5).decode_record(b"\xFF").is_none());
    assert!(DataType::character(2).decode_record(b"abc").is_none(), "longer than its type");
}
