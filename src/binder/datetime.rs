//! [ISO/IEC 9075:1992 §5.3, §6.6, §6.8, §6.10] Datetimes as a statement
//! binds them: a `<datetime literal>`, of the SQL-session's time zone
//! displacement if it writes none; CURRENT_DATE, CURRENT_TIME and
//! CURRENT_TIMESTAMP, each the statement's one instant [§6.8 GR3]; EXTRACT;
//! and CAST to or from a datetime, given the session's displacement and
//! the current date, which its rules read.

use std::sync::Arc;

use super::{Binder, Scope, rule};
use crate::ast::{DatetimeFunction, ExtractField, ValueExpression};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::logical::Expr;
use crate::value::datetime::{Datetime, Field, MAX_TIME_PRECISION, MICROS_PER_SECOND, Moment, Now, Written};
use crate::value::types::datetime::{cast_at, current};
use crate::value::{DataType, Decimal, SMALLINT_PRECISION, Value, pow10};

/// [ISO/IEC 9075:1992 §6.1 SR25] The `<time precision>` implied: 0.
const TIME_PRECISION: u32 = 0;
/// [ISO/IEC 9075:1992 §6.1 SR25] The `<timestamp precision>` implied: 6.
const TIMESTAMP_PRECISION: u32 = 6;

/// The datetime type of `moment`, of `precision`, `zoned` or not.
pub(super) fn type_of(moment: Moment, precision: u32, zoned: bool) -> DataType {
    match moment {
        Moment::Date => DataType::Date,
        Moment::Time => DataType::Time { precision, zoned },
        Moment::Timestamp => DataType::Timestamp { precision, zoned },
    }
}

/// [ISO/IEC 9075:1992 §5.3 SR15-SR18] The value and type of the
/// `<datetime literal>` `w`: of its `<time zone interval>`, WITH TIME ZONE,
/// or else of the session's displacement, `now`'s; of as many digits of
/// the second's fraction as it writes.
///
/// # Errors
///
/// SQLSTATE 22007 if its instant's year in UTC leaves 0001 to 9999 [§6.1
/// SR6].
pub(crate) fn literal(w: &Written, now: Now) -> Result<(Value, DataType)> {
    let d = Datetime::of_local(w.moment, w.local, w.zone.unwrap_or(now.zone), w.zone.is_some(), w.precision);
    if !d.in_range() {
        return fail_with(
            Code::Data,
            SqlState::INVALID_DATETIME_FORMAT,
            format!("[ISO/IEC 9075:1992 §6.1 SR6] {d} is not of a year from 0001 to 9999 in UTC"),
        );
    }
    Ok((Value::Datetime(d), type_of(w.moment, w.precision, w.zone.is_some())))
}

/// [ISO/IEC 9075:1992 §6.10] CAST to `to`, a datetime type or from a
/// datetime, in the session `now` gives: a function of the statement, its
/// instant and displacement fixed in it.
struct CastAt {
    to: DataType,
    now: Now,
}

impl ScalarFunction for CastAt {
    fn name(&self) -> &'static str {
        "CAST"
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(self.to)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        cast_at(args.first().cloned().unwrap_or(Value::Null), &self.to, self.now)
    }
}

/// `e`, of type `from`, as the SQL-session `now` gives shows it: a datetime
/// without time zone in its displacement [§6.1 SR5], so its fields are
/// read as shown; any other as it is.
pub(super) fn shown(e: Expr, from: DataType, now: Now) -> Expr {
    match from {
        DataType::Time { zoned: false, .. } | DataType::Timestamp { zoned: false, .. } => {
            Expr::Call(ScalarRef(Arc::new(CastAt { to: from, now })), vec![e])
        }
        DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::Bit { .. }
        | DataType::BitVarying { .. }
        | DataType::Extension(_) => e,
    }
}

/// [ISO/IEC 9075:1992 §6.10 SR4] Whether CAST takes a value of `from` to
/// `to`, either a datetime type: from and to a character string, a
/// datetime of the same fields, a date and a timestamp, a time and a
/// timestamp; no number, no bit string.
fn castable(from: &DataType, to: &DataType) -> bool {
    let moment = |t: &DataType| match t {
        DataType::Date => Some(Moment::Date),
        DataType::Time { .. } => Some(Moment::Time),
        DataType::Timestamp { .. } => Some(Moment::Timestamp),
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::Bit { .. }
        | DataType::BitVarying { .. }
        | DataType::Extension(_) => None,
    };
    match (moment(from), moment(to)) {
        (Some(Moment::Date), Some(Moment::Time)) | (Some(Moment::Time), Some(Moment::Date)) => false,
        (Some(_), Some(_)) => true,
        (Some(_), None) => to.is_character(),
        (None, _) => from.is_character(),
    }
}

/// [ISO/IEC 9075:1992 §6.6] EXTRACT of `field` from a datetime, as shown.
struct Extract {
    field: ExtractField,
    /// The scale of its result: of SECOND, the source's precision.
    scale: u32,
}

