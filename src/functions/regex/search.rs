//! [ISO/IEC 9075-2:2023 §6.31, §6.33] The functions that search a string
//! for an XQuery regular expression's matches: OCCURRENCES_REGEX counts
//! them, POSITION_REGEX gives where one or one of its groups is, and
//! SUBSTRING_REGEX gives it.
//!
//! Each is called with its units, CHARACTERS or OCTETS [Feature F846];
//! POSITION_REGEX then with START or AFTER; then the pattern, the flag,
//! the subject and the start position; and the occurrence and group, but
//! by OCCURRENCES_REGEX. The parser writes those not written as their
//! implicit values [§6.31 SR3, SR4; §6.33 SR8].

use super::{COUNTED, Cache, Subject, counted, operands, parameter};
use crate::error::{Code, Result, fail};
use crate::functions::ScalarFunction;
use crate::functions::strings::{character, text, whole};
use crate::params::ParameterType;
use crate::regex::Captures;
use crate::value::{DataType, Value};

/// [ISO/IEC 9075-2:2023 §6.31] OCCURRENCES_REGEX, no key word of SQL-92.
pub const OCCURRENCES_REGEX: &str = "OCCURRENCES_REGEX";
/// [ISO/IEC 9075-2:2023 §6.31] POSITION_REGEX, no key word of SQL-92.
pub const POSITION_REGEX: &str = "POSITION_REGEX";
/// [ISO/IEC 9075-2:2023 §6.33] SUBSTRING_REGEX, no key word of SQL-92.
pub const SUBSTRING_REGEX: &str = "SUBSTRING_REGEX";
/// Which argument of SUBSTRING_REGEX is its subject.
pub(super) const SUBSTRING_SUBJECT: usize = 3;

/// What a search was called with, its operands known not null.
struct Search<'a> {
    pattern: &'a str,
    flag: &'a str,
    subject: Subject,
    start: i128,
}

/// The search `[units, pattern, flag, subject, from]` asks for; none if
/// an operand is null, which makes the result null [§6.31 GR18b i, GR19b
/// i; §6.33 GR4b i]. A null start position does too, which the rules leave
/// unsaid.
fn search([units, pattern, flag, subject, from]: [&Value; 5]) -> Result<Option<Search<'_>>> {
    let (Some(pattern), Some(flag), Some(subject), Some(start)) = (text(pattern)?, text(flag)?, text(subject)?, whole(Some(from))?) else {
        return Ok(None);
    };
    Ok(Some(Search { pattern, flag, subject: Subject::new(subject, units)?, start }))
}

/// [§6.31 GR19b, §6.33 GR4b] The `occurrence`-th match's group `group` of
/// `found` in `cache`: none if the occurrence is less than 1, the group is
/// none of the pattern's, the start position is past the string, or there
/// are fewer matches; else the match vector and that group's span.
fn nth(cache: &Cache, found: &Search<'_>, occurrence: i128, group: i128) -> Result<Option<(usize, usize)>> {
    // [GR19b ii]
    let Some(occurrence) = usize::try_from(occurrence).ok().filter(|o| *o >= 1) else { return Ok(None) };
    let regex = cache.compiled(found.pattern, found.flag)?;
    // [GR19b iii]
    let Some(group) = usize::try_from(group).ok().filter(|g| *g <= regex.groups()) else { return Ok(None) };
    // [GR19b iv]
    let Some(at) = found.subject.start(found.start) else { return Ok(None) };
    // [GR19b vi]
    let list = regex.list(&found.subject.chars, at, occurrence)?;
    Ok(list.get(occurrence.saturating_sub(1)).and_then(|mv: &Captures| mv.get(group)))
}

/// OCCURRENCES_REGEX ( PAT FLAG FL IN RSS FROM SP USING CLU ).
#[derive(Default)]
pub(super) struct Occurrences {
    cache: Cache,
}

