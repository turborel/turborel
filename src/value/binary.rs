//! [ISO/IEC 9075-2:2023 §4.4.1] A binary string's value: a sequence of
//! octets. Two compare octet by octet [§8.2 GR5], a string before every
//! longer one it begins, so trailing X'00' octets count: the choice of
//! IA219 that a string beginning another and followed by X'00' octets is
//! less than it, and of IA022 that trailing X'00' octets are significant.
//!
//! Its key orders as the strings compare, and no key begins another. Each
//! octet is itself, but a 0 octet is followed by `ESCAPE`, and the key
//! ends in a 0 octet and `END`, which orders below every escaped 0.
//!
//! ```text
//! key: { octet other than 0 | 0 ESCAPE (255) }... 0 END (0)
//! ```
//!
//! \[DESIGN\] (RULES.md D961) A string of at most `INLINE` octets is held
//! in the value itself, with no allocation. A longer one is one buffer,
//! never changed, that every clone of the value shares.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// The bits of an octet.
pub const BYTE_BITS: u32 = u8::BITS;
/// A key's marker after a 0 octet of the string.
const ESCAPE: u8 = u8::MAX;
/// A key's marker after the 0 octet that ends it.
const END: u8 = 0;
/// The radix of a `<hexit>` [§5.3].
const HEX_RADIX: u32 = 16;

/// The octets a binary string held in its value takes at most: what
/// leaves the value three words, as a `String` is, so nothing that holds
/// one grows past what holds a character string's text (RULES.md D961).
pub const INLINE: usize = 22;

/// A binary string's value.
#[derive(Clone)]
pub struct Binary(Form);

/// How a binary string is held.
#[derive(Clone)]
enum Form {
    /// In the value: its first `len` octets.
    Inline {
        /// The octets of the string.
        len: u8,
        /// The string, then zeros.
        octets: [u8; INLINE],
    },
    /// In a buffer its clones share.
    Shared(Arc<[u8]>),
}

/// `octets` in the form held in a value, if they are few enough.
fn held(octets: &[u8]) -> Option<Form> {
    let mut all = [0_u8; INLINE];
    all.get_mut(..octets.len())?.copy_from_slice(octets);
    Some(Form::Inline { len: u8::try_from(octets.len()).ok()?, octets: all })
}

impl Binary {
    /// The string of `octets`.
    pub fn new(octets: Vec<u8>) -> Binary {
        Binary(held(&octets).unwrap_or_else(|| Form::Shared(Arc::from(octets))))
    }

    /// The string of `octets`, copied from where they are: into the value
    /// with no allocation if they fit.
    pub fn of(octets: &[u8]) -> Binary {
        Binary(held(octets).unwrap_or_else(|| Form::Shared(Arc::from(octets))))
    }

    /// Whether it is held in the value, with no buffer.
    pub fn is_inline(&self) -> bool {
        matches!(self.0, Form::Inline { .. })
    }

    /// The octets `hexits` writes, two to each [ISO/IEC 9075-2:2023 §5.3
    /// SR4], spaces between them passed over; none if a character is
    /// neither a `<hexit>` nor a space, or the hexits are odd in number.
    pub fn of_hex(hexits: &str) -> Option<Binary> {
        let digits: Vec<u32> = hexits.chars().filter(|&c| c != ' ').map(|c| c.to_digit(HEX_RADIX)).collect::<Option<_>>()?;
        let (pairs, odd) = digits.as_chunks::<2>();
        if !odd.is_empty() {
            return None;
        }
        let octet = |&[high, low]: &[u32; 2]| u8::try_from(high.checked_mul(HEX_RADIX)?.checked_add(low)?).ok();
        pairs.iter().map(octet).collect::<Option<Vec<u8>>>().map(Binary::new)
    }

    /// Its octets.
    pub fn bytes(&self) -> &[u8] {
        match &self.0 {
            Form::Inline { len, octets } => octets.get(..usize::from(*len)).unwrap_or_default(),
            Form::Shared(octets) => octets,
        }
    }

    /// How many octets it has, the most a `u32` counts.
    pub fn len(&self) -> u32 {
        u32::try_from(self.bytes().len()).unwrap_or(u32::MAX)
    }

    /// Whether it has no octets.
    pub fn is_empty(&self) -> bool {
        self.bytes().is_empty()
    }

    /// The octets `from` (the first 1) on, `count` of them at most, or all
    /// to its end.
    #[must_use]
    pub fn substring(&self, from: u32, count: u32) -> Binary {
        let start = from.saturating_sub(1).min(self.len());
        let end = start.saturating_add(count).min(self.len());
        let range = usize::try_from(start).unwrap_or(usize::MAX)..usize::try_from(end).unwrap_or(usize::MAX);
        Binary::of(self.bytes().get(range).unwrap_or_default())
    }

    /// Its octets, then `other`'s [ISO/IEC 9075-2:2023 §6.32 GR3].
    #[must_use]
    pub fn concat(&self, other: &Binary) -> Binary {
        Binary::new([self.bytes(), other.bytes()].concat())
    }