impl ScalarFunction for Extract {
    fn name(&self) -> &'static str {
        "EXTRACT"
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Exact { precision: SMALLINT_PRECISION.saturating_add(self.scale), scale: self.scale })
    }
    /// [GR3a] The field's value; [GR3a ii] of TIMEZONE_HOUR and
    /// TIMEZONE_MINUTE, the hours and minutes of its displacement, of its
    /// sign; [GR3b] null of null.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(Value::Datetime(d)) = args.first() else { return Ok(Value::Null) };
        let n = match self.field {
            ExtractField::TimezoneHour => i64::from(d.zone.checked_div(60).unwrap_or(0)),
            ExtractField::TimezoneMinute => i64::from(d.zone.checked_rem(60).unwrap_or(0)),
            ExtractField::Datetime(Field::Second) => {
                let micros = d.field(Field::Second).unwrap_or(0);
                let unit =
                    pow10(MAX_TIME_PRECISION.saturating_sub(self.scale)).and_then(|u| i64::try_from(u).ok()).unwrap_or(MICROS_PER_SECOND);
                return Ok(Value::Exact(Decimal::new(i128::from(micros.checked_div(unit).unwrap_or(0)), self.scale)));
            }
            ExtractField::Datetime(f) => d.field(f).unwrap_or(0),
        };
        Ok(Value::Exact(Decimal::new(i128::from(n), 0)))
    }
}

/// The fields a datetime type has, if it is one.
fn fields(t: &DataType) -> Option<(Moment, u32, bool)> {
    match t {
        DataType::Date => Some((Moment::Date, 0, false)),
        DataType::Time { precision, zoned } => Some((Moment::Time, *precision, *zoned)),
        DataType::Timestamp { precision, zoned } => Some((Moment::Timestamp, *precision, *zoned)),
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::Bit { .. }
        | DataType::BitVarying { .. }
        | DataType::Extension(_) => None,
    }
}

/// [ISO/IEC 9075:1992 §6.6 SR3] Whether a datetime of `moment` has
/// `field`.
fn has(moment: Moment, field: Field) -> bool {
    match field {
        Field::Year | Field::Month | Field::Day => moment != Moment::Time,
        Field::Hour | Field::Minute | Field::Second => moment != Moment::Date,
    }
}

/// [ISO/IEC 9075:1992 §6.8 SR1] The type of `f`: of a type WITH TIME
/// ZONE, of the precision written or the one implied [§6.1 SR25].
pub fn function_type(f: DatetimeFunction) -> DataType {
    let implied = if f.moment == Moment::Time { TIME_PRECISION } else { TIMESTAMP_PRECISION };
    type_of(f.moment, f.precision.unwrap_or(implied), true)
}

/// The value of the `<datetime literal>` `w`, of UTC if it writes no
/// displacement: where no SQL-session gives one.
pub fn literal_value(w: &Written) -> Value {
    Value::Datetime(Datetime::of_local(w.moment, w.local, w.zone.unwrap_or(0), w.zone.is_some(), w.precision))
}

impl Binder<'_> {
    /// [ISO/IEC 9075:1992 §6.8] CURRENT_DATE, CURRENT_TIME or
    /// CURRENT_TIMESTAMP: the statement's instant, a constant of it [GR3].
    pub(super) fn datetime_function(&self, f: DatetimeFunction) -> (Expr, DataType) {
        let t = function_type(f);
        let precision = if let DataType::Time { precision, .. } | DataType::Timestamp { precision, .. } = t { precision } else { 0 };
        (Expr::Const(current(f.moment, precision, self.now)), t)
    }

    /// [ISO/IEC 9075:1992 §6.6] EXTRACT (`field` FROM `source`), its source
    /// a datetime that has the field \[SR3a\], or of TIMEZONE_HOUR or
    /// TIMEZONE_MINUTE one WITH TIME ZONE \[SR3b\]; exact, of scale 0, but of
    /// SECOND the source's precision \[SR4\].
    pub(super) fn extract(&self, field: ExtractField, source: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (e, t) = self.value_expression(source, scope)?;
        let Some((moment, precision, zoned)) = fields(&t) else {
            return Err(rule(format!("[ISO/IEC 9075:1992 §6.6 SR3] EXTRACT is of a datetime, not {t}")));
        };
        let (valid, scale) = match field {
            ExtractField::Datetime(f) => (has(moment, f), if f == Field::Second { precision } else { 0 }),
            ExtractField::TimezoneHour | ExtractField::TimezoneMinute => (zoned, 0),
        };
        if !valid {
            return Err(rule(format!("[ISO/IEC 9075:1992 §6.6 SR3] {t} has no field {field:?}")));
        }
        let f = Extract { field, scale };
        let result = f.return_type(&[t])?;
        Ok((Expr::Call(ScalarRef(Arc::new(f)), vec![shown(e, t, self.now)]), result))
    }

    /// [ISO/IEC 9075:1992 §6.10] CAST of `e`, of type `from`, to `to`, one
    /// of them a datetime type: of the session's displacement and date.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if CAST takes no value of `from` to `to` [SR4].
    pub(super) fn datetime_cast(&self, e: Expr, from: DataType, to: DataType) -> Result<Expr> {
        if !castable(&from, &to) {
            return Err(SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075:1992 §6.10 SR4] CAST of {from} to {to} is not valid")));
        }
        Ok(Expr::Call(ScalarRef(Arc::new(CastAt { to, now: self.now })), vec![e]))
    }
}
