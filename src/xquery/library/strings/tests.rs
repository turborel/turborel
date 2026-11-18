//! Tests of the functions on strings.

use super::super::testing::{Focus, atom, call, call_in, code, collator, dbl, empty, focus, int, s, seq, text, texts, typed};
use crate::xml::{Atomic, AtomicType, Item};

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.2.1.1,
/// §7.2.2.1] the examples of the code point functions, a character past
/// the BMP one code point; a code point no character of XML err:FOCH0001.
#[test]
fn code_points() {
    let devanagari = seq(&[int(2309), int(2358), int(2378), int(2325)]);
    assert_eq!(text(call("codepoints-to-string", &[devanagari])), "अशॊक");
    let points = call("string-to-codepoints", &[s("Thérèse")]).unwrap();
    assert_eq!(texts(&points), vec!["84", "104", "233", "114", "232", "115", "101"]);
    assert_eq!(texts(&call("string-to-codepoints", &[s("😀")]).unwrap()), vec!["128512"]);
    assert!(call("string-to-codepoints", &[s("")]).unwrap().items().is_empty());
    assert!(call("string-to-codepoints", &[empty()]).unwrap().items().is_empty());
    assert_eq!(text(call("codepoints-to-string", &[empty()])), "");
    assert_eq!(text(call("codepoints-to-string", &[int(1)])), "\u{1}");
    for bad in [0, 0xD800, 0xFFFE, 0x0011_0000, -1] {
        assert_eq!(code(call("codepoints-to-string", &[int(bad)])), "FOCH0001", "{bad}");
    }
    assert_eq!(code(call("codepoints-to-string", &[s("65")])), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.3.2.1,
/// §7.3.3] `fn:compare` by the code point collation, the default, or a
/// collation named; the empty sequence of the empty sequence; an unknown
/// collation err:FOCH0002; `fn:codepoint-equal`.
#[test]
fn comparing_strings() {
    assert_eq!(text(call("compare", &[s("abc"), s("abc")])), "0");
    assert_eq!(text(call("compare", &[s("a"), s("b")])), "-1");
    assert_eq!(text(call("compare", &[s("b"), s("B")])), "1");
    assert_eq!(text(call("compare", &[s("abc"), s("abc ")])), "-1");
    assert_eq!(text(call("compare", &[s("Bob"), s("bob"), s("urn:turborel:collation:CASE_INSENSITIVE")])), "0");
    assert_eq!(text(call("compare", &[s("a"), s("B"), s("urn:turborel:collation:UNICODE")])), "-1");
    let insensitive = Focus { collation: collator("CASE_INSENSITIVE"), ..focus() };
    assert_eq!(text(call_in(&insensitive, "compare", &[s("X"), s("x")])), "0");
    assert!(call("compare", &[empty(), s("a")]).unwrap().items().is_empty());
    assert_eq!(code(call("compare", &[s("a"), s("b"), s("deutsch")])), "FOCH0002");
    assert_eq!(typed(call("codepoint-equal", &[s("abc"), s("abc")])), (AtomicType::Boolean, "true".to_owned()));
    assert_eq!(text(call("codepoint-equal", &[s("abc"), s("ABC")])), "false");
    assert!(call("codepoint-equal", &[s("a"), empty()]).unwrap().items().is_empty());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.4.1.1,
/// §7.4.2.1] the examples of `fn:concat` and `fn:string-join`, an atomic
/// value of any type cast to a string, the empty sequence a zero-length
/// one.
#[test]
fn concat_and_string_join() {
    assert_eq!(text(call("concat", &[s("un"), s("grateful")])), "ungrateful");
    let parts = [s("Thy "), empty(), s("old "), s("groans"), s(""), s(" ring"), s(" yet"), s(" in"), s(" my"), s(" ancient"), s(" ears.")];
    assert_eq!(text(call("concat", &parts)), "Thy old groans ring yet in my ancient ears.");
    assert_eq!(text(call("concat", &[s("Ciao!"), empty()])), "Ciao!");
    assert_eq!(text(call("concat", &[int(1), dbl("1.5E6"), atom(AtomicType::Boolean, "true")])), "11.5E6true");
    assert_eq!(code(call("concat", &[seq(&[int(1), int(2)]), s("")])), "XPTY0004");
    let words = seq(&[s("Now"), s("is"), s("the"), s("time"), s("...")]);
    assert_eq!(text(call("string-join", &[words, s(" ")])), "Now is the time ...");
    let blow = seq(&[s("Blow, "), s("blow, "), s("thou "), s("winter "), s("wind!")]);
    assert_eq!(text(call("string-join", &[blow, s("")])), "Blow, blow, thou winter wind!");
    assert_eq!(text(call("string-join", &[empty(), s("separator")])), "");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.4.3.1]
/// the examples of `fn:substring`, positions rounded, NaN and infinities
/// compared as doubles.
#[test]
fn substrings_by_position() {
    let cases = [
        (s("motor car"), dbl("6"), None, " car"),
        (s("metadata"), dbl("4"), Some(dbl("3")), "ada"),
        (s("12345"), dbl("1.5"), Some(dbl("2.6")), "234"),
        (s("12345"), dbl("0"), Some(dbl("3")), "12"),
        (s("12345"), dbl("5"), Some(dbl("-3")), ""),
        (s("12345"), dbl("-3"), Some(dbl("5")), "1"),
        (s("12345"), dbl("NaN"), Some(dbl("3")), ""),
        (s("12345"), dbl("1"), Some(dbl("NaN")), ""),
        (empty(), dbl("1"), Some(dbl("3")), ""),
        (s("12345"), dbl("-42"), Some(dbl("INF")), "12345"),
        (s("12345"), dbl("-INF"), Some(dbl("INF")), ""),
        (s("äöü"), int(2), None, "öü"),
    ];
    for (source, start, length, expected) in cases {
        let mut args = vec![source, start];
        args.extend(length);
        assert_eq!(text(call("substring", &args)), expected, "{args:?}");
    }
    assert_eq!(code(call("substring", &[s("a"), s("1")])), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.4.4.1,
/// §7.4.5.1] the examples of `fn:string-length` and `fn:normalize-space`,
/// in characters; of the context item when no argument is given,
/// err:FONC0001 if there is none.
#[test]
fn length_and_space() {
    assert_eq!(text(call("string-length", &[s("Harp not on that string, madam; that is past.")])), "45");
    assert_eq!(text(call("string-length", &[empty()])), "0");
    assert_eq!(text(call("string-length", &[s("äöü😀")])), "4");
    let at = focus().at(Item::Atomic(Atomic::new(AtomicType::Integer, "12345")), 1, 1);
    assert_eq!(text(call_in(&at, "string-length", &[])), "5");
    assert_eq!(code(call("string-length", &[])), "FONC0001");
    let spaced = s(" The    wealthy curled darlings\n\tof   our nation. ");
    assert_eq!(text(call("normalize-space", &[spaced])), "The wealthy curled darlings of our nation.");
    assert_eq!(text(call("normalize-space", &[empty()])), "");
    let blank = focus().at(Item::Atomic(Atomic::new(AtomicType::String, "  a  b ")), 1, 1);
    assert_eq!(text(call_in(&blank, "normalize-space", &[])), "a b");
    assert_eq!(code(call("normalize-space", &[])), "FONC0001");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.4.9.1]
/// the examples of `fn:translate`: a character mapped, or dropped where
/// the translation is shorter, the first of two mappings used.
#[test]
fn translate_examples() {
    assert_eq!(text(call("translate", &[s("bar"), s("abc"), s("ABC")])), "BAr");
    assert_eq!(text(call("translate", &[s("--aaa--"), s("abc-"), s("ABC")])), "AAA");
    assert_eq!(text(call("translate", &[s("abcdabc"), s("abc"), s("AB")])), "ABdAB");
    assert_eq!(text(call("translate", &[s("aa"), s("aa"), s("xy")])), "xx");
    assert_eq!(text(call("translate", &[s("abc"), s(""), s("xyz")])), "abc");
    assert_eq!(text(call("translate", &[empty(), s("a"), s("b")])), "");
}
