//! An SQL interval type mapped to XQuery's duration types [ISO/IEC
//! 9075-14:2006 draft §9.5 GR8l]: `xdt:yearMonthDuration` or
//! `xdt:dayTimeDuration`, restricted by a pattern of exactly the fields
//! of the type, its leading field of the digits of its precision.
//!
//! Two readings make the pattern describe what §9.8 GR9h writes. Of a
//! fractional seconds precision of 0, the seconds are written with no
//! fraction \[GR9h vi, vii\], so SECS is `\p{Nd}{2}S`, as of a type of no
//! seconds \[GR8l ii 2\], not `\p{Nd}{2}.\p{Nd}{0}S`. Of INTERVAL SECOND,
//! the seconds are the leading field, of L digits \[GR9h vii 13\], so they
//! are `\p{Nd}{PLIT}`, as each other leading field is, not `\p{Nd}{2}`.

use std::fmt::Write;

use super::types::XDT_PREFIX;
use crate::value::datetime::Field;
use crate::value::interval::Qualifier;

/// The digits of a field after the leading one [§9.5 GR8l iii].
const FIELD_DIGITS: u8 = 2;

/// [§9.5 GR8l vi] The `xs:restriction` of an interval type of the
/// qualifier `q`: of DTYPE \[v\], by FACETP \[iii\].
pub(super) fn duration(q: Qualifier) -> String {
    let base = if q.year_month() { "yearMonthDuration" } else { "dayTimeDuration" };
    format!(r#"<xs:restriction base="{XDT_PREFIX}:{base}"><xs:pattern value="{}"/></xs:restriction>"#, pattern(q))
}

/// [§9.5 GR8l iii] FACETP's value: an optional minus, P, then each field
/// of `q` with its designator, T before the first of hours, minutes and
/// seconds; the leading field of PLIT digits, the others of two; seconds
/// with a fraction of SLIT digits after `.` if the type has one \[ii\].
fn pattern(q: Qualifier) -> String {
    let mut out = String::from("-?P");
    for field in q.fields() {
        let timed = matches!(field, Field::Hour | Field::Minute | Field::Second);
        if timed && !out.contains('T') {
            out.push('T');
        }
        let digits = if field == q.start { q.leading } else { FIELD_DIGITS };
        // Writing to a String cannot fail.
        let _ = write!(out, r"\p{{Nd}}{{{digits}}}");
        if field == Field::Second && q.fraction > 0 {
            let _ = write!(out, r".\p{{Nd}}{{{}}}", q.fraction);
        }
        out.push(designator(field));
    }
    out
}

/// The designator of `field` in a duration [Schema2 §3.2.6.1].
fn designator(field: Field) -> char {
    match field {
        Field::Year => 'Y',
        Field::Month | Field::Minute => 'M',
        Field::Day => 'D',
        Field::Hour => 'H',
        Field::Second => 'S',
    }
}
