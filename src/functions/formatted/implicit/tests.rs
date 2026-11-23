//! Tests of the `<JSON datetime template>`'s implicit datetime data type
//! and conversion: each case of §9.46 SR10, and §9.51 applied by it.

use super::*;
use crate::value::Value;
use crate::value::datetime::{Datetime, MICROS_PER_DAY, Moment, Now, days_from_civil};

/// The conversion of `template`, in UTC on 2024-06-15.
fn cast(template: &str) -> Result<Arc<dyn ScalarFunction>> {
    let utc = days_from_civil(2024_i64, 6_i64, 15_i64).saturating_mul(MICROS_PER_DAY);
    json_datetime_cast(template, Clocked::from(Now { utc, zone: 0_i16 }))
}

/// The implicit datetime data type of `template`.
fn implicit(template: &str) -> DataType {
    cast(template).unwrap().return_type(&[]).unwrap()
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 SR10a, SR10b, SR10c, SR10f i to v]
/// a template is dated by a year, a rounded year, a month or a day; timed
/// by an hour, a minute, a second, a fraction or A.M. and P.M.; zoned by
/// TZH or TZM; and its implicit datetime data type is TIMESTAMP if dated
/// and timed, TIME if timed alone, DATE if dated alone, WITH TIME ZONE if
/// zoned.
#[test]
fn implicit_types_of_templates_9_46() {
    for dated in ["YYYY", "RRRR", "YY-MM", "DD", "DDD"] {
        assert_eq!(implicit(dated), DataType::Date, "{dated}");
    }
    let time = |precision: u32, zoned: bool| DataType::Time { precision, zoned };
    let timestamp = |precision: u32, zoned: bool| DataType::Timestamp { precision, zoned };
    for timed in ["HH24", "HH12 A.M.", "MI", "SS", "SSSSS", "FF1"] {
        assert!(matches!(implicit(timed), DataType::Time { zoned: false, .. }), "{timed}");
    }
    assert_eq!(implicit("YYYY-MM-DD HH24:MI:SS TZH:TZM"), timestamp(0_u32, true), "SR10f i");
    assert_eq!(implicit("YYYY-MM-DD HH24:MI:SS"), timestamp(0_u32, false), "SR10f ii");
    assert_eq!(implicit("HH24:MI TZH"), time(0_u32, true), "SR10f iii");
    assert_eq!(implicit("HH24:MI"), time(0_u32, false), "SR10f iv");
    assert_eq!(implicit("YYYY-MM-DD"), DataType::Date, "SR10f v");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 SR10b i, ii] the fractional
/// seconds precision of a timed template is FFn's n, else 0;
/// \[DESIGN\] RULES.md D18: of FF7 to FF9, past turborel's 6 digits of a
/// second's fraction, SQLCODE -900.
#[test]
fn fractional_precision_of_templates_9_46() {
    assert_eq!(implicit("HH24:MI:SS.FF3"), DataType::Time { precision: 3_u32, zoned: false });
    assert_eq!(implicit("YYYY-MM-DD HH24:MI:SS.FF6"), DataType::Timestamp { precision: 6_u32, zoned: false });
    assert_eq!(implicit("YYYY-MM-DD HH24:MI:SS"), DataType::Timestamp { precision: 0_u32, zoned: false });
    for fine in ["SS.FF7", "SS.FF9"] {
        assert_eq!(cast(fine).err().map(|e| e.sqlcode()), Some(-900_i32), "{fine}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 SR10, SR10d, SR10e, SR10h] a
/// template not of the lexical grammar of a `<datetime template>`, zoned
/// but not timed, neither dated nor timed, or breaking a Syntax Rule of
/// §9.52 for its implicit type, is SQLCODE -103, citing the rule. MC/DC
/// of `zoned && !timed`: TZH:TZM refused, HH24:MI TZH and YYYY taken
/// (`implicit_types_of_templates_9_46`); of `!dated && !timed`: `-`
/// refused, YYYY and HH24 taken.
#[test]
fn templates_refused_9_46() {
    let rule = |template: &str| {
        let e = cast(template).err().unwrap();
        assert_eq!(e.code, Code::SyntaxRule, "{template}");
        e.rule.unwrap()
    };
    assert_eq!(rule("yyyy-mm-dd"), "ISO/IEC 9075-2:2023 §9.52 SR2");
    assert_eq!(rule(""), "ISO/IEC 9075-2:2023 §9.52 SR2");
    assert_eq!(rule("YYYY-MM-DD TZH"), "ISO/IEC 9075-2:2023 §9.46 SR10d");
    assert_eq!(rule("TZH:TZM"), "ISO/IEC 9075-2:2023 §9.46 SR10d");
    assert_eq!(rule("-"), "ISO/IEC 9075-2:2023 §9.46 SR10e");
    assert_eq!(rule("YYYY RR"), "ISO/IEC 9075-2:2023 §9.52 SR5");
    assert_eq!(rule("HH12:MI"), "ISO/IEC 9075-2:2023 §9.52 SR8, SR9");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 b] [§9.51 GR5
/// to GR12] the conversion reads a character string by the template to a
/// value of its implicit datetime data type, taking a year not written in
/// full from the SQL-session's current year; a string the template does
/// not read is the exception of §9.51, SQLSTATE 22007, which `datetime()`
/// replaces.
#[test]
fn strings_read_by_templates_9_46() {
    let read = |template: &str, text: &str| cast(template).unwrap().call(&[Value::Char(text.into())]);
    let Value::Datetime(date) = read("YYYY-MM-DD", "2024-02-29").unwrap() else { panic!("a date") };
    assert_eq!((date.moment, date.to_string()), (Moment::Date, "2024-02-29".to_owned()));
    let Value::Datetime(year) = read("YY-MM-DD", "25-01-02").unwrap() else { panic!("a date") };
    assert_eq!(year.to_string(), "2025-01-02");
    let Value::Datetime(zoned) = read("HH24:MI:SS.FF2 TZH:TZM", "12:34:56.78 +05:30").unwrap() else { panic!("a time") };
    let expected: Datetime = Datetime::of_local(Moment::Time, 45_296_780_000_i64, 330_i16, true, 2_u32);
    assert_eq!((zoned.moment, zoned.zoned, zoned.precision, zoned.zone, zoned.utc), (Moment::Time, true, 2_u8, 330_i16, expected.utc));
    assert_eq!(read("YYYY-MM-DD", "2023-02-29").unwrap_err().sqlstate().as_str(), "22007");
    assert_eq!(read("YYYY-MM-DD", "02/29/2024").unwrap_err().sqlstate().as_str(), "22007");
}
