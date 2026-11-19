//! Tests of resolving URIs.

use super::super::testing::{Focus, call, call_in, code, empty, focus, int, s, typed};
use super::*;

/// The base of [RFC 3986 §5.4]'s examples.
const BASE: &str = "http://a/b/c/d;p?q";

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §8.1] [RFC
/// 3986 §5.4.1, §5.4.2] the normal and abnormal examples of resolving a
/// reference against a base.
#[test]
fn rfc_3986_examples() {
    let cases = [
        ("g:h", "g:h"),
        ("g", "http://a/b/c/g"),
        ("./g", "http://a/b/c/g"),
        ("g/", "http://a/b/c/g/"),
        ("/g", "http://a/g"),
        ("//g", "http://g"),
        ("?y", "http://a/b/c/d;p?y"),
        ("g?y", "http://a/b/c/g?y"),
        ("#s", "http://a/b/c/d;p?q#s"),
        ("g#s", "http://a/b/c/g#s"),
        ("g?y#s", "http://a/b/c/g?y#s"),
        (";x", "http://a/b/c/;x"),
        ("g;x?y#s", "http://a/b/c/g;x?y#s"),
        ("", "http://a/b/c/d;p?q"),
        (".", "http://a/b/c/"),
        ("./", "http://a/b/c/"),
        ("..", "http://a/b/"),
        ("../g", "http://a/b/g"),
        ("../..", "http://a/"),
        ("../../g", "http://a/g"),
        ("../../../g", "http://a/g"),
        ("../../../../g", "http://a/g"),
        ("/./g", "http://a/g"),
        ("/../g", "http://a/g"),
        ("g.", "http://a/b/c/g."),
        ("..g", "http://a/b/c/..g"),
        ("./../g", "http://a/b/g"),
        ("./g/.", "http://a/b/c/g/"),
        ("g/./h", "http://a/b/c/g/h"),
        ("g/../h", "http://a/b/c/h"),
    ];
    for (relative, expected) in cases {
        assert_eq!(resolve(relative, BASE).as_deref(), Some(expected), "{relative}");
    }
    assert_eq!(resolve("g", "http://a").as_deref(), Some("http://a/g"));
    assert_eq!(resolve("g", "relative/base"), None);
    assert_eq!(remove_dot_segments("mid/content=5/../6"), "mid/6");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §8.1]
/// `fn:resolve-uri`: against the base given, or the static context's; an
/// absolute reference unchanged; the empty sequence of the empty sequence;
/// no static base URI err:FONS0005; a relative base err:FORG0009.
#[test]
fn resolve_uri_function() {
    assert_eq!(typed(call("resolve-uri", &[s("g"), s(BASE)])), (AtomicType::AnyUri, "http://a/b/c/g".to_owned()));
    assert_eq!(typed(call("resolve-uri", &[s("a:./b/../c"), s(BASE)])).1, "a:./b/../c");
    assert_eq!(typed(call("resolve-uri", &[s(""), s(BASE)])).1, BASE);
    assert!(call("resolve-uri", &[empty(), s(BASE)]).unwrap().items().is_empty());
    assert_eq!(code(call("resolve-uri", &[s("g")])), "FONS0005");
    let based = Focus { base_uri: Some("http://e.org/dir/f".to_owned()), ..focus() };
    assert_eq!(typed(call_in(&based, "resolve-uri", &[s("g")])).1, "http://e.org/dir/g");
    assert_eq!(code(call("resolve-uri", &[s("g"), s("not/absolute")])), "FORG0009");
    assert_eq!(code(call("resolve-uri", &[int(1), s(BASE)])), "XPTY0004");
}
