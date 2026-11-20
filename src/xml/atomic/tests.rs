//! Tests of casting atomic values.

use super::*;

fn untyped(text: &str) -> Atomic {
    Atomic::new(AtomicType::UntypedAtomic, text)
}

/// `text`, untyped, cast as `to`: its canonical form, or the error code.
fn cast_text(text: &str, to: AtomicType) -> String {
    match cast(&untyped(text), to) {
        Ok(a) => {
            assert_eq!(a.kind, to);
            a.lexical.into_string()
        }
        Err(e) => {
            assert_eq!(e.sqlstate(), SqlState::XQUERY_ERROR, "{e}");
            let message = e.to_string();
            message.split_whitespace().find(|w| w.starts_with("err:")).unwrap_or("err:?").to_owned()
        }
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR3d vi] [XQueryFO §17.1.1,
/// §17.1.2] [Schema2 §3.2.2, §3.2.3, §3.3.13] numbers and truth values
/// read by their lexical forms, whitespace collapsed, and written as
/// casting them to a string writes them; anything else err:FORG0001.
#[test]
fn numbers_and_truth_values() {
    let cases = [
        (" +012.50 ", AtomicType::Decimal, "12.5"),
        ("-0.0", AtomicType::Decimal, "0"),
        (".5", AtomicType::Decimal, "0.5"),
        ("100.", AtomicType::Decimal, "100"),
        ("1.2.3", AtomicType::Decimal, "err:FORG0001"),
        ("0042", AtomicType::Integer, "42"),
        ("-0", AtomicType::Integer, "0"),
        ("4.0", AtomicType::Integer, "err:FORG0001"),
        ("1", AtomicType::Boolean, "true"),
        ("false", AtomicType::Boolean, "false"),
        ("TRUE", AtomicType::Boolean, "err:FORG0001"),
        ("1.5E0", AtomicType::Double, "1.5"),
        ("1e6", AtomicType::Double, "1.0E6"),
        ("0.000001", AtomicType::Double, "0.000001"),
        ("1.25e-7", AtomicType::Double, "1.25E-7"),
        ("-0", AtomicType::Double, "-0"),
        ("INF", AtomicType::Double, "INF"),
        ("NaN", AtomicType::Float, "NaN"),
        ("0.1", AtomicType::Float, "0.1"),
        ("1E", AtomicType::Double, "err:FORG0001"),
        ("+INF", AtomicType::Double, "err:FORG0001"),
    ];
    for (text, to, expected) in cases {
        assert_eq!(cast_text(text, to), expected, "{text} as {}", to.name());
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [XQueryFO §17.1.3,
/// §17.1.6] casts among numbers and truth
/// values: a decimal's fraction dropped as an integer; a float or double
/// its shortest decimal; a double as a float its mantissa truncated to 24
/// bits, past the float's exponents an infinity or a zero
/// [XQueryFO §17.1.3.1]; NaN and infinity no decimal, err:FOCA0002; zero
/// and NaN false.
#[test]
fn casts_among_numbers() {
    let of = |kind, lexical: &str| Atomic::new(kind, lexical);
    let cases = [
        (of(AtomicType::Decimal, "-3.9"), AtomicType::Integer, "-3"),
        (of(AtomicType::Decimal, "-0.5"), AtomicType::Integer, "0"),
        (of(AtomicType::Double, "2.5"), AtomicType::Decimal, "2.5"),
        (of(AtomicType::Double, "1.0E20"), AtomicType::Integer, "100000000000000000000"),
        (of(AtomicType::Integer, "7"), AtomicType::Double, "7"),
        (of(AtomicType::Double, "0.1"), AtomicType::Float, "0.099999994"),
        (of(AtomicType::Double, "0.5"), AtomicType::Float, "0.5"),
        (of(AtomicType::Double, "-3.4028236E38"), AtomicType::Float, "-3.4028235E38"),
        (of(AtomicType::Double, "1.0E39"), AtomicType::Float, "INF"),
        (of(AtomicType::Double, "-1.0E-46"), AtomicType::Float, "-0"),
        (of(AtomicType::Double, "1.0E-45"), AtomicType::Float, "0"),
        (of(AtomicType::Double, "1.401298464324817E-45"), AtomicType::Float, "1.0E-45"),
        (of(AtomicType::Double, "1.0E-40"), AtomicType::Float, "1.0E-40"),
        (of(AtomicType::Double, "4.9E-324"), AtomicType::Float, "0"),
        (of(AtomicType::Float, "0.1"), AtomicType::Float, "0.1"),
        (of(AtomicType::Float, "0.1"), AtomicType::Double, "0.10000000149011612"),
        (of(AtomicType::Boolean, "true"), AtomicType::Decimal, "1"),
        (of(AtomicType::Decimal, "0"), AtomicType::Boolean, "false"),
        (of(AtomicType::Double, "NaN"), AtomicType::Boolean, "false"),
        (of(AtomicType::Float, "-2"), AtomicType::Boolean, "true"),
        (of(AtomicType::Integer, "5"), AtomicType::String, "5"),
    ];
    for (value, to, expected) in cases {
        assert_eq!(&*cast(&value, to).unwrap().lexical, expected, "{value:?} as {}", to.name());
    }
    let error = cast(&of(AtomicType::Double, "INF"), AtomicType::Decimal).unwrap_err();
    assert!(error.to_string().contains("FOCA0002"), "{error}");
    let refused = cast(&of(AtomicType::Boolean, "true"), AtomicType::Date).unwrap_err();
    assert!(refused.to_string().contains("XPTY0004"), "{refused}");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [XQueryFO §17.1.1,
/// §17.1.2, §17.1.5, §10.2] [Schema2
/// §3.2.7.1, §3.2.8.1, §3.2.9.1] dates and times: their local values
/// kept, a fraction's zeros dropped, a zero time zone Z, 24:00:00 the next
/// day's start; invalid dates err:FORG0001; years past the limits
/// err:FODT0001; a date and time cast as its date or time.
#[test]
fn dates_and_times() {
    let cases = [
        ("2020-02-29", AtomicType::Date, "2020-02-29"),
        ("2021-02-29", AtomicType::Date, "err:FORG0001"),
        ("2020-01-02+05:30", AtomicType::Date, "2020-01-02+05:30"),
        ("2020-01-02-00:00", AtomicType::Date, "2020-01-02Z"),
        ("0000-01-01", AtomicType::Date, "err:FORG0001"),
        ("10000-01-01", AtomicType::Date, "err:FODT0001"),
        ("-0001-01-01", AtomicType::Date, "err:FODT0001"),
        ("12:30:05.500", AtomicType::Time, "12:30:05.5"),
        ("24:00:00", AtomicType::Time, "00:00:00"),
        ("24:00:01", AtomicType::Time, "err:FORG0001"),
        ("12:30:05+14:01", AtomicType::Time, "err:FORG0001"),
        ("12:30", AtomicType::Time, "err:FORG0001"),
        ("1999-12-31T24:00:00", AtomicType::DateTime, "2000-01-01T00:00:00"),
        ("2020-01-02T03:04:05.1234567Z", AtomicType::DateTime, "2020-01-02T03:04:05.123456Z"),
        ("2020-01-02 03:04:05", AtomicType::DateTime, "err:FORG0001"),
    ];
    for (text, to, expected) in cases {
        assert_eq!(cast_text(text, to), expected, "{text} as {}", to.name());
    }
    let instant = Atomic::new(AtomicType::DateTime, "2020-01-02T03:04:05-08:00");
    assert_eq!(&*cast(&instant, AtomicType::Date).unwrap().lexical, "2020-01-02-08:00");
    assert_eq!(&*cast(&instant, AtomicType::Time).unwrap().lexical, "03:04:05-08:00");
    let day = Atomic::new(AtomicType::Date, "2020-01-02");
    assert_eq!(&*cast(&day, AtomicType::DateTime).unwrap().lexical, "2020-01-02T00:00:00");
    assert!(cast(&day, AtomicType::Time).is_err());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j, §9.8 GR9f, GR9g]
/// [XQueryFO §10.7] a date, time or date and time adjusted to UTC and its
/// time zone removed: one with none as it is; a date the date of its start
/// in UTC; and written as §9.8 writes an SQL value, of a number of digits
/// of fraction, its time zone ±hh:mm.
#[test]
fn adjusted_to_utc() {
    let utc = |text: &str, shape| Moment::parse(text, shape).unwrap().unwrap().in_utc().text();
    assert_eq!(utc("2020-01-02T03:04:05+05:00", Shape::DateTime), "2020-01-01T22:04:05");
    assert_eq!(utc("2020-01-02T03:04:05", Shape::DateTime), "2020-01-02T03:04:05");
    assert_eq!(utc("2020-01-02+05:00", Shape::Date), "2020-01-01");
    assert_eq!(utc("2020-01-02-05:00", Shape::Date), "2020-01-02");
    assert_eq!(utc("01:00:00+02:00", Shape::Time), "23:00:00");
    let m = Moment::parse("2020-01-02T03:04:05.5-08:00", Shape::DateTime).unwrap().unwrap();
    assert_eq!(m.sql_literal(), "2020-01-02 03:04:05.5-08:00");
    assert_eq!(m.written(3), "2020-01-02T03:04:05.500-08:00");
    assert_eq!(m.written(0), "2020-01-02T03:04:05.5-08:00");
    let noon = Moment::parse("12:00:00Z", Shape::Time).unwrap().unwrap();
    assert_eq!((noon.written(0), noon.written(2)), ("12:00:00+00:00".to_owned(), "12:00:00.00+00:00".to_owned()));
    assert_eq!(m.year(), 2020);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [XQueryFO §10.3,
/// §17.1.4] durations: years and months, or
/// days to seconds, in canonical form, P0M and PT0S of zero; the other
/// components, or none, err:FORG0001; one cast as the other zero.
#[test]
fn durations() {
    let cases = [
        ("P14M", AtomicType::YearMonthDuration, "P1Y2M"),
        ("-P1Y0M", AtomicType::YearMonthDuration, "-P1Y"),
        ("P0Y", AtomicType::YearMonthDuration, "P0M"),
        ("P1D", AtomicType::YearMonthDuration, "err:FORG0001"),
        ("P", AtomicType::YearMonthDuration, "err:FORG0001"),
        ("P1M1Y", AtomicType::YearMonthDuration, "err:FORG0001"),
        ("PT36H", AtomicType::DayTimeDuration, "P1DT12H"),
        ("P1DT0.50S", AtomicType::DayTimeDuration, "P1DT0.5S"),
        ("-PT90M", AtomicType::DayTimeDuration, "-PT1H30M"),
        ("PT0S", AtomicType::DayTimeDuration, "PT0S"),
        ("P1DT", AtomicType::DayTimeDuration, "err:FORG0001"),
        ("PT1.5M", AtomicType::DayTimeDuration, "err:FORG0001"),
        ("P1Y", AtomicType::DayTimeDuration, "err:FORG0001"),
        ("P99999999999999999999999999999999999999Y", AtomicType::YearMonthDuration, "err:FODT0002"),
    ];
    for (text, to, expected) in cases {
        assert_eq!(cast_text(text, to), expected, "{text} as {}", to.name());
    }
    let months = Atomic::new(AtomicType::YearMonthDuration, "P1Y");
    assert_eq!(&*cast(&months, AtomicType::DayTimeDuration).unwrap().lexical, "PT0S");
    assert_eq!(duration_units("-P1DT1S", false).unwrap(), -86_401_000_000);
    assert!(duration_units("x", true).is_err());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [Schema2 §3.2.15]
/// [XQueryFO §17.1.7] hex binary: two digits
/// an octet, written in upper case; an odd count or no digit is no
/// lexical form.
#[test]
fn hex_binary() {
    assert_eq!(cast_text("0fB7", AtomicType::HexBinary), "0FB7");
    assert_eq!(cast_text("", AtomicType::HexBinary), "");
    assert_eq!(cast_text("ABC", AtomicType::HexBinary), "err:FORG0001");
    assert_eq!(cast_text("0G", AtomicType::HexBinary), "err:FORG0001");
    assert_eq!(hex("00ff"), Some(vec![0, 255]));
    assert_eq!(hex_text(&[1, 171]), "01AB");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §3.1.3.35] [Schema2 §3.2, §3.3,
/// Appendix A, Appendix C.1] [XQueryDM §2.6.2] every built-in atomic
/// type, named once, derived from its base up to a primitive type; what a
/// cast treats it as; its whiteSpace facet; and the ordered and numeric
/// fundamental facets of its primitive type.
#[test]
fn the_built_in_types_and_their_derivation() {
    let names: Vec<&str> = AtomicType::all().map(AtomicType::name).collect();
    assert_eq!(names.len(), 44);
    for (i, name) in names.iter().enumerate() {
        assert!(!names[i + 1..].contains(name), "{name} twice");
    }
    assert!(AtomicType::Byte.derives_from(AtomicType::Decimal) && AtomicType::Byte.derives_from(AtomicType::Byte));
    assert!(!AtomicType::Byte.derives_from(AtomicType::UnsignedByte) && !AtomicType::Decimal.derives_from(AtomicType::Integer));
    assert!(AtomicType::Id.derives_from(AtomicType::String) && AtomicType::DayTimeDuration.derives_from(AtomicType::Duration));
    assert_eq!(AtomicType::UntypedAtomic.base(), None);
    assert_eq!(AtomicType::NcName.base(), Some(AtomicType::Name));
    assert_eq!(AtomicType::UnsignedShort.bounds(), (Some("0"), Some("65535")));
    assert_eq!((AtomicType::Language.primitive(), AtomicType::Short.primitive()), (Primitive::String, Primitive::Integer));
    let spaces = [AtomicType::String, AtomicType::NormalizedString, AtomicType::Token, AtomicType::UntypedAtomic, AtomicType::Date];
    let expected = [WhiteSpace::Preserve, WhiteSpace::Replace, WhiteSpace::Collapse, WhiteSpace::Preserve, WhiteSpace::Collapse];
    assert_eq!(spaces.map(AtomicType::white_space), expected);
    assert_eq!((Primitive::Integer.schema(), Primitive::DayTimeDuration.schema()), (Primitive::Decimal, Primitive::Duration));
    assert_eq!(
        (Primitive::Float.ordered(), Primitive::GDay.ordered(), Primitive::QName.ordered()),
        (Ordered::Total, Ordered::Partial, Ordered::False)
    );
    assert!(Primitive::Integer.numeric() && !Primitive::Duration.numeric());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [Schema2 §3.2.6.1,
/// §3.2.10.1 to §3.2.14.1, §3.2.16, §3.2.17] [XQueryFO §17.1.1, §17.1.2]
/// the lexical forms of the other primitive types: a duration of any
/// components, written as XQuery writes it; the Gregorian types, their
/// fields checked as a date's; base64 in groups of four, spaces ignored,
/// padded at the end alone; any string a URI.
#[test]
fn other_primitive_types_lexical_forms() {
    let cases = [
        ("P1Y2M3DT4H5.5S", AtomicType::Duration, "P1Y2M3DT4H5.5S"),
        ("-P13M", AtomicType::Duration, "-P1Y1M"),
        ("P0Y", AtomicType::Duration, "PT0S"),
        ("PT36H", AtomicType::Duration, "P1DT12H"),
        ("P1.5Y", AtomicType::Duration, "err:FORG0001"),
        ("PT", AtomicType::Duration, "err:FORG0001"),
        (" 2020-02 ", AtomicType::GYearMonth, "2020-02"),
        ("2020-13", AtomicType::GYearMonth, "err:FORG0001"),
        ("2020-2", AtomicType::GYearMonth, "err:FORG0001"),
        ("2020+05:00", AtomicType::GYear, "2020+05:00"),
        ("0000", AtomicType::GYear, "err:FORG0001"),
        ("20200", AtomicType::GYear, "err:FODT0001"),
        ("--02-29", AtomicType::GMonthDay, "--02-29"),
        ("--02-30", AtomicType::GMonthDay, "err:FORG0001"),
        ("---31Z", AtomicType::GDay, "---31Z"),
        ("---1", AtomicType::GDay, "err:FORG0001"),
        ("--12--", AtomicType::GMonth, "--12--"),
        ("--12", AtomicType::GMonth, "err:FORG0001"),
        ("--13--", AtomicType::GMonth, "err:FORG0001"),
        ("AQID", AtomicType::Base64Binary, "AQID"),
        ("A Q I =", AtomicType::Base64Binary, "AQI="),
        ("AQ==", AtomicType::Base64Binary, "AQ=="),
        ("AQ=A", AtomicType::Base64Binary, "err:FORG0001"),
        ("A===", AtomicType::Base64Binary, "err:FORG0001"),
        ("AQ==AQID", AtomicType::Base64Binary, "err:FORG0001"),
        ("AQI", AtomicType::Base64Binary, "err:FORG0001"),
        (" a b ", AtomicType::AnyUri, "a b"),
    ];
    for (text, to, expected) in cases {
        assert_eq!(cast_text(text, to), expected, "{text} as {}", to.name());
    }
    assert_eq!(base64("AQID").unwrap(), [1, 2, 3]);
    assert_eq!(base64_text(&[1]), "AQ==");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [Schema2 §3.3,
/// Appendix A] [XQueryFO §17.1.1, §17.2] the derived types: an integer
/// type's bounds, a sign taken as its base takes it; a string type's
/// white space normalized; language, NMTOKEN, Name and NCName literals by
/// their productions; anything else err:FORG0001.
#[test]
fn derived_types_by_the_facets_they_fix() {
    let cases = [
        ("+0", AtomicType::NonPositiveInteger, "0"),
        ("1", AtomicType::NonPositiveInteger, "err:FORG0001"),
        ("0", AtomicType::NegativeInteger, "err:FORG0001"),
        ("-1", AtomicType::NegativeInteger, "-1"),
        ("127", AtomicType::Byte, "127"),
        ("128", AtomicType::Byte, "err:FORG0001"),
        ("-129", AtomicType::Byte, "err:FORG0001"),
        ("18446744073709551615", AtomicType::UnsignedLong, "18446744073709551615"),
        ("18446744073709551616", AtomicType::UnsignedLong, "err:FORG0001"),
        ("-1", AtomicType::UnsignedInt, "err:FORG0001"),
        ("0", AtomicType::PositiveInteger, "err:FORG0001"),
        ("1.0", AtomicType::Int, "err:FORG0001"),
        (" a \t b ", AtomicType::Token, "a b"),
        (" a\tb ", AtomicType::NormalizedString, " a b "),
        ("en-US", AtomicType::Language, "en-US"),
        ("i-klingon", AtomicType::Language, "i-klingon"),
        ("x-abcdefgh", AtomicType::Language, "x-abcdefgh"),
        ("x-abcdefghi", AtomicType::Language, "err:FORG0001"),
        ("eng", AtomicType::Language, "err:FORG0001"),
        ("en-", AtomicType::Language, "err:FORG0001"),
        ("-a", AtomicType::NmToken, "-a"),
        ("a b", AtomicType::NmToken, "err:FORG0001"),
        ("-a", AtomicType::Name, "err:FORG0001"),
        ("a:b", AtomicType::Name, "a:b"),
        ("a:b", AtomicType::NcName, "err:FORG0001"),
        ("_x", AtomicType::Id, "_x"),
        ("1x", AtomicType::IdRef, "err:FORG0001"),
        ("e", AtomicType::Entity, "e"),
    ];
    for (text, to, expected) in cases {
        assert_eq!(cast_text(text, to), expected, "{text} as {}", to.name());
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [XQueryFO §17.1,
/// §17.1.4, §17.1.5, §17.1.7, §17.4, §17.5] casts within and across the
/// type hierarchy: cast as what the target is treated as, then checked
/// against its facets; durations to each other; dates to the Gregorian
/// types; hex and base64 to each other; `xs:NOTATION` abstract,
/// err:XPST0080; `xs:QName` from no string, err:XPTY0004, nor from one
/// not a literal, err:XPST0083.
#[test]
fn casts_within_and_across_the_hierarchy() {
    let of = |kind, lexical: &str| Atomic::new(kind, lexical);
    let cases = [
        (of(AtomicType::Double, "3.7"), AtomicType::Int, "3"),
        (of(AtomicType::Int, "5"), AtomicType::Short, "5"),
        (of(AtomicType::Boolean, "true"), AtomicType::Byte, "1"),
        (of(AtomicType::Decimal, "1.5"), AtomicType::Token, "1.5"),
        (of(AtomicType::Short, "-5"), AtomicType::Decimal, "-5"),
        (of(AtomicType::HexBinary, "0FB7"), AtomicType::Base64Binary, "D7c="),
        (of(AtomicType::Base64Binary, "D7c="), AtomicType::HexBinary, "0FB7"),
        (of(AtomicType::DayTimeDuration, "P1D"), AtomicType::Duration, "P1D"),
        (of(AtomicType::Duration, "P1Y2MT3H"), AtomicType::YearMonthDuration, "P1Y2M"),
        (of(AtomicType::Duration, "-P1Y2MT3H"), AtomicType::DayTimeDuration, "-PT3H"),
        (of(AtomicType::YearMonthDuration, "P1Y"), AtomicType::Duration, "P1Y"),
        (of(AtomicType::DateTime, "2020-02-29T12:00:00+05:00"), AtomicType::GMonthDay, "--02-29+05:00"),
        (of(AtomicType::Date, "2021-03-04"), AtomicType::GYear, "2021"),
        (of(AtomicType::Date, "2021-03-04"), AtomicType::GMonth, "--03--"),
        (of(AtomicType::Date, "2021-03-04Z"), AtomicType::GDay, "---04Z"),
        (of(AtomicType::DateTime, "2021-03-04T00:00:00"), AtomicType::GYearMonth, "2021-03"),
        (of(AtomicType::Duration, "P1D"), AtomicType::String, "P1D"),
    ];
    for (value, to, expected) in cases {
        assert_eq!(&*cast(&value, to).unwrap().lexical, expected, "{value:?} as {}", to.name());
    }
    let refused = [
        (of(AtomicType::Double, "300"), AtomicType::Byte, "FORG0001"),
        (of(AtomicType::Decimal, "1.5"), AtomicType::NcName, "FORG0001"),
        (of(AtomicType::String, "a"), AtomicType::Notation, "XPST0080"),
        (of(AtomicType::String, "a:b"), AtomicType::QName, "XPST0083"),
        (of(AtomicType::UntypedAtomic, "a:b"), AtomicType::QName, "XPTY0004"),
        (of(AtomicType::Time, "12:00:00"), AtomicType::GDay, "XPTY0004"),
        (of(AtomicType::AnyUri, "a"), AtomicType::HexBinary, "XPTY0004"),
        (of(AtomicType::Date, "2020-01-01"), AtomicType::AnyUri, "XPTY0004"),
    ];
    for (value, to, code) in refused {
        let error = cast(&value, to).unwrap_err();
        assert!(error.to_string().contains(code), "{value:?} as {}: {error}", to.name());
    }
    assert_eq!(from_lexical(" a:b ", AtomicType::QName).unwrap().unwrap().lexical.as_ref(), "a:b");
    assert!(from_lexical("a:b:c", AtomicType::Notation).unwrap().is_none());
    assert_eq!(normalize(" a \n\r b ", WhiteSpace::Replace), " a    b ");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [Schema2 §3.2.6.1]
/// [XQueryFO §10.1.1, §17.1.1] text that is no lexical form of a
/// duration, of no P, of a component of no number or designator, of a
/// designator unknown or out of order, of a fraction outside the seconds,
/// or of a T of no time, is err:FORG0001 cast as each duration type; a
/// component too large to hold is err:FODT0002; xs:duration reads both
/// halves, its own canonical form.
#[test]
fn duration_lexical_forms_are_checked() {
    let types = [AtomicType::Duration, AtomicType::YearMonthDuration, AtomicType::DayTimeDuration];
    for text in ["1Y", "-1D", "P1", "PY", "P1Q", "P1DT", "PT1D", "P1D1Y", "P1.5Y", "P1.5D", "PT1.H", "P1Dé", "PT1H2H", "P-1Y", "PT"] {
        for to in types {
            assert_eq!(cast_text(text, to), "err:FORG0001", "{text} as {}", to.name());
        }
    }
    let huge = "9".repeat(40);
    for (text, to) in [
        (format!("P{huge}Y"), AtomicType::Duration),
        (format!("P{huge}M"), AtomicType::YearMonthDuration),
        (format!("P{huge}D"), AtomicType::DayTimeDuration),
        (format!("PT{huge}S"), AtomicType::Duration),
        (format!("P1Y{huge}M"), AtomicType::Duration),
        (format!("P195{}DT1{}H", "0".repeat(25), "0".repeat(28)), AtomicType::DayTimeDuration),
        (format!("P195{}DT1{}H", "0".repeat(25), "0".repeat(28)), AtomicType::Duration),
    ] {
        assert_eq!(cast_text(&text, to), "err:FODT0002", "{text} as {}", to.name());
    }
    for (text, expected) in [("P1Y2M3DT4H5M6.5S", "P1Y2M3DT4H5M6.5S"), ("-P13M", "-P1Y1M"), ("PT0S", "PT0S"), ("P0Y", "PT0S")] {
        assert_eq!(cast_text(text, AtomicType::Duration), expected, "{text}");
    }
}
