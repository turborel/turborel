//! Tests of simple type definitions: facets, restriction, lists, unions,
//! validation and casts.

use std::sync::Arc;

use super::*;
use crate::error::{Code, SqlState};
use crate::xml::{Atomic, Primitive};

/// `base` restricted by `facets`, none fixed.
fn restricted(base: SimpleType, facets: Vec<Facet>) -> Result<SimpleType> {
    let specified: Vec<FacetSpec> = facets.into_iter().map(FacetSpec::new).collect();
    SimpleType::restrict(&Arc::new(base), &specified)
}

/// The built-in `t` restricted by `facets`.
fn of(t: AtomicType, facets: Vec<Facet>) -> SimpleType {
    restricted(SimpleType::built_in(t), facets).unwrap()
}

/// The message of the error a restriction fails with.
fn refused(base: SimpleType, facets: Vec<Facet>) -> String {
    restricted(base, facets).unwrap_err().to_string()
}

/// Whether `literal` is valid for `t`; an invalid one is err:FORG0001.
fn valid(t: &SimpleType, literal: &str) -> bool {
    match t.validate(literal) {
        Ok(_) => true,
        Err(e) => {
            assert_eq!(e.sqlstate(), SqlState::XQUERY_ERROR, "{e}");
            assert!(e.to_string().contains("FORG0001"), "{e}");
            false
        }
    }
}

