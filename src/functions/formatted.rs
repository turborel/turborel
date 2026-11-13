//! [ISO/IEC 9075-2:2023 §6.13 SR4, GR5e, GR6e, GR14-GR18] CAST with FORMAT
//! [Feature T839]: a datetime to a character string as a `<datetime
//! template>` writes it [§9.50], or a character string to a datetime as one
//! reads it [§9.51].
//!
//! The template is a `<character string literal>`, so it is read, and held
//! to the Syntax Rules of §9.52, as the statement is bound
//! (`template.rs`); the functions here are made of it then, and none is
//! registered.

mod format;
mod parse;
mod template;

use std::sync::Arc;

use super::ScalarFunction;
use super::strings::text;
use crate::error::{Code, Result, SqlError, fail};
use crate::value::datetime::{Clocked, Datetime, Moment, Now};
use crate::value::types::datetime::{fitted, shown};
use crate::value::{DataType, Value};
use parse::{Pattern, Target};
use template::Template;

/// [ISO/IEC 9075-2:2023 §6.13] FORMAT, no key word of SQL-92, before a
/// `<cast template>`.
pub const FORMAT: &str = "FORMAT";

/// The fields of the datetime type `t`, if it is one.
fn target(t: &DataType) -> Option<Target> {
    match t {
        DataType::Date => Some((Moment::Date, 0, false)),
        DataType::Time { precision, zoned } => Some((Moment::Time, *precision, *zoned)),
        DataType::Timestamp { precision, zoned } => Some((Moment::Timestamp, *precision, *zoned)),
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Boolean
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Row(_)
        | DataType::Extension(_) => None,
    }
}

/// The length of the character string type `t`, and whether fixed, if it
/// is one.
fn string(t: &DataType) -> Option<(u32, bool)> {
    match t {
        DataType::Character { length, .. } => Some((*length, true)),
        DataType::Varying { length, .. } => Some((*length, false)),
        DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Boolean
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Row(_)
        | DataType::Extension(_) => None,
    }
}

/// The error of a CAST the Syntax Rule `rule` refuses, `why`.
fn refused(rule: &str, why: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 {rule}] {why}"))
}

/// [§6.13 SR4] CAST of a value of `from` to `to` FORMAT `template`, as a
/// function of that value, of the SQL-session `now` gives: its instant
/// read only if the cast takes the current year or month [§9.51 GR5].
///
/// # Errors
///
/// SQLCODE -103 unless one type is a datetime and the other a character
/// string \[SR4\], if the template breaks a Syntax Rule of §9.52, or if the
/// string type is shorter than the template's minimum length [§9.50 SR3,
/// §9.51 SR3].
pub fn formatted_cast(from: &DataType, to: &DataType, template: &str, now: Clocked<'_>) -> Result<Arc<dyn ScalarFunction>> {
    // [SR4a]
    if let (Some(datetime), Some((length, fixed))) = (target(from), string(to)) {
        let template = read(template, datetime, length, "§9.50 SR3")?;
        return Ok(Arc::new(Formatted { template, to: to.clone(), length, fixed, zone: now.zone() }));
    }
    // [SR4b]
    if let (Some((length, _)), Some(datetime)) = (string(from), target(to)) {
        let template = read(template, datetime, length, "§9.51 SR3")?;
        let now = if template.needs_now(datetime.0) { now.instant() } else { Now { utc: 0, zone: now.zone() } };
        return Ok(Arc::new(Parsed { pattern: template.pattern()?, to: to.clone(), target: datetime, now }));
    }
    Err(refused("§6.13 SR4", &format!("CAST of {from} to {to} has a FORMAT only from a datetime to a character string, or back")))
}

/// `text` read as a template of `datetime`, for a string of `length`
/// characters at most, as `rule` requires.
fn read(text: &str, (moment, _, zoned): Target, length: u32, rule: &str) -> Result<Template> {
    let template = Template::read(text, moment, zoned)?;
    let least = template.min_length();
    if usize::try_from(length).is_ok_and(|n| n < least) {
        return Err(refused(
            rule,
            &format!("a string of at most {length} characters is shorter than the template '{text}', of {least} characters at least"),
        ));
    }
    Ok(template)
}

/// CAST of a datetime to a character string FORMAT a template.
struct Formatted {
    template: Template,
    to: DataType,
    length: u32,
    fixed: bool,
    /// The SQL-session's displacement, a datetime without time zone shown
    /// in it [§6.1 GR5].
    zone: i16,
}

impl ScalarFunction for Formatted {
    fn name(&self) -> &'static str {
        "CAST"
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(self.to.clone())
    }
    /// [§6.13 GR5e, GR6e] Null of null; else the datetime as the template
    /// writes it, fitted to the type.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22001 if it is longer than the type's length.
    fn call(&self, args: &[Value]) -> Result<Value> {
        match args.first() {
            Some(Value::Datetime(d)) => {
                let d: Datetime = shown(d, Now { utc: 0, zone: self.zone });
                fitted(self.template.format(&d), self.length, self.fixed)
            }
            Some(Value::Null) | None => Ok(Value::Null),
            Some(
                v @ (Value::Char(_)
                | Value::Exact(_)
                | Value::Approx(_)
                | Value::DecFloat(_)
                | Value::Binary(_)
                | Value::Bool(_)
                | Value::Interval(_)
                | Value::Array(_)
                | Value::Row(_)
                | Value::Extension(_)),
            ) => fail(Code::Api, format!("[DESIGN] a formatted CAST of a datetime was given {v:?}")),
        }
    }
}

/// CAST of a character string to a datetime FORMAT a template.
struct Parsed {
    pattern: Pattern,
    to: DataType,
    target: Target,
    /// The SQL-session's clock: its instant if read, else 0001-01-01.
    now: Now,
}

impl ScalarFunction for Parsed {
    fn name(&self) -> &'static str {
        "CAST"
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(self.to.clone())
    }
    /// [§6.13 GR14a i, GR15a i, GR16a i, GR17a i, GR18a i] Null of null
    /// [§9.51 GR2]; else the string, untrimmed, as the template reads it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22007 if the template does not read it, or it writes no
    /// datetime.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(fcs) = text(args.first().unwrap_or(&Value::Null))? else { return Ok(Value::Null) };
        Ok(Value::Datetime(self.pattern.parse(fcs, self.target, self.now)?))
    }
}