    /// The position of `needle`'s first occurrence in it, the first octet
    /// 1, or 0 if there is none; 1 if `needle` is empty [ISO/IEC
    /// 9075-2:2023 §6.31 GR3].
    pub fn position(&self, needle: &Binary) -> u32 {
        if needle.is_empty() {
            return 1;
        }
        let at = self.bytes().windows(needle.bytes().len()).position(|w| w == needle.bytes());
        at.and_then(|i| u32::try_from(i).ok()).map_or(0, |i| i.saturating_add(1))
    }

    /// Appends its key to `out`, as the module's documentation says.
    pub fn put_key(&self, out: &mut Vec<u8>) {
        for &b in self.bytes() {
            out.push(b);
            if b == 0 {
                out.push(ESCAPE);
            }
        }
        out.extend_from_slice(&[0, END]);
    }

    /// The binary string whose key begins `rest`, which then holds the
    /// bytes after it; none if it begins with no key.
    pub fn take_key(rest: &mut &[u8]) -> Option<Binary> {
        let mut octets = Vec::new();
        loop {
            let (&b, after) = rest.split_first()?;
            *rest = after;
            if b != 0 {
                octets.push(b);
                continue;
            }
            let (&marker, past) = rest.split_first()?;
            *rest = past;
            match marker {
                ESCAPE => octets.push(0),
                END => return Some(Binary::new(octets)),
                1..=254 => return None,
            }
        }
    }
}

/// Its octets as hexits, two to each, as a `<binary string literal>`
/// writes them [ISO/IEC 9075-2:2023 §5.3].
impl fmt::Display for Binary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.bytes().iter().try_for_each(|b| write!(f, "{b:02X}"))
    }
}

/// The empty string.
impl Default for Binary {
    fn default() -> Binary {
        Binary(Form::Inline { len: 0, octets: [0; INLINE] })
    }
}

/// Its octets, whatever the form.
impl fmt::Debug for Binary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Binary").field("octets", &self.bytes()).finish()
    }
}

/// Equal octets are equal whatever the form.
impl PartialEq for Binary {
    fn eq(&self, other: &Binary) -> bool {
        self.bytes() == other.bytes()
    }
}

impl Eq for Binary {}

/// Octet by octet, a string before every longer one it begins [ISO/IEC
/// 9075-2:2023 §8.2 GR5].
impl Ord for Binary {
    fn cmp(&self, other: &Binary) -> Ordering {
        self.bytes().cmp(other.bytes())
    }
}

impl PartialOrd for Binary {
    fn partial_cmp(&self, other: &Binary) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// As its octets hash, so equal strings hash equal whatever the form.
impl Hash for Binary {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bytes().hash(state);
    }
}

