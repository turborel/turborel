//! Unit tests of interval values (RULES.md R-TEST-1, R-TEST-2): strings
//! read and written in each qualifier, fields, and conversion between
//! qualifiers. Split from interval.rs (RULES.md R-SIZE-2).

use super::*;

/// The qualifier `start` TO `end` of `leading` digits and 6 of fraction.
fn q(start: Field, end: Field, leading: u32) -> Qualifier {
    Qualifier::new(start, end, leading, 6)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.3, §4.5.2 Table 7, §10.1
/// SR5, §6.10 GR5e] an interval's string is its fields in order, the
/// leading of no more digits than its precision, each other of two and in
/// its range, a sign before it; it is written as read, but for the
/// fraction's digits past the precision, which are dropped.
#[test]
fn strings_are_read_and_written() {
    let cases = [
        (q(Field::Year, Field::Month, 2), "1-02", 14),
        (q(Field::Year, Field::Year, 4), "1999", 1999 * 12),
        (q(Field::Month, Field::Month, 3), "-100", -100),
        (
            q(Field::Day, Field::Second, 2),
            "3 04:05:06.500000",
            3 * unit(Field::Day) + 4 * unit(Field::Hour) + 5 * unit(Field::Minute) + 6_500_000,
        ),
        (q(Field::Hour, Field::Minute, 2), "-12:59", -(12 * unit(Field::Hour) + 59 * unit(Field::Minute))),
        (q(Field::Minute, Field::Second, 9), "123456789:00.000001", 123_456_789 * unit(Field::Minute) + 1),
        (q(Field::Second, Field::Second, 2), "7.250000", 7_250_000),
    ];
    for (qualifier, text, units) in cases {
        assert_eq!(parse(&qualifier, text), Some(units), "{text}");
        assert_eq!(Interval { units, qualifier }.to_string(), text);
    }
    let second = Qualifier::new(Field::Second, Field::Second, 2, 2);
    assert_eq!(parse(&second, "1.239"), Some(1_230_000), "dropped");
    assert_eq!(Interval { units: 1_230_000, qualifier: second }.to_string(), "1.23");
    let day_hour = q(Field::Day, Field::Hour, 2);
    assert_eq!(parse(&day_hour, "1 2"), parse(&day_hour, "1 02"), "of any digits");
    for bad in ["", "1", "1 24", "100 01", "1:01", "1 01:00", "x", "1 01 ", "1 024"] {
        assert!(parse(&day_hour, bad).is_none(), "{bad}");
    }
    assert!(parse(&q(Field::Year, Field::Month, 2), "1-12").is_none(), "months to 11");
    assert!(parse(&q(Field::Minute, Field::Second, 2), "1:60").is_none(), "seconds to 59");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.6 GR3a i, §9.2 GR3m, §6.10
/// GR12d, §6.15 GR5e, §10.1 GR5] an interval's fields as shown, of its
/// sign, the leading field holding all above; one converted to another
/// qualifier truncated to its least field, refused if its leading field
/// cannot hold it; qualifiers joined; their positions.
#[test]
fn fields_and_conversion() {
    let i = Interval { units: -(30 * unit(Field::Hour) + 15 * unit(Field::Minute) + 1), qualifier: q(Field::Hour, Field::Second, 2) };
    assert_eq!([Field::Hour, Field::Minute, Field::Second].map(|f| i.field(f)), [Some(-30), Some(-15), Some(-1)]);
    assert_eq!(i.field(Field::Day), None);
    let days = i.to(q(Field::Day, Field::Hour, 2)).unwrap();
    assert_eq!(days.to_string(), "-1 06", "truncated");
    assert!(i.to(q(Field::Hour, Field::Hour, 1)).is_none(), "30 hours in one digit");
    let joined = q(Field::Year, Field::Year, 2).joined(&q(Field::Month, Field::Month, 3), 4);
    assert_eq!(joined.to_string(), "YEAR(4) TO MONTH");
    assert!(joined.year_month() && !q(Field::Day, Field::Day, 2).year_month());
    assert_eq!(Qualifier::new(Field::Second, Field::Second, 3, 2).to_string(), "SECOND(3, 2)");
    assert_eq!(q(Field::Day, Field::Second, 2).to_string(), "DAY(2) TO SECOND(6)");
    assert_eq!(q(Field::Day, Field::Day, 3).to_string(), "DAY(3)");
    assert_eq!(q(Field::Day, Field::Second, 2).positions(), 2 + 9 + 7);
    assert_eq!(q(Field::Year, Field::Month, 4).positions(), 7);
    assert_eq!(field_name(Field::Minute), "MINUTE");
}
