//! A binary string decoded to the characters it encodes [XML §4.3.3]: its
//! encoding told by its byte order mark or its first octets [XML 1.0
//! Appendix F.1] [XML 1.1 Appendix E.1], then by the encoding its
//! declaration names, matched
//! case-insensitively.
//!
//! turborel reads UTF-8 and UTF-16, as every processor does [XML §4.3.3],
//! and UTF-16BE, UTF-16LE, ISO-8859-1 and US-ASCII; another encoding is
//! one it is unable to process, a fatal error [XML §4.3.3]. A text in
//! UTF-16 begins with a byte order mark; one in UTF-16BE or UTF-16LE has
//! none, and declares its encoding.

use crate::xml::chars::is_space;

/// The encodings read, by their IANA names.
const UTF_8: &str = "UTF-8";
/// UTF-16, with a byte order mark.
const UTF_16: &str = "UTF-16";
/// UTF-16, big-endian, with none.
const UTF_16BE: &str = "UTF-16BE";
/// UTF-16, little-endian, with none.
const UTF_16LE: &str = "UTF-16LE";
/// ISO 8859 part 1.
const ISO_8859_1: &str = "ISO-8859-1";
/// ASCII.
const US_ASCII: &str = "US-ASCII";

// [XML 1.0 Appendix F.1] [XML 1.1 Appendix E.1] The octets that tell an
// encoding.
/// UTF-8's byte order mark.
const UTF_8_MARK: [u8; 3] = [0xEF, 0xBB, 0xBF];
/// UTF-16's, big-endian.
const BIG_MARK: [u8; 2] = [0xFE, 0xFF];
/// UTF-16's, little-endian.
const LITTLE_MARK: [u8; 2] = [0xFF, 0xFE];
/// UCS-4's marks, in its four orders, which turborel does not read.
const UCS_4_MARKS: [[u8; 4]; 4] = [[0, 0, 0xFE, 0xFF], [0xFF, 0xFE, 0, 0], [0, 0, 0xFF, 0xFE], [0xFE, 0xFF, 0, 0]];
/// `<?` in a big-endian 16-bit encoding.
const BIG_OPEN: [u8; 4] = [0, 0x3C, 0, 0x3F];
/// `<?` in a little-endian one.
const LITTLE_OPEN: [u8; 4] = [0x3C, 0, 0x3F, 0];
/// The octets that begin a declaration in an encoding of ASCII's.
const ASCII_OPEN: &[u8] = b"<?xml";
/// The octets that end it.
const ASCII_CLOSE: &[u8] = b"?>";
/// The last octet of ASCII.
const ASCII_LAST: u8 = 0x7F;

/// The characters `bytes` encode.
///
/// # Errors
///
/// Why they encode none: an encoding not read, a declaration of another
/// encoding than theirs, or octets not of their encoding [XML §4.3.3].
pub(super) fn decode(bytes: &[u8]) -> Result<String, String> {
    if UCS_4_MARKS.iter().any(|mark| bytes.starts_with(mark)) {
        return Err(unread("UCS-4"));
    }
    if let Some(rest) = bytes.strip_prefix(&UTF_8_MARK) {
        return labeled(utf8(rest)?, UTF_8);
    }
    if let Some(rest) = bytes.strip_prefix(&BIG_MARK) {
        return labeled(utf16(rest, true)?, UTF_16);
    }
    if let Some(rest) = bytes.strip_prefix(&LITTLE_MARK) {
        return labeled(utf16(rest, false)?, UTF_16);
    }
    if bytes.starts_with(&BIG_OPEN) {
        return declared(utf16(bytes, true)?, UTF_16BE);
    }
    if bytes.starts_with(&LITTLE_OPEN) {
        return declared(utf16(bytes, false)?, UTF_16LE);
    }
    let head = bytes
        .starts_with(ASCII_OPEN)
        .then(|| bytes.windows(ASCII_CLOSE.len()).position(|w| w == ASCII_CLOSE))
        .flatten()
        .and_then(|end| bytes.get(..end))
        .map(|head| head.iter().map(|&b| char::from(b)).collect::<String>())
        .unwrap_or_default();
    let Some(name) = encoding_of(&head) else {
        return utf8(bytes);
    };
    match name.to_ascii_uppercase().as_str() {
        UTF_8 => utf8(bytes),
        ISO_8859_1 => Ok(bytes.iter().map(|&b| char::from(b)).collect()),
        US_ASCII => ascii(bytes),
        _ => Err(unread(name)),
    }
}

/// The error of an encoding turborel does not read.
fn unread(name: &str) -> String {
    format!("the binary string is in the encoding '{name}', which turborel does not read [XML §4.3.3]")
}

/// The encoding the declaration that begins `text` names, if it names
/// one [XML §4.3.3 \[80\]].
fn encoding_of(text: &str) -> Option<&str> {
    let declaration = text.strip_prefix("<?xml").filter(|d| d.starts_with(is_space))?;
    let declaration = declaration.split_once("?>").map_or(declaration, |(d, _)| d);
    let value = declaration.split_once("encoding")?.1.trim_start_matches(is_space).strip_prefix('=')?.trim_start_matches(is_space);
    let quote = value.chars().next().filter(|&q| q == '"' || q == '\'')?;
    value.get(1..)?.split_once(quote).map(|(name, _)| name)
}

/// `text`, read in `encoding` after its byte order mark, unless it
/// declares another [XML §4.3.3].
fn labeled(text: String, encoding: &str) -> Result<String, String> {
    if let Some(name) = encoding_of(&text).filter(|n| !n.eq_ignore_ascii_case(encoding)) {
        return Err(format!("the binary string is in {encoding} and declares the encoding '{name}' [XML §4.3.3]"));
    }
    Ok(text)
}

/// `text`, read in `encoding` without a byte order mark, if it declares
/// it [XML §4.3.3].
fn declared(text: String, encoding: &str) -> Result<String, String> {
    if encoding_of(&text).is_some_and(|n| n.eq_ignore_ascii_case(encoding)) {
        Ok(text)
    } else {
        Err(format!("the binary string is in a 16-bit encoding with no byte order mark, so it declares {encoding} [XML §4.3.3]"))
    }
}

/// The characters `bytes` encode in UTF-8.
fn utf8(bytes: &[u8]) -> Result<String, String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|e| format!("the binary string is not UTF-8 at octet {} [XML §4.3.3]", e.valid_up_to().saturating_add(1)))
}

/// The characters `bytes` encode in UTF-16, big-endian if `big`.
fn utf16(bytes: &[u8], big: bool) -> Result<String, String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("the binary string is not UTF-16: it has an odd number of octets [XML §4.3.3]".to_owned());
    }
    let units = bytes.as_chunks::<2>().0.iter().map(|&two| if big { u16::from_be_bytes(two) } else { u16::from_le_bytes(two) });
    char::decode_utf16(units)
        .collect::<Result<String, _>>()
        .map_err(|e| format!("the binary string is not UTF-16: it has the lone surrogate #x{:X} [XML §4.3.3]", e.unpaired_surrogate()))
}

/// The characters `bytes` encode in US-ASCII.
fn ascii(bytes: &[u8]) -> Result<String, String> {
    match bytes.iter().position(|&b| b > ASCII_LAST) {
        Some(at) => Err(format!("the binary string is not US-ASCII at octet {} [XML §4.3.3]", at.saturating_add(1))),
        None => Ok(bytes.iter().map(|&b| char::from(b)).collect()),
    }
}

#[cfg(test)]
mod tests;