/// A literal facet value.
fn lit(s: &str) -> Box<str> {
    s.into()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.5 GR8] [Schema2 §4.1.5
/// applicable facets] the facets that apply to a type: by its primitive
/// type's row, a list's or a union's own; none to `xdt:untypedAtomic`.
#[test]
fn applicable_facets_by_base() {
    let atomic = |t| Variety::Atomic(t);
    assert!(applicable(&atomic(AtomicType::String), FacetKind::Length));
    assert!(applicable(&atomic(AtomicType::Token), FacetKind::WhiteSpace));
    assert!(!applicable(&atomic(AtomicType::Boolean), FacetKind::Enumeration));
    assert!(applicable(&atomic(AtomicType::Boolean), FacetKind::Pattern));
    assert!(applicable(&atomic(AtomicType::Short), FacetKind::TotalDigits));
    assert!(!applicable(&atomic(AtomicType::Date), FacetKind::Length));
    assert!(applicable(&atomic(AtomicType::DayTimeDuration), FacetKind::MinExclusive));
    assert!(applicable(&atomic(AtomicType::HexBinary), FacetKind::MaxLength));
    assert!(!applicable(&atomic(AtomicType::Double), FacetKind::FractionDigits));
    assert!(!applicable(&atomic(AtomicType::UntypedAtomic), FacetKind::Pattern));
    let list = SimpleType::list(Arc::new(SimpleType::built_in(AtomicType::Int))).unwrap();
    assert!(applicable(list.variety(), FacetKind::Length) && !applicable(list.variety(), FacetKind::MaxInclusive));
    let union = SimpleType::union(vec![Arc::new(SimpleType::built_in(AtomicType::Int))]).unwrap();
    assert!(applicable(union.variety(), FacetKind::Enumeration) && !applicable(union.variety(), FacetKind::WhiteSpace));
    assert!(refused(SimpleType::built_in(AtomicType::Boolean), vec![Facet::Enumeration(lit("true"))]).contains("§4.1.5"));
    assert_eq!(FacetKind::FractionDigits.to_string(), "fractionDigits");
}

/// Each restriction of `cases` refused by the rule named.
fn refusals(cases: Vec<(SimpleType, Vec<Facet>, &str)>) {
    for (base, facets, rule) in cases {
        let message = refused(base, facets);
        assert!(message.contains(rule), "{rule}: {message}");
    }
}

/// Whether each of `good` is valid for `t` and none of `bad` is.
fn check(t: &SimpleType, good: &[&str], bad: &[&str]) {
    for literal in good {
        assert!(valid(t, literal), "{literal} for {}", t.name());
    }
    for literal in bad {
        assert!(!valid(t, literal), "{literal} for {}", t.name());
    }
}

/// `base` restricted by `facet`, fixed.
fn fixed(base: SimpleType, facet: Facet) -> SimpleType {
    SimpleType::restrict(&Arc::new(base), &[FacetSpec { facet, fixed: true }]).unwrap()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.5 GR8] [Schema2 §4.1.3,
/// §4.3.1.4, §4.3.2.4, §4.3.3.4, §4.3.6.4, §4.3.11.4, §4.3.12.4] a
/// restriction's lengths, white space and digits: each facet once a step;
/// length alone; no wider than the base's; a fixed facet kept; minLength
/// no more than maxLength, fractionDigits than totalDigits.
#[test]
fn restriction_of_counts() {
    let built = SimpleType::built_in;
    let five = of(AtomicType::String, vec![Facet::Length(5)]);
    let some = of(AtomicType::String, vec![Facet::MinLength(2), Facet::MaxLength(5)]);
    refusals(vec![
        (built(AtomicType::String), vec![Facet::MaxLength(3), Facet::MaxLength(4)], "§4.1.3"),
        (five.clone(), vec![Facet::MaxLength(9)], "§4.3.1.4"),
        (five.clone(), vec![Facet::Length(4)], "§4.3.1.4"),
        (some.clone(), vec![Facet::MinLength(1)], "§4.3.2.4"),
        (some.clone(), vec![Facet::MaxLength(6)], "§4.3.3.4"),
        (some, vec![Facet::MinLength(4), Facet::MaxLength(3)], "§4.3.2.4"),
        (fixed(built(AtomicType::String), Facet::MaxLength(5)), vec![Facet::MaxLength(4)], "fixed"),
        (built(AtomicType::Token), vec![Facet::WhiteSpace(WhiteSpace::Preserve)], "§4.3.6.4"),
        (of(AtomicType::String, vec![Facet::WhiteSpace(WhiteSpace::Replace)]), vec![Facet::WhiteSpace(WhiteSpace::Preserve)], "§4.3.6.4"),
        (built(AtomicType::Int), vec![Facet::WhiteSpace(WhiteSpace::Preserve)], "fixed"),
        (built(AtomicType::Integer), vec![Facet::FractionDigits(2)], "fixed"),
        (built(AtomicType::Decimal), vec![Facet::TotalDigits(0)], "§4.3.11"),
        (of(AtomicType::Decimal, vec![Facet::TotalDigits(5)]), vec![Facet::TotalDigits(6)], "§4.3.11.4"),
        (built(AtomicType::Decimal), vec![Facet::TotalDigits(2), Facet::FractionDigits(3)], "§4.3.12.4"),
    ]);
    assert!(restricted(five, vec![Facet::Length(5)]).is_ok());
    assert!(restricted(built(AtomicType::Integer), vec![Facet::FractionDigits(0)]).is_ok());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.5 GR8d] [Schema2 §4.3.5.5,
/// §4.3.4.2, §4.3.7.2, §4.3.7.4 to §4.3.10.4] a restriction's bounds,
/// enumeration and patterns: values of the base's value space, its bounds
/// aside; one bound a side a step; none beyond the base's, an exclusive
/// one restated; lower below upper, an indeterminate order no conflict; a
/// pattern a regular expression of XML Schema.
#[test]
fn restriction_of_values() {
    let built = SimpleType::built_in;
    let byte = || built(AtomicType::Byte);
    let hundred = of(AtomicType::Byte, vec![Facet::MaxInclusive(lit("100"))]);
    let above = of(AtomicType::Byte, vec![Facet::MinExclusive(lit("5"))]);
    let nine = fixed(byte(), Facet::MaxInclusive(lit("9")));
    refusals(vec![
        (built(AtomicType::Int), vec![Facet::MaxInclusive(lit("3000000000"))], "§4.3.7.2"),
        (byte(), vec![Facet::MaxInclusive(lit("x"))], "§4.3.7.2"),
        (hundred.clone(), vec![Facet::MaxInclusive(lit("120"))], "§4.3.7.4"),
        (hundred.clone(), vec![Facet::MinExclusive(lit("100"))], "§4.3.9.4"),
        (hundred, vec![Facet::MaxExclusive(lit("102"))], "§4.3.8.4"),
        (above.clone(), vec![Facet::MinInclusive(lit("5"))], "§4.3.10.4"),
        (byte(), vec![Facet::MaxInclusive(lit("1")), Facet::MaxExclusive(lit("2"))], "§4.3.8.4"),
        (byte(), vec![Facet::MinInclusive(lit("1")), Facet::MinExclusive(lit("0"))], "§4.3.9.4"),
        (byte(), vec![Facet::MinInclusive(lit("5")), Facet::MaxInclusive(lit("4"))], "§4.3.7.4"),
        (byte(), vec![Facet::MinInclusive(lit("5")), Facet::MaxExclusive(lit("5"))], "§4.3.10.4"),
        (byte(), vec![Facet::MinExclusive(lit("5")), Facet::MaxExclusive(lit("4"))], "§4.3.8.4"),
        (nine.clone(), vec![Facet::MaxInclusive(lit("8"))], "fixed"),
        (built(AtomicType::Int), vec![Facet::Enumeration(lit("x"))], "§4.3.5.5"),
        (built(AtomicType::String), vec![Facet::Pattern(lit("(+|-)"))], "§4.3.4.2"),
    ]);
    assert!(restricted(byte(), vec![Facet::MaxExclusive(lit("127"))]).is_ok());
    assert!(restricted(above, vec![Facet::MinExclusive(lit("5"))]).is_ok());
    assert!(restricted(nine, vec![Facet::MaxInclusive(lit("+09"))]).is_ok());
    let month_or_days = vec![Facet::MinInclusive(lit("P1M")), Facet::MaxInclusive(lit("P30D"))];
    assert!(restricted(built(AtomicType::Duration), month_or_days).is_ok());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j, §9.5 GR8a, GR8c]
/// [Schema2 §4.1.4, §4.3] a literal validated: normalized, matched against
/// the patterns, a step's ORed and steps ANDed, read, then checked against
/// each facet: lengths in characters or octets, none of a QName; an
/// enumeration's values by equality; digits; bounds, an indeterminate
/// order out of them.
#[test]
fn validation_by_facets() {
    let first = of(AtomicType::String, vec![Facet::Pattern(lit("a.*")), Facet::Pattern(lit("b.*"))]);
    let three = of(AtomicType::String, vec![Facet::WhiteSpace(WhiteSpace::Collapse), Facet::Length(3)]);
    let days = ["--01-01", "--07-04", "--12-25"].map(|d| Facet::Enumeration(lit(d)));
    let digits = vec![Facet::TotalDigits(5), Facet::FractionDigits(2)];
    let later = vec![Facet::MinInclusive(lit("2000-01-01T00:00:00Z"))];
    let cases: [(SimpleType, &[&str], &[&str]); 14] = [
        (of(AtomicType::String, vec![Facet::Pattern(lit("\\d{3}-[A-Z]{2}"))]), &["123-AB"], &["123-ab", " 123-AB"]),
        (restricted(first, vec![Facet::Pattern(lit(".*z"))]).unwrap(), &["az", "bz"], &["cz", "ab"]),
        (three.clone(), &["  a b  "], &["ab"]),
        (of(AtomicType::GMonthDay, days.to_vec()), &["--07-04"], &["--07-05"]),
        (of(AtomicType::Decimal, vec![Facet::Enumeration(lit("1.0")), Facet::Enumeration(lit("2"))]), &["1", "1.00"], &["3"]),
        (of(AtomicType::Decimal, digits), &["123.45", "1234.5", "12.500"], &["123.456", "1234.56", "0.001"]),
        (of(AtomicType::Integer, vec![Facet::MaxInclusive(lit("100"))]), &["100"], &["101", "x"]),
        (of(AtomicType::DateTime, later), &["2000-01-02T00:00:00"], &["2000-01-01T00:00:00", "1999-12-31T00:00:00Z"]),
        (of(AtomicType::Duration, vec![Facet::MaxInclusive(lit("P1M"))]), &["P27D", "P1M"], &["P30D"]),
        (of(AtomicType::HexBinary, vec![Facet::MaxLength(2)]), &["0FB7"], &["0FB7AA"]),
        (of(AtomicType::Base64Binary, vec![Facet::MinLength(3)]), &["AQID"], &["AQ=="]),
        (of(AtomicType::Double, vec![Facet::MinExclusive(lit("0"))]), &["1E-300"], &["-0"]),
        (of(AtomicType::Boolean, vec![Facet::Pattern(lit("[01]"))]), &["1"], &["true"]),
        (of(AtomicType::QName, vec![Facet::Length(1)]), &["a:b"], &[]),
    ];
    for (t, good, bad) in cases {
        check(&t, good, bad);
    }
    assert_eq!(three.validate(" a  b ").unwrap()[0].lexical.as_ref(), "a b");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [Schema2 §2.5.1.2,
/// §3.3.5, §4.1.2.2, §4.1.5 list of atomic] lists: a list's items its
/// literal split at white space, its lengths counted in items, its
/// enumeration's values lists; no list of a list; the built-in lists of
/// one item at least.
#[test]
fn lists() {
    let ints = SimpleType::list(Arc::new(SimpleType::built_in(AtomicType::Integer))).unwrap();
    assert_eq!(ints.validate(" 1  2 3 ").unwrap().len(), 3);
    check(&ints, &["", "1"], &["1 x"]);
    let three = restricted(ints.clone(), vec![Facet::Length(3), Facet::Enumeration(lit("1 2 3"))]).unwrap();
    check(&three, &["1 2 03"], &["1 2", "3 2 1"]);
    assert!(SimpleType::list(Arc::new(ints)).unwrap_err().to_string().contains("§4.1.5"));
    check(&SimpleType::built_in_list(BuiltInList::NmTokens), &["a b"], &["", "a !"]);
    check(&SimpleType::built_in_list(BuiltInList::IdRefs), &["a b"], &["1"]);
    check(&SimpleType::built_in_list(BuiltInList::Entities), &["e"], &[""]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [Schema2 §2.5.1.3,
/// §4.1.2.3] unions: a value of the first member that validates it, the
/// font size example of §4.1.2.3; its own enumeration and patterns; its
/// members' unions flattened; none of no member; a list of a union.
#[test]
fn unions() {
    let size = of(AtomicType::PositiveInteger, vec![Facet::MinInclusive(lit("8")), Facet::MaxInclusive(lit("72"))]);
    let words = ["small", "medium", "large"].map(|n| Facet::Enumeration(lit(n)));
    let font = SimpleType::union(vec![Arc::new(size), Arc::new(of(AtomicType::NmToken, words.to_vec()))]).unwrap();
    assert_eq!(font.validate("12").unwrap()[0].kind, AtomicType::PositiveInteger);
    assert_eq!(font.validate(" large ").unwrap()[0].kind, AtomicType::NmToken);
    check(&font, &[], &["huge", "7"]);
    let fewer = restricted(font.clone(), vec![Facet::Enumeration(lit("small")), Facet::Enumeration(lit("8"))]).unwrap();
    check(&fewer, &["8", "small"], &["12"]);
    let patterned = restricted(font.clone(), vec![Facet::Pattern(lit("[0-9]+"))]).unwrap();
    check(&patterned, &[" 12 "], &["large"]);
    let flat = SimpleType::union(vec![Arc::new(font), Arc::new(SimpleType::built_in(AtomicType::Date))]).unwrap();
    assert!(matches!(flat.variety(), Variety::Union(members) if members.len() == 3));
    assert!(SimpleType::union(Vec::new()).is_err());
    let mixed = SimpleType::list(Arc::new(flat)).unwrap();
    let kinds: Vec<AtomicType> = mixed.validate("9 small 2020-01-01").unwrap().iter().map(|a| a.kind).collect();
    assert_eq!(kinds, [AtomicType::PositiveInteger, AtomicType::NmToken, AtomicType::Date]);
}

/// Verifies: [DESIGN] RULES.md R-CF-6, D18: lists and unions nest at most
/// `MAX_TYPE_NESTING` deep, a deeper type SQLCODE -900.
#[test]
fn nesting_is_bounded() {
    let mut t = SimpleType::built_in(AtomicType::Int);
    let mut error = None;
    for _ in 0..MAX_TYPE_NESTING {
        let union = match SimpleType::union(vec![Arc::new(t.clone())]) {
            Ok(u) => u,
            Err(e) => {
                error = Some(e);
                break;
            }
        };
        match SimpleType::list(Arc::new(union)) {
            Ok(l) => t = l,
            Err(e) => {
                error = Some(e);
                break;
            }
        }
    }
    assert_eq!(error.expect("too deep").code, Code::Unsupported);
    assert!(valid(&t, "1 2"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [XQueryFO §17.1.1,
/// §17.4, §17.5] a cast as a type derived by restriction: a string read
/// as a literal, its pattern matched before; another value cast as the
/// built-in type, its pattern matched against its canonical
/// representation as of its own primitive type, so an integer is never a
/// distance of two fraction digits, or as of the target's; no cast as a
/// list, err:XPST0051.
#[test]
fn casts_as_derived_types() {
    let atomic = |kind, lexical: &str| Atomic::new(kind, lexical);
    let small = of(AtomicType::Decimal, vec![Facet::TotalDigits(3), Facet::FractionDigits(1)]);
    assert_eq!(small.cast(&atomic(AtomicType::UntypedAtomic, " 12.3 ")).unwrap().lexical.as_ref(), "12.3");
    assert_eq!(small.cast(&atomic(AtomicType::Double, "12.5")).unwrap().kind, AtomicType::Decimal);
    for refused in [atomic(AtomicType::UntypedAtomic, "1.25"), atomic(AtomicType::Double, "1.25"), atomic(AtomicType::Integer, "1234")] {
        let error = small.cast(&refused).unwrap_err();
        assert!(error.to_string().contains("FORG0001"), "{error}");
    }
    let distance = of(AtomicType::Decimal, vec![Facet::Pattern(lit("\\d+\\.\\d{2}"))]);
    assert!(distance.cast(&atomic(AtomicType::UntypedAtomic, "5.00")).is_ok());
    assert!(distance.cast(&atomic(AtomicType::Integer, "5")).is_err());
    assert!(distance.cast(&atomic(AtomicType::Double, "5")).is_err());
    let whole = of(AtomicType::DateTime, vec![Facet::Pattern(lit("\\p{Nd}{4}-\\p{Nd}{2}-\\p{Nd}{2}T\\p{Nd}{2}:\\p{Nd}{2}:\\p{Nd}{2}"))]);
    assert!(whole.cast(&atomic(AtomicType::DateTime, "2020-01-02T03:04:05")).is_ok());
    assert!(whole.cast(&atomic(AtomicType::DateTime, "2020-01-02T03:04:05.5")).is_err());
    assert!(whole.cast(&atomic(AtomicType::Date, "2020-01-02")).is_ok());
    let list = SimpleType::built_in_list(BuiltInList::NmTokens);
    assert!(list.cast(&atomic(AtomicType::String, "a")).unwrap_err().to_string().contains("XPST0051"));
    assert_eq!(SimpleType::built_in(AtomicType::Int).named("my:int").name(), "my:int");
    assert_eq!(list.atomic(), None);
    assert_eq!(Primitive::Decimal.schema(), Primitive::Decimal);
}
