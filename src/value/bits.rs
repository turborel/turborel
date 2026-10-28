//! [ISO/IEC 9075:1992 §4.3, §6.1] A bit string's value: its bits, the
//! first the most significant bit of its first byte, and how many there
//! are; the bits past them in its last byte are 0, so two values of the
//! same bits are the same bytes.
//!
//! Its key orders as bit strings compare [§8.2 GR4], a string before every
//! longer one it begins, and no key begins another: its bytes, each
//! followed by a marker of how many of its bits are the string's, `FULL`
//! for a byte all of whose bits are and that more follow, else 0 to 8 for
//! the last. The empty string is a 0 byte and the marker 0.
//!
//! ```text
//! key: { byte | FULL (9) }... byte | count of its bits (0 to 8)
//! ```

use std::cmp::Ordering;
use std::fmt;

use crate::error::{Code, Result, SqlError};

/// The bits of a byte.
pub const BYTE_BITS: u32 = 8;
/// A key's marker: a byte all of whose bits are the string's, more
/// following.
const FULL: u8 = 9;

/// A bit string's value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Bits {
    bytes: Box<[u8]>,
    len: u32,
}

/// Bytes that hold `bits` bits.
fn bytes_for(bits: u32) -> usize {
    usize::try_from(bits.div_ceil(BYTE_BITS)).unwrap_or(usize::MAX)
}

impl Bits {
    /// The first `len` bits of `bytes`, the rest of its last byte made 0.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `bytes` holds fewer than `len` bits.
    pub fn new(mut bytes: Vec<u8>, len: u32) -> Result<Bits> {
        let need = bytes_for(len);
        if bytes.len() < need {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {} bytes do not hold {len} bits", bytes.len())));
        }
        bytes.truncate(need);
        let unused = BYTE_BITS.saturating_mul(u32::try_from(need).unwrap_or(u32::MAX)).saturating_sub(len);
        if let Some(last) = bytes.last_mut() {
            *last &= u8::MAX.checked_shl(unused).unwrap_or(0);
        }
        Ok(Bits { bytes: bytes.into_boxed_slice(), len })
    }

    /// The bits `digits` writes, each 0 or 1 [§5.3 `<bit>`]; none if one is
    /// another character.
    pub fn of_bits(digits: &str) -> Option<Bits> {
        let mut bytes = vec![0_u8; bytes_for(u32::try_from(digits.chars().count()).ok()?)];
        for (i, c) in digits.chars().enumerate() {
            let set = match c {
                '0' => false,
                '1' => true,
                _ => return None,
            };
            if set {
                let byte = bytes.get_mut(i.checked_div(8)?)?;
                *byte |= 0x80_u8.checked_shr(u32::try_from(i.checked_rem(8)?).ok()?)?;
            }
        }
        Bits::new(bytes, u32::try_from(digits.chars().count()).ok()?).ok()
    }

    /// The bits `hexits` writes, four to each [§5.3 SR11]; none if one is
    /// no `<hexit>`.
    pub fn of_hex(hexits: &str) -> Option<Bits> {
        let digits: String = hexits.chars().map(|c| c.to_digit(16).map(|d| format!("{d:04b}"))).collect::<Option<_>>()?;
        Bits::of_bits(&digits)
    }

    /// Its bytes, the bits past its length 0.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// How many bits it has.
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Whether it has no bits.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Bit `i`, the first 0.
    fn bit(&self, i: u32) -> bool {
        let at = i.checked_div(BYTE_BITS).and_then(|b| usize::try_from(b).ok());
        let byte = at.and_then(|b| self.bytes.get(b)).copied().unwrap_or(0);
        byte & i.checked_rem(BYTE_BITS).and_then(|r| 0x80_u8.checked_shr(r)).unwrap_or(0) != 0
    }

    /// Its bits as 0s and 1s.
    pub fn digits(&self) -> String {
        (0..self.len).map(|i| if self.bit(i) { '1' } else { '0' }).collect()
    }

    /// The bits `from` (the first 1) on, `count` of them at most, or all
    /// to its end: the bits SUBSTRING gives [§6.7 GR6].
    #[must_use]
    pub fn substring(&self, from: u32, count: u32) -> Bits {
        let start = from.saturating_sub(1).min(self.len);
        let end = start.saturating_add(count).min(self.len);
        let digits: String = (start..end).map(|i| if self.bit(i) { '1' } else { '0' }).collect();
        Bits::of_bits(&digits).unwrap_or_default()
    }

    /// Its bits, then `other`'s [§6.13 GR3].
    #[must_use]
    pub fn concat(&self, other: &Bits) -> Bits {
        Bits::of_bits(&format!("{}{}", self.digits(), other.digits())).unwrap_or_default()
    }

    /// Appends its key to `out`, as the module's documentation says.
    pub fn put_key(&self, out: &mut Vec<u8>) {
        let mut left = self.len;
        for (i, b) in self.bytes.iter().enumerate() {
            out.push(*b);
            let last = i.saturating_add(1) == self.bytes.len();
            out.push(if last { u8::try_from(left).unwrap_or(0) } else { FULL });
            left = left.saturating_sub(BYTE_BITS);
        }
        if self.bytes.is_empty() {
            out.extend_from_slice(&[0, 0]);
        }
    }