impl ScalarFunction for Occurrences {
    fn name(&self) -> &str {
        OCCURRENCES_REGEX
    }
    /// [§6.31 SR1, SR3] Of character strings and an exact start position,
    /// exact numeric of scale 0.
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [_, pattern, flag, subject, from] = args else { return fail(Code::Api, "[DESIGN] OCCURRENCES_REGEX takes five operands") };
        operands(OCCURRENCES_REGEX, &[("pattern", pattern), ("flag", flag), ("subject", subject)], &[("start position", from)])?;
        Ok(COUNTED)
    }
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        parameter(position, args, &[1, 2, 3], &[4])
    }
    /// \[GR18\] Null if an operand is; -1 if the start position is not of
    /// the string; else how many matches there are from it.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [units, pattern, flag, subject, from] = args else { return fail(Code::Api, "[DESIGN] OCCURRENCES_REGEX takes five operands") };
        let Some(found) = search([units, pattern, flag, subject, from])? else { return Ok(Value::Null) };
        let Some(at) = found.subject.start(found.start) else { return Ok(counted(-1)) };
        let list = self.cache.compiled(found.pattern, found.flag)?.list(&found.subject.chars, at, usize::MAX)?;
        Ok(counted(i128::try_from(list.len()).unwrap_or(i128::MAX)))
    }
}

/// POSITION_REGEX ( START or AFTER PAT FLAG FL IN RSS FROM SP USING CLU
/// OCCURRENCE OCC GROUP CAP ).
#[derive(Default)]
pub(super) struct Position {
    cache: Cache,
}

impl ScalarFunction for Position {
    fn name(&self) -> &str {
        POSITION_REGEX
    }
    /// [§6.31 SR1, SR4] Of character strings and exact numbers, exact
    /// numeric of scale 0.
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [_, _, pattern, flag, subject, from, occurrence, group] = args else {
            return fail(Code::Api, "[DESIGN] POSITION_REGEX takes eight operands");
        };
        let strings = [("pattern", pattern), ("flag", flag), ("subject", subject)];
        operands(POSITION_REGEX, &strings, &[("start position", from), ("occurrence", occurrence), ("group", group)])?;
        Ok(COUNTED)
    }
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        parameter(position, args, &[2, 3, 4], &[5, 6, 7])
    }
    /// \[GR19\] Null if an operand is; where the match or group begins,
    /// or under AFTER the position after it ends; 0 if there is none.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [units, side, pattern, flag, subject, from, occurrence, group] = args else {
            return fail(Code::Api, "[DESIGN] POSITION_REGEX takes eight operands");
        };
        let (Some(occurrence), Some(group)) = (whole(Some(occurrence))?, whole(Some(group))?) else { return Ok(Value::Null) };
        let Some(found) = search([units, pattern, flag, subject, from])? else { return Ok(Value::Null) };
        let after = text(side)? == Some(super::AFTER);
        Ok(counted(match nth(&self.cache, &found, occurrence, group)? {
            Some((start, end)) => found.subject.position(if after { end } else { start }),
            None => 0,
        }))
    }
}

/// SUBSTRING_REGEX ( PAT FLAG FL IN RSS FROM SP USING CLU OCCURRENCE OCC
/// GROUP CAP ).
#[derive(Default)]
pub(super) struct Substring {
    cache: Cache,
}

impl ScalarFunction for Substring {
    fn name(&self) -> &str {
        SUBSTRING_REGEX
    }
    /// [§6.33 SR8f] CHARACTER VARYING as long as its subject at most.
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [_, pattern, flag, subject, from, occurrence, group] = args else {
            return fail(Code::Api, "[DESIGN] SUBSTRING_REGEX takes seven operands");
        };
        let strings = [("pattern", pattern), ("flag", flag), ("subject", subject)];
        operands(SUBSTRING_REGEX, &strings, &[("start position", from), ("occurrence", occurrence), ("group", group)])?;
        Ok(DataType::Varying { length: character(SUBSTRING_REGEX, subject)? })
    }
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        parameter(position, args, &[1, 2, 3], &[4, 5, 6])
    }
    /// \[GR4\] Null if an operand is, or if there is no such match or the
    /// group matched nothing; else what it matched.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [units, pattern, flag, subject, from, occurrence, group] = args else {
            return fail(Code::Api, "[DESIGN] SUBSTRING_REGEX takes seven operands");
        };
        let (Some(occurrence), Some(group)) = (whole(Some(occurrence))?, whole(Some(group))?) else { return Ok(Value::Null) };
        let Some(found) = search([units, pattern, flag, subject, from])? else { return Ok(Value::Null) };
        Ok(match nth(&self.cache, &found, occurrence, group)? {
            Some((start, end)) => Value::Char(found.subject.chars.get(start..end).unwrap_or_default().iter().collect()),
            None => Value::Null,
        })
    }
}
