//! Tests of escaping strings for URIs.

use super::super::testing::{call, empty, s, text};

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.4.10.1]
/// the examples of `fn:encode-for-uri`: `%` and the delimiters escaped,
/// `#` and the marks kept, each octet of UTF-8 in upper case hexadecimal.
#[test]
fn encode_for_uri_examples() {
    let weather = "http://www.example.com/00/Weather/CA/Los%20Angeles#ocean";
    assert_eq!(text(call("encode-for-uri", &[s(weather)])), "http%3A%2F%2Fwww.example.com%2F00%2FWeather%2FCA%2FLos%2520Angeles#ocean");
    assert_eq!(text(call("encode-for-uri", &[s("~bébé")])), "~b%C3%A9b%C3%A9");
    assert_eq!(text(call("encode-for-uri", &[s("100% organic")])), "100%25%20organic");
    assert_eq!(text(call("encode-for-uri", &[s("-_.!~*'()")])), "-_.!~*'()");
    assert_eq!(text(call("encode-for-uri", &[empty()])), "");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.4.11.1]
/// the examples of `fn:iri-to-uri`, the delimiters and `%` kept.
#[test]
fn iri_to_uri_examples() {
    let weather = "http://www.example.com/00/Weather/CA/Los%20Angeles#ocean";
    assert_eq!(text(call("iri-to-uri", &[s(weather)])), weather);
    assert_eq!(text(call("iri-to-uri", &[s("http://www.example.com/~bébé")])), "http://www.example.com/~b%C3%A9b%C3%A9");
    assert_eq!(text(call("iri-to-uri", &[s("a b;/?:@&=+$,[]%")])), "a%20b;/?:@&=+$,[]%");
    assert_eq!(text(call("iri-to-uri", &[s("{😀}")])), "%7B%F0%9F%98%80%7D");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.4.12.1]
/// the examples of `fn:escape-html-uri`: printable ASCII kept, a space
/// too, the rest escaped.
#[test]
fn escape_html_uri_examples() {
    let spaced = "http://www.example.com/00/Weather/CA/Los Angeles#ocean";
    assert_eq!(text(call("escape-html-uri", &[s(spaced)])), spaced);
    let script = "javascript:if (navigator.browserLanguage == 'fr') window.open('http://www.example.com/~bébé');";
    let escaped = "javascript:if (navigator.browserLanguage == 'fr') window.open('http://www.example.com/~b%C3%A9b%C3%A9');";
    assert_eq!(text(call("escape-html-uri", &[s(script)])), escaped);
    assert_eq!(text(call("escape-html-uri", &[s("\t\u{7F}")])), "%09%7F");
}