    /// The bit string whose key begins `rest`, which then holds the bytes
    /// after it; none if it begins with no key.
    pub fn take_key(rest: &mut &[u8]) -> Option<Bits> {
        let (mut bytes, mut len) = (Vec::new(), 0_u32);
        loop {
            let ([byte, marker], after) = rest.split_first_chunk::<2>().map(|(c, a)| (*c, a))?;
            *rest = after;
            match marker {
                FULL => {
                    bytes.push(byte);
                    len = len.checked_add(BYTE_BITS)?;
                }
                0..=8 => {
                    if marker != 0 {
                        bytes.push(byte);
                    }
                    return Bits::new(bytes, len.checked_add(u32::from(marker))?).ok();
                }
                _ => return None,
            }
        }
    }
}

/// The empty bit string.
impl Default for Bits {
    fn default() -> Bits {
        Bits { bytes: Box::new([]), len: 0 }
    }
}

/// [ISO/IEC 9075:1992 §8.2 GR4] Bit by bit: equal if of the same length and
/// bits; else less if a prefix of the other, or 0 where the other's first
/// different bit is 1.
impl Ord for Bits {
    fn cmp(&self, other: &Bits) -> Ordering {
        let common = self.len.min(other.len);
        (0..common).map(|i| self.bit(i).cmp(&other.bit(i))).find(|o| o.is_ne()).unwrap_or_else(|| self.len.cmp(&other.len))
    }
}

impl PartialOrd for Bits {
    fn partial_cmp(&self, other: &Bits) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Its bits as 0s and 1s.
impl fmt::Display for Bits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.digits())
    }
}

// Unit tests: a bit string's bytes, literals, operations and keys
// (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;

    /// Every bit string of up to `most` bits, the empty one first.
    fn every(most: u32) -> Vec<Bits> {
        let mut digits = vec![String::new()];
        for _ in 0..most {
            let longer: Vec<String> = digits
                .iter()
                .filter(|d| d.len() == digits.last().map_or(0, String::len))
                .flat_map(|d| [format!("{d}0"), format!("{d}1")])
                .collect();
            digits.extend(longer);
        }
        digits.iter().map(|d| Bits::of_bits(d).unwrap()).collect()
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.2 GR4] the keys of bit strings,
    /// compared as bytes, order every two of up to 9 bits as they compare,
    /// bit by bit, a string before every longer one it begins, which is
    /// the order of their 0s and 1s; no key begins another, so two
    /// columns' keys order as the pairs; each reads back the same.
    #[test]
    fn bit_keys_order_as_bit_strings_compare() {
        let all = every(9);
        assert_eq!(all.len(), 1023);
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
            assert_eq!(Bits::take_key(&mut rest).as_ref(), Some(a), "{a} reads back");
            assert!(rest.is_empty(), "{a}'s key is read whole");
            for (b, kb) in all.iter().zip(&keys) {
                assert_eq!(a.cmp(b), a.digits().cmp(&b.digits()), "{a} and {b} compare bit by bit");
                assert_eq!(ka.cmp(kb), a.cmp(b), "the keys of {a} and {b}");
                assert!(ka == kb || !kb.starts_with(ka), "{a}'s key begins {b}'s");
            }
        }
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.3 SR11, §6.7 GR6, §6.13 GR3] a bit
    /// string's bytes hold its bits, those past them 0; a literal's 0s and
    /// 1s or hexits make one, no other character; SUBSTRING takes bits
    /// from a position, as many as there are at most; || joins two.
    #[test]
    fn bits_are_made_cut_and_joined() {
        let b = Bits::new(vec![0xFF, 0xFF, 0xFF], 10).unwrap();
        assert_eq!((b.bytes(), b.len(), b.digits().as_str()), (&[0xFF, 0xC0][..], 10, "1111111111"), "the rest made 0");
        assert_eq!(Bits::new(vec![0xFF], 9).unwrap_err().code, Code::Api, "too few bytes");
        assert_eq!(Bits::of_hex("a5").map(|h| h.to_string()), Some("10100101".to_owned()));
        assert!(Bits::of_bits("012").is_none() && Bits::of_hex("G").is_none());
        let s = Bits::of_bits("110010").unwrap();
        assert_eq!(s.substring(2, 3).to_string(), "100");
        assert_eq!(s.substring(5, 9).to_string(), "10", "to its end");
        assert!(s.substring(9, 1).is_empty() && !s.is_empty() && Bits::default().is_empty());
        assert_eq!(s.concat(&Bits::of_bits("01").unwrap()).to_string(), "11001001");
    }

    /// Verifies: [DESIGN] a key that is cut short, or of a marker no key
    /// has, reads as none.
    #[test]
    fn malformed_bit_keys_are_none() {
        for bad in [&[][..], &[0x80][..], &[0x80, FULL][..], &[0x80, 10][..]] {
            assert!(Bits::take_key(&mut &bad[..]).is_none(), "{bad:?}");
        }
    }
}
