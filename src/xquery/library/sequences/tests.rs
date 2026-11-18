//! Tests of the general functions and operators on sequences.

use super::super::testing::{atom, call, code, dbl, dec, empty, int, s, seq, text, texts};
use super::*;
use crate::xml::AtomicType;

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.1.3.1]
/// the examples of `fn:index-of`, values not comparable distinct, and by
/// a collation.
#[test]
fn index_of_examples() {
    let tens = seq(&[int(10), int(20), int(30), int(40)]);
    assert!(call("index-of", &[tens, int(35)]).unwrap().items().is_empty());
    let repeated = seq(&[int(10), int(20), int(30), int(30), int(20), int(10)]);
    assert_eq!(texts(&call("index-of", &[repeated, int(20)]).unwrap()), ["2", "5"]);
    let words = seq(&[s("a"), s("sport"), s("and"), s("a"), s("pastime")]);
    assert_eq!(texts(&call("index-of", &[words, s("a")]).unwrap()), ["1", "4"]);
    assert_eq!(call("index-of", &[seq(&[s("1"), int(1)]), dbl("1")]).unwrap().items().len(), 1);
    assert_eq!(
        texts(&call("index-of", &[seq(&[s("A"), s("a")]), s("a"), s("urn:turborel:collation:CASE_INSENSITIVE")]).unwrap()),
        ["1", "2"]
    );
    assert_eq!(code(call("index-of", &[int(1), empty()])), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.1.4.1,
/// §15.1.5.1, §15.4.1.1] the examples of `fn:empty`, `fn:exists` and
/// `fn:count`.
#[test]
fn empty_exists_count() {
    let removed = call("remove", &[seq(&[s("hello"), s("world")]), int(1)]).unwrap();
    assert_eq!(text(call("empty", &[removed])), "false");
    assert_eq!(text(call("exists", &[call("remove", &[s("hello"), int(1)]).unwrap()])), "false");
    assert_eq!(text(call("count", &[seq(&[dec("98.5"), dec("98.3"), dec("98.9")])])), "3");
    assert_eq!(text(call("count", &[empty()])), "0");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.1.6.1]
/// `fn:distinct-values`: the first of equal values kept, of different
/// numeric types, NaN once, untyped values as strings.
#[test]
fn distinct_values_examples() {
    assert_eq!(texts(&call("distinct-values", &[seq(&[int(1), dec("2.0"), int(3), int(2)])]).unwrap()), ["1", "2.0", "3"]);
    let untyped =
        seq(&[atom(AtomicType::UntypedAtomic, "cherry"), atom(AtomicType::UntypedAtomic, "bar"), atom(AtomicType::UntypedAtomic, "bar")]);
    assert_eq!(texts(&call("distinct-values", &[untyped]).unwrap()), ["cherry", "bar"]);
    assert_eq!(texts(&call("distinct-values", &[seq(&[dbl("NaN"), dbl("NaN"), dbl("0"), dbl("-0")])]).unwrap()), ["NaN", "0"]);
    assert_eq!(texts(&call("distinct-values", &[seq(&[s("1"), int(1)])]).unwrap()), ["1", "1"]);
    assert_eq!(texts(&call("distinct-values", &[seq(&[s("A"), s("a")]), s("urn:turborel:collation:CASE_INSENSITIVE")]).unwrap()), ["A"]);
    assert!(call("distinct-values", &[empty()]).unwrap().items().is_empty());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.1.7.1
/// to §15.1.9.1] the examples of `fn:insert-before`, `fn:remove` and
/// `fn:reverse`.
#[test]
fn insert_remove_reverse() {
    let x = || seq(&[s("a"), s("b"), s("c")]);
    for (at, expected) in [(0, "zabc"), (1, "zabc"), (2, "azbc"), (3, "abzc"), (4, "abcz"), (9, "abcz")] {
        assert_eq!(texts(&call("insert-before", &[x(), int(at), s("z")]).unwrap()).concat(), expected, "{at}");
    }
    assert_eq!(texts(&call("insert-before", &[empty(), int(3), s("z")]).unwrap()), ["z"]);
    for (at, expected) in [(0, "abc"), (1, "bc"), (6, "abc"), (3, "ab")] {
        assert_eq!(texts(&call("remove", &[x(), int(at)]).unwrap()).concat(), expected, "{at}");
    }
    assert!(call("remove", &[empty(), int(3)]).unwrap().items().is_empty());
    assert_eq!(texts(&call("reverse", &[x()]).unwrap()), ["c", "b", "a"]);
    assert_eq!(texts(&call("reverse", &[s("hello")]).unwrap()), ["hello"]);
    assert!(call("reverse", &[empty()]).unwrap().items().is_empty());
    assert_eq!(code(call("remove", &[x(), dec("1")])), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.1.10.1,
/// §15.1.11] the examples of `fn:subsequence`, positions rounded, and
/// `fn:unordered`.
#[test]
fn subsequences() {
    let items = || seq(&[s("1"), s("2"), s("3"), s("4"), s("5")]);
    assert_eq!(texts(&call("subsequence", &[items(), dbl("4")]).unwrap()), ["4", "5"]);
    assert_eq!(texts(&call("subsequence", &[items(), dbl("3"), dbl("2")]).unwrap()), ["3", "4"]);
    assert_eq!(texts(&call("subsequence", &[items(), dbl("1.5"), dbl("2.6")]).unwrap()), ["2", "3", "4"]);
    assert_eq!(texts(&call("subsequence", &[items(), dbl("-1"), dbl("3")]).unwrap()), ["1"]);
    assert!(call("subsequence", &[items(), dbl("NaN")]).unwrap().items().is_empty());
    assert!(call("subsequence", &[empty(), int(1)]).unwrap().items().is_empty());
    assert_eq!(texts(&call("unordered", &[items()]).unwrap()).len(), 5);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.2.1 to
/// §15.2.3] the functions of cardinality, each of its error.
#[test]
fn cardinality() {
    assert!(call("zero-or-one", &[empty()]).unwrap().items().is_empty());
    assert_eq!(code(call("zero-or-one", &[seq(&[int(1), int(2)])])), "FORG0003");
    assert_eq!(texts(&call("one-or-more", &[seq(&[int(1), int(2)])]).unwrap()), ["1", "2"]);
    assert_eq!(code(call("one-or-more", &[empty()])), "FORG0004");
    assert_eq!(text(call("exactly-one", &[int(1)])), "1");
    assert_eq!(code(call("exactly-one", &[empty()])), "FORG0005");
    assert_eq!(code(call("exactly-one", &[seq(&[int(1), int(2)])])), "FORG0005");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.1.2.1,
/// §15.5.1.1] the examples of `op:concatenate` and `op:to`; more items
/// than a sequence holds SQLCODE -900.
#[test]
fn concatenate_and_to() {
    let one_two_three = seq(&[int(1), int(2), int(3)]);
    assert_eq!(texts(&concatenate(&one_two_three, &seq(&[int(4), int(5)])).unwrap()), ["1", "2", "3", "4", "5"]);
    assert_eq!(texts(&concatenate(&one_two_three, &empty()).unwrap()), ["1", "2", "3"]);
    assert!(concatenate(&empty(), &empty()).unwrap().items().is_empty());
    let i = |n: &str| Atomic::new(AtomicType::Integer, n);
    assert_eq!(texts(&to(&i("1"), &i("3")).unwrap()), ["1", "2", "3"]);
    assert!(to(&i("3"), &i("1")).unwrap().items().is_empty());
    assert_eq!(texts(&to(&i("5"), &i("5")).unwrap()), ["5"]);
    assert_eq!(texts(&to(&i("-1"), &i("1")).unwrap()), ["-1", "0", "1"]);
    assert_eq!(to(&i("1"), &i("99999999999999999999")).unwrap_err().sqlcode(), -900_i32);
    assert!(to(&Atomic::new(AtomicType::Decimal, "1"), &i("2")).is_err());
}