// Unit tests: a binary string's literals, operations and keys (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;

    /// Every binary string of up to `most` octets, each octet 0, 1 or 255.
    fn every(most: usize) -> Vec<Binary> {
        let mut all = vec![Vec::new()];
        let mut last = vec![Vec::new()];
        for _ in 0..most {
            last = last.iter().flat_map(|s: &Vec<u8>| [0_u8, 1, 255].map(|b| [s.as_slice(), &[b]].concat())).collect();
            all.extend(last.iter().cloned());
        }
        all.into_iter().map(Binary::new).collect()
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.2 GR5] the keys of
    /// binary strings, compared as bytes, order every two of up to 4
    /// octets of 0, 1 or 255 as they compare, octet by octet, a string
    /// before every longer one it begins, trailing X'00' octets and all;
    /// no key begins another, so two columns' keys order as the pairs;
    /// each reads back the same.
    #[test]
    fn binary_keys_order_as_binary_strings_compare() {
        let all = every(4);
        assert_eq!(all.len(), 121);
        let keys: Vec<Vec<u8>> = all
            .iter()
            .map(|b| {
                let mut out = Vec::new();
                b.put_key(&mut out);
                out
            })
            .collect();
        for (a, ka) in all.iter().zip(&keys) {
            let mut rest = ka.as_slice();
            assert_eq!(Binary::take_key(&mut rest).as_ref(), Some(a), "{a} reads back");
            assert!(rest.is_empty(), "{a}'s key is read whole");
            for (b, kb) in all.iter().zip(&keys) {
                assert_eq!(a.cmp(b), a.bytes().cmp(b.bytes()), "{a} and {b} compare octet by octet");
                assert_eq!(ka.cmp(kb), a.cmp(b), "the keys of {a} and {b}");
                assert!(ka == kb || !kb.starts_with(ka), "{a}'s key begins {b}'s");
            }
        }
        assert!(Binary::new(vec![1]) < Binary::new(vec![1, 0]), "IA219: a trailing X'00' counts");
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §5.3 SR4, §6.31 GR3,
    /// §6.32 GR3] a literal's hexits make octets, two to each, spaces
    /// passed over, even within a pair, but no other character and no
    /// odd hexit; SUBSTRING
    /// takes octets from a position, as many as there are at most; ||
    /// joins two; POSITION finds the first occurrence, 1 for the empty
    /// string and 0 for none; a string shows as its hexits.
    #[test]
    fn binary_strings_are_made_cut_joined_and_searched() {
        let s = Binary::of_hex("0a ff 1B").unwrap();
        assert_eq!((s.bytes(), s.len(), s.to_string().as_str()), (&[0x0A, 0xFF, 0x1B][..], 3, "0AFF1B"));
        assert!(Binary::of_hex("ABC").is_none() && Binary::of_hex("G0").is_none() && Binary::of_hex("A\tB").is_none());
        assert_eq!(Binary::of_hex(" A B ").map(|b| b.to_string()), Some("AB".to_owned()), "a space even within a pair");
        assert_eq!(Binary::of_hex("").map(|b| b.is_empty()), Some(true));
        assert_eq!(s.substring(2, 1).to_string(), "FF");
        assert_eq!(s.substring(2, 9).to_string(), "FF1B", "to its end");
        assert!(s.substring(9, 1).is_empty() && s.substring(0, 1).to_string() == "0A");
        assert_eq!(s.concat(&Binary::new(vec![0])).to_string(), "0AFF1B00");
        let needle = |h: &str| s.position(&Binary::of_hex(h).unwrap());
        assert_eq!((needle("FF1B"), needle(""), needle("1B0A"), needle("0A")), (2, 1, 0, 1));
    }

    /// Verifies: [DESIGN] (RULES.md D961) a binary string of at most
    /// `INLINE` octets is held in its value and a longer one in a buffer
    /// its clones share; either gives back its octets, made of a vector or
    /// of a slice, and a value is no wider for it.
    #[test]
    fn a_binary_string_is_held_inline_up_to_the_capacity() {
        assert_eq!((size_of::<Binary>(), size_of::<crate::value::Value>()), (24, 48));
        for n in [0, 1, INLINE - 1, INLINE, INLINE + 1, 300] {
            let octets: Vec<u8> = (0..=u8::MAX).cycle().take(n).collect();
            let (made, copied) = (Binary::new(octets.clone()), Binary::of(&octets));
            assert_eq!((made.is_inline(), copied.is_inline()), (n <= INLINE, n <= INLINE), "{n} octets");
            assert_eq!((made.bytes(), copied.bytes(), usize::try_from(made.len()).unwrap()), (&octets[..], &octets[..], n));
            let clone = made.clone();
            assert_eq!(clone, made);
            assert_eq!(std::ptr::eq(clone.bytes(), made.bytes()), n > INLINE, "a long string's clone shares its buffer");
        }
        assert!(Binary::default().is_inline() && Binary::default().is_empty());
    }

    /// Verifies: [ISO/IEC 9075-2:2023 §8.2 GR5] equal octets are equal,
    /// order equal and hash equal whatever holds them, a string orders
    /// before every longer one it begins across the forms, and a string
    /// shows its octets alone.
    #[test]
    fn the_forms_of_a_binary_string_compare_and_hash_as_their_octets() {
        use std::hash::{BuildHasher, RandomState};
        let (held, buffered) = (Binary::of(&[1, 0, 2]), Binary(Form::Shared(Arc::from(&[1_u8, 0, 2][..]))));
        assert!(held.is_inline() && !buffered.is_inline());
        assert_eq!(held, buffered);
        assert_eq!((held.cmp(&buffered), held.partial_cmp(&buffered)), (Ordering::Equal, Some(Ordering::Equal)));
        let hasher = RandomState::new();
        assert_eq!(hasher.hash_one(&held), hasher.hash_one(&buffered));
        assert_eq!(hasher.hash_one(&held), hasher.hash_one(&[1_u8, 0, 2][..]), "as its octets hash");
        assert_ne!(held, Binary::of(&[1, 0, 3]));
        let (short, long) = (Binary::of(&[7; INLINE]), Binary::of(&[7; INLINE + 1]));
        assert_eq!(
            (short.cmp(&long), long.cmp(&short), Binary::of(&[8]).cmp(&long)),
            (Ordering::Less, Ordering::Greater, Ordering::Greater)
        );
        assert_eq!(format!("{held:?}"), "Binary { octets: [1, 0, 2] }");
        assert_eq!(format!("{buffered:?}"), format!("{held:?}"));
        let joined = short.concat(&Binary::of(&[9]));
        assert_eq!((joined.bytes().len(), joined.bytes().last(), joined.is_inline()), (INLINE + 1, Some(&9), false));
        assert_eq!(long.substring(2, 3), Binary::of(&[7; 3]));
    }

    /// Verifies: [DESIGN] a key that is cut short, or of a marker after a
    /// 0 octet no key has, reads as none.
    #[test]
    fn malformed_binary_keys_are_none() {
        for bad in [&[][..], &[0x80][..], &[0][..], &[0x80, 0, 1][..]] {
            assert!(Binary::take_key(&mut &bad[..]).is_none(), "{bad:?}");
        }
    }
}
