//! Tests of decoding a binary string by its encoding.

use super::super::tests::shape_of_bytes;
use super::super::{Syntax, Whitespace, parse_bytes};
use crate::error::SqlState;

/// `text` in UTF-16, big-endian if `big`, with no byte order mark.
fn utf16(text: &str, big: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    for unit in text.encode_utf16() {
        bytes.extend(if big { unit.to_be_bytes() } else { unit.to_le_bytes() });
    }
    bytes
}

/// `text` in UTF-16, big-endian if `big`, after its byte order mark.
fn marked(text: &str, big: bool) -> Vec<u8> {
    let mark: [u8; 2] = if big { [0xFE, 0xFF] } else { [0xFF, 0xFE] };
    [&mark[..], &utf16(text, big)].concat()
}

fn refused(bytes: &[u8]) -> String {
    let error = parse_bytes(bytes, Syntax::Document, Whitespace::Preserve).unwrap_err();
    assert_eq!(error.sqlstate(), SqlState::INVALID_XML_DOCUMENT, "{error}");
    error.to_string()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR6a] [XML §4.3.3] [XML
/// 1.0 Appendix F.1] [XML 1.1 Appendix E.1] UTF-8, with or without its
/// byte order mark, and UTF-16 with its mark, in either order, are read,
/// and declare no other encoding.
#[test]
fn utf_8_and_utf_16_are_read() {
    assert_eq!(shape_of_bytes("<a>é</a>".as_bytes()), "<a>é</>");
    assert_eq!(shape_of_bytes(&[&[0xEF, 0xBB, 0xBF][..], b"<?xml version='1.0' encoding='utf-8'?><a/>"].concat()), "<a></>");
    assert_eq!(shape_of_bytes(&marked("<a>\u{10000}</a>", true)), "<a>\u{10000}</>");
    assert_eq!(shape_of_bytes(&marked("<?xml version='1.0' encoding='UTF-16'?><a/>", false)), "<a></>");
    assert!(
        refused(&[&[0xEF, 0xBB, 0xBF][..], b"<?xml version='1.0' encoding='ISO-8859-1'?><a/>"].concat()).contains("declares the encoding")
    );
    assert!(refused(&marked("<?xml version='1.0' encoding='UTF-8'?><a/>", true)).contains("declares the encoding"));
    assert!(refused(&[b'<', b'a', b'>', 0xFF, b'<', b'/', b'a', b'>']).contains("not UTF-8 at octet 4"));
    assert!(refused(&[0xFE, 0xFF, 0, b'<', 0]).contains("odd number"));
    assert!(refused(&[0xFE, 0xFF, 0xD8, 0, 0, b'a']).contains("lone surrogate"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR6a] [XML §4.3.3] [XML
/// 1.0 Appendix F.1] [XML 1.1 Appendix E.1] UTF-16BE and UTF-16LE, with
/// no mark, declare
/// themselves; ISO-8859-1 and US-ASCII are read as they declare; another
/// encoding, UCS-4 among them, is refused.
#[test]
fn declared_encodings() {
    assert_eq!(shape_of_bytes(&utf16("<?xml version='1.0' encoding='UTF-16BE'?><a/>", true)), "<a></>");
    assert_eq!(shape_of_bytes(&utf16("<?xml version='1.0' encoding='utf-16le'?><a/>", false)), "<a></>");
    assert!(refused(&utf16("<?xml version='1.0'?><a/>", true)).contains("declares UTF-16BE"));
    assert_eq!(shape_of_bytes(&[&b"<?xml version='1.0' encoding='ISO-8859-1'?><a>"[..], &[0xE9], b"</a>"].concat()), "<a>é</>");
    assert_eq!(shape_of_bytes(b"<?xml version='1.0' encoding='US-ASCII'?><a/>"), "<a></>");
    assert!(refused(&[&b"<?xml version='1.0' encoding='US-ASCII'?><a>"[..], &[0xE9], b"</a>"].concat()).contains("not US-ASCII"));
    assert!(refused(b"<?xml version='1.0' encoding='Shift_JIS'?><a/>").contains("'Shift_JIS', which turborel does not read"));
    assert!(refused(&[0, 0, 0xFE, 0xFF, 0, 0, 0, b'<']).contains("UCS-4"));
}
