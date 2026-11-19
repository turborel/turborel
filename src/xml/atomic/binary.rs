//! Binary values [Schema2 §3.2.15, §3.2.16]: `xs:hexBinary` and
//! `xs:base64Binary`, octets in hex or in base64 [RFC 2045 §6.8].
//!
//! \[DESIGN\] RULES.md D106: [Schema2 §3.2.16] defines base64's literals as
//! RFC 2045's encoding, whose lines a collapsed literal has as spaces. So
//! a literal of `xs:base64Binary` is groups of four characters of the
//! alphabet, spaces among them ignored, the last group padded with `=`;
//! another character is refused, not ignored as RFC 2045 lets a decoder.
//! Its string is the same without spaces, as [Schema2] defines no
//! canonical representation [XQueryFO §17.1.2].

use std::fmt::Write;

/// [RFC 2045 §6.8] The 64 characters of base64, each standing for six
/// bits, in order.
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/// [RFC 2045 §6.8] The character that pads base64 to a whole group.
const PAD: char = '=';
/// [RFC 2045 §6.8] The octets of a group, which four characters write.
const GROUP: usize = 3;
/// The characters of a group.
const QUAD: usize = 4;
/// The bits each character of base64 stands for.
const SIX_BITS: u32 = 6;
/// The low six bits.
const SIX_MASK: u32 = 0x3F;

/// The octets `text` encodes, if it is a lexical form of `xs:hexBinary`:
/// two hexadecimal digits each [Schema2 §3.2.15.1].
#[must_use]
pub fn hex(text: &str) -> Option<Vec<u8>> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    digits
        .chunks(2)
        .map(|pair| {
            std::str::from_utf8(pair).ok().filter(|p| p.bytes().all(|b| b.is_ascii_hexdigit())).and_then(|p| u8::from_str_radix(p, 16).ok())
        })
        .collect()
}

/// `octets` as the canonical lexical form of `xs:hexBinary`: two upper case
/// hexadecimal digits each [Schema2 §3.2.15.2].
#[must_use]
pub fn hex_text(octets: &[u8]) -> String {
    octets.iter().fold(String::with_capacity(octets.len().saturating_mul(2)), |mut out, o| {
        let _ = write!(out, "{o:02X}");
        out
    })
}

/// `octets` in base64, of no line breaks: each three octets four
/// characters of six bits, a last group of fewer padded with `=` [RFC 2045
/// §6.8].
#[must_use]
pub fn base64_text(octets: &[u8]) -> String {
    let mut out = String::new();
    for group in octets.chunks(GROUP) {
        let mut word = [0_u8; 4];
        for (w, o) in word.iter_mut().skip(1).zip(group) {
            *w = *o;
        }
        let word = u32::from_be_bytes(word);
        for k in 0..=GROUP {
            if k > group.len() {
                out.push(PAD);
                continue;
            }
            let shift = u32::try_from(GROUP.saturating_sub(k)).unwrap_or(0).saturating_mul(SIX_BITS);
            let six = word.checked_shr(shift).unwrap_or(0) & SIX_MASK;
            out.push(char::from(BASE64.get(usize::try_from(six).unwrap_or(0)).copied().unwrap_or(b'A')));
        }
    }
    out
}

/// The octets `text` encodes, if it is a lexical form of `xs:base64Binary`
/// as above.
#[must_use]
pub fn base64(text: &str) -> Option<Vec<u8>> {
    let chars: Vec<u8> = text.bytes().filter(|&b| b != b' ').collect();
    if !chars.len().is_multiple_of(QUAD) {
        return None;
    }
    let mut out = Vec::with_capacity(chars.len().checked_div(QUAD).unwrap_or(0).saturating_mul(GROUP));
    let last = chars.len().checked_div(QUAD).unwrap_or(0).saturating_sub(1);
    for (n, quad) in chars.chunks(QUAD).enumerate() {
        let pads = quad.iter().rev().take_while(|&&b| char::from(b) == PAD).count();
        if pads > 2 || (pads > 0 && n != last) {
            return None;
        }
        let mut word = 0_u32;
        for &b in quad.iter().take(QUAD.saturating_sub(pads)) {
            let six = BASE64.iter().position(|&c| c == b).and_then(|p| u32::try_from(p).ok())?;
            word = word.checked_shl(SIX_BITS)? | six;
        }
        let kept = GROUP.saturating_sub(pads);
        let word = word.checked_shl(SIX_BITS.saturating_mul(u32::try_from(pads).ok()?))?;
        let bytes = word.to_be_bytes();
        out.extend(bytes.iter().skip(1).take(kept));
    }
    Some(out)
}
