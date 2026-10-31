//! \[DESIGN\] Base 64 [RFC 4648 §4], as SCRAM writes its salt, proofs and
//! signatures [RFC 5802 §7]: the standard alphabet, padded with `=`; a
//! text of any other character, or padded otherwise, is refused.

/// \[DESIGN\] The alphabet of RFC 4648, Table 1: the value of each
/// character is its index.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/// \[DESIGN\] The padding character [RFC 4648 §4].
const PAD: u8 = b'=';
/// The bits of a character of the encoding.
const BITS: u32 = 6;

/// The character of the six bits `value` holds at `shift`.
fn character(value: u32, shift: u32) -> char {
    let index = usize::try_from(value.wrapping_shr(shift) & 0x3f).unwrap_or(0);
    ALPHABET.get(index).map_or('A', |c| char::from(*c))
}

/// `bytes`, encoded.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0_u32, |v, (i, b)| {
            let shift = 16_u32.wrapping_sub(u32::try_from(i).unwrap_or(0).wrapping_mul(8));
            v | u32::from(*b).wrapping_shl(shift)
        });
        // A chunk of n bytes makes n + 1 characters, padded to four.
        for (i, shift) in [18_u32, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= chunk.len() { character(value, shift) } else { char::from(PAD) });
        }
    }
    out
}

/// The value of the character `c`, if it is of the alphabet.
fn value(c: u8) -> Option<u32> {
    ALPHABET.iter().position(|a| *a == c).and_then(|i| u32::try_from(i).ok())
}

/// The bytes `text` encodes; none if it is not of the encoding.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let (quads, rest) = text.as_bytes().as_chunks::<4>();
    if !rest.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    let last = quads.len().checked_sub(1);
    for (n, quad) in quads.iter().enumerate() {
        let pads = quad.iter().rev().take_while(|c| **c == PAD).count();
        if pads > 2 || (pads > 0 && Some(n) != last) {
            return None;
        }
        let digits = quad.get(..4_usize.wrapping_sub(pads))?;
        let value = digits.iter().try_fold(0_u32, |v, c| Some(v.wrapping_shl(BITS) | value(*c)?))?;
        // Each pad leaves two bits of the last character spare: zero in a
        // canonical encoding [RFC 4648 §3.5], so one text alone is a
        // signature's.
        let spare = u32::try_from(pads).ok()?;
        if value & 1_u32.wrapping_shl(spare.wrapping_mul(2)).wrapping_sub(1) != 0 {
            return None;
        }
        let value = value.wrapping_shl(BITS.wrapping_mul(spare));
        let bytes = value.to_be_bytes();
        out.extend(bytes.get(1..4_usize.wrapping_sub(pads))?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RFC 4648 §10: the test vectors of base 64, each
    /// encoded and decoded back.
    #[test]
    fn the_rfc_vectors_encode_and_decode() {
        for (bytes, text) in
            [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")]
        {
            assert_eq!(encode(bytes.as_bytes()), text);
            assert_eq!(decode(text), Some(bytes.as_bytes().to_vec()), "{text}");
        }
        assert_eq!(decode(&encode(&[0xff, 0xfe, 0x00, 0x3e, 0x3f])), Some(vec![0xff, 0xfe, 0x00, 0x3e, 0x3f]));
    }

    /// Verifies: [DESIGN] RULES.md R-ERR-1: a text not of base 64 is
    /// refused, never a panic: of a length not a multiple of four, a
    /// character not of the alphabet, padding of three, or padding before
    /// the end, or spare bits not zero [RFC 4648 §3.5].
    #[test]
    fn a_malformed_text_is_refused() {
        for text in ["Zg=", "Zm9", "Zm,v", "Z===", "Zg==Zm9v", "Zm9v Zg==", "Zh==", "Zm9="] {
            assert_eq!(decode(text), None, "{text}");
        }
    }
}
