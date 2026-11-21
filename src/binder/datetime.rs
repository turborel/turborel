//! [ISO/IEC 9075:1992 §5.3, §6.6, §6.8, §6.10] Datetimes as a statement
//! binds them: a `<datetime literal>`, of the SQL-session's time zone
//! displacement if it writes none; CURRENT_DATE, CURRENT_TIME and
//! CURRENT_TIMESTAMP, each the statement's one instant [§6.8 GR3]; EXTRACT,
//! of a datetime or an interval; and CAST to or from a datetime or an
//! interval, given the session's displacement and the current date, which
//! its rules read.

use std::sync::Arc;

use super::{Binder, Scope, rule};
use crate::ast::{DatetimeFunction, ExtractField, ValueExpression};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::functions::over_vectors::ints_of;
use crate::functions::{Arg, ScalarFunction, ScalarRef};
use crate::logical::{Expr, Slot};
use crate::storage::Column;
use crate::value::Spelled;
use crate::value::datetime::{Clocked, Datetime, Field, MAX_TIME_PRECISION, MICROS_PER_SECOND, Moment, Now, Written};
use crate::value::interval::{MAX_LEADING_PRECISION, Qualifier, field_name};
use crate::value::types::datetime::{cast_at, current, current_local};
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
/// or else of the session's displacement, `zone`; of as many digits of
/// the second's fraction as it writes.
///
/// # Errors
///
/// SQLSTATE 22007 if its instant's year in UTC leaves 0001 to 9999 [§6.1
/// GR6].
pub(crate) fn literal(w: &Written, zone: i16) -> Result<(Value, DataType)> {
    let d = Datetime::of_local(w.moment, w.local, w.zone.unwrap_or(zone), w.zone.is_some(), w.precision);
    if !d.in_range() {
        return fail_with(
            Code::Data,
            SqlState::INVALID_DATETIME_FORMAT,
            format!("[ISO/IEC 9075:1992 §6.1 GR6] {d} is not of a year from 0001 to 9999 in UTC"),
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
        Ok(self.to.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        cast_at(args.first().cloned().unwrap_or(Value::Null), &self.to, self.now)
    }
}

/// `e`, of type `from`, as the SQL-session `now` gives shows it: a datetime
/// without time zone in its displacement [§6.1 GR5], so its fields are
/// read as shown; any other as it is.
pub(super) fn shown(e: Expr, from: DataType, now: Clocked<'_>) -> Expr {
    match from {
        DataType::Time { zoned: false, .. } | DataType::Timestamp { zoned: false, .. } => {
            // A CAST to its own type reads the displacement alone, which
            // what keeps a plan compares (RULES.md D613).
            Expr::Call(ScalarRef(Arc::new(CastAt { to: from, now: Now { utc: 0, zone: now.zone() } })), vec![e])
        }
        DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Json
        | DataType::Extension(_)
        | DataType::Xml(_) => e,
    }
}

/// [ISO/IEC 9075:1992 §9.2 GR3l, §6.1 GR5] `e`, of type `from`, as stored
/// in a column of type `to`: a time or timestamp without time zone, in one
/// WITH TIME ZONE, of the session's displacement, `now`'s, which it
/// implies; any other as it is.
pub(super) fn stored(e: Expr, from: DataType, to: &DataType, now: Clocked<'_>) -> Expr {
    if takes_zone(&from.clone(), to) { shown(e, from, now) } else { e }
}

/// Whether a value of `from`, stored in a column of `to`, takes the
/// session's displacement: a time or timestamp without time zone, in one
/// WITH TIME ZONE.
pub(super) fn takes_zone(from: &DataType, to: &DataType) -> bool {
    matches!(from, DataType::Time { zoned: false, .. } | DataType::Timestamp { zoned: false, .. })
        && matches!(to, DataType::Time { zoned: true, .. } | DataType::Timestamp { zoned: true, .. })
}

/// [ISO/IEC 9075:1992 §6.10 SR4] Whether CAST takes a value of `from` to
/// `to`: as `castable` says, either a datetime or interval type; else as
/// `binary_castable` says, as `Binder::cast` refuses.
pub(crate) fn cast_is_valid(from: &DataType, to: &DataType) -> bool {
    if from.is_datetime() || to.is_datetime() || from.is_interval() || to.is_interval() {
        return castable(from, to);
    }
    binary_castable(from, to)
}

/// [ISO/IEC 9075-2:2023 §6.13 SR7] Whether CAST takes a value of `from` to
/// `to` as far as binary strings go: a binary string to and from binary
/// strings alone, or a type beyond the standard, which says for itself.
pub(crate) fn binary_castable(from: &DataType, to: &DataType) -> bool {
    let beyond = matches!(from, DataType::Extension(_)) || matches!(to, DataType::Extension(_));
    beyond || from.is_binary() == to.is_binary()
}

/// [ISO/IEC 9075:1992 §6.10 SR4] Whether CAST takes a value of `from` to
/// `to`, either a datetime type: from and to a character string, a
/// datetime of the same fields, a date and a timestamp, a time and a
/// timestamp; no number, no binary string.
fn castable(from: &DataType, to: &DataType) -> bool {
    if from.is_interval() || to.is_interval() {
        return interval_castable(from, to);
    }
    let moment = |t: &DataType| match t {
        DataType::Date => Some(Moment::Date),
        DataType::Time { .. } => Some(Moment::Time),
        DataType::Timestamp { .. } => Some(Moment::Timestamp),
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Json
        | DataType::Extension(_)
        | DataType::Xml(_) => None,
    };
    match (moment(from), moment(to)) {
        (Some(Moment::Date), Some(Moment::Time)) | (Some(Moment::Time), Some(Moment::Date)) => false,
        (Some(_), Some(_)) => true,
        (Some(_), None) => to.is_character(),
        (None, _) => from.is_character(),
    }
}

/// [ISO/IEC 9075:1992 §6.10 SR4-SR6] Whether CAST takes a value of `from`
/// to `to`, either an interval type: from and to a character string, an
/// interval of the same class, an exact number to and from one of a
/// single field.
fn interval_castable(from: &DataType, to: &DataType) -> bool {
    let single = |t: &DataType| if let DataType::Interval(q) = t { q.start == q.end } else { false };
    match (from, to) {
        (DataType::Interval(a), DataType::Interval(b)) => a.year_month() == b.year_month(),
        (DataType::Exact { .. }, DataType::Interval(_)) => single(to),
        (DataType::Interval(_), DataType::Exact { .. }) => single(from),
        (DataType::Interval(_), _) => to.is_character(),
        (_, _) => from.is_character(),
    }
}

/// [ISO/IEC 9075:1992 §6.6] EXTRACT of `field` from a datetime, as shown,
/// or from an interval.
struct Extract {
    field: ExtractField,
    /// The scale of its result: of SECOND, the source's precision.
    scale: u32,
    /// The digits of its result before the point.
    digits: u32,
}

impl ScalarFunction for Extract {
    fn name(&self) -> &'static str {
        "EXTRACT"
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Exact { precision: self.digits.saturating_add(self.scale), scale: self.scale, spelled: Spelled::Numeric })
    }
    /// [GR3a] The field's value; [GR3a ii] of TIMEZONE_HOUR and
    /// TIMEZONE_MINUTE, the hours and minutes of its displacement, of its
    /// sign; [GR3b] null of null.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let n = match (args.first(), self.field) {
            (Some(Value::Datetime(d)), ExtractField::TimezoneHour) => Some(i128::from(d.zone.checked_div(60).unwrap_or(0))),
            (Some(Value::Datetime(d)), ExtractField::TimezoneMinute) => Some(i128::from(d.zone.checked_rem(60).unwrap_or(0))),
            (Some(Value::Datetime(d)), ExtractField::Datetime(f)) => d.field(f).map(i128::from),
            (Some(Value::Interval(i)), ExtractField::Datetime(f)) => i.field(f),
            _ => None,
        };
        let Some(n) = n else { return Ok(Value::Null) };
        if self.field != ExtractField::Datetime(Field::Second) {
            return Ok(Value::Exact(Decimal::new(n, 0)));
        }
        // Microseconds, to the scale.
        let unit = pow10(MAX_TIME_PRECISION.saturating_sub(self.scale)).unwrap_or(i128::from(MICROS_PER_SECOND));
        Ok(Value::Exact(Decimal::new(n.checked_div(unit).unwrap_or(0), self.scale)))
    }
    /// \[DESIGN\] (RULES.md D363) Of a vector of datetimes, each one's
    /// field as `call` gives it, an exact number, of the scale of SECOND
    /// or else of 0.
    fn eval_batch(&self, args: &[Arg<'_>], rows: usize) -> Option<Result<Column>> {
        let [Arg::Column(Column::Datetimes(datetimes))] = args else { return None };
        let unit = i64::try_from(pow10(MAX_TIME_PRECISION.saturating_sub(self.scale))?).ok()?;
        let field = |d: Datetime| match self.field {
            ExtractField::TimezoneHour => Some(i64::from(d.zone.checked_div(60).unwrap_or(0))),
            ExtractField::TimezoneMinute => Some(i64::from(d.zone.checked_rem(60).unwrap_or(0))),
            ExtractField::Datetime(Field::Second) => d.field(Field::Second).map(|n| n.checked_div(unit).unwrap_or(0)),
            ExtractField::Datetime(f) => d.field(f),
        };
        let scale = if self.field == ExtractField::Datetime(Field::Second) { self.scale } else { 0 };
        Some(Ok(ints_of(rows, scale, |n| datetimes.get(n).and_then(field))))
    }
}

/// [ISO/IEC 9075:1992 §6.6 SR3a, SR4, GR3a i] EXTRACT of `field` from
/// `e`, an interval of `q`: a field it has, of its sign, SECOND of the
/// scale of its fraction.
///
/// # Errors
///
/// SQLCODE -103 of a field it has not, or of TIMEZONE_HOUR or
/// TIMEZONE_MINUTE.
fn interval_extract(field: ExtractField, e: Expr, q: Qualifier) -> Result<(Expr, DataType)> {
    let ExtractField::Datetime(f) = field else {
        return Err(rule(format!("[ISO/IEC 9075:1992 §6.6 SR3b] INTERVAL {q} has no time zone")));
    };
    if !q.has(f) {
        return Err(rule(format!("[ISO/IEC 9075:1992 §6.6 SR3a] INTERVAL {q} has no field {}", field_name(f))));
    }
    let scale = if f == Field::Second { u32::from(q.fraction) } else { 0 };
    let x = Extract { field, scale, digits: MAX_LEADING_PRECISION };
    let result = x.return_type(&[])?;
    Ok((Expr::Call(ScalarRef(Arc::new(x)), vec![e]), result))
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
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Json
        | DataType::Extension(_)
        | DataType::Xml(_) => None,
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
/// ZONE, of the precision written or the one implied [§6.1 SR25]; of
/// LOCALTIME or LOCALTIMESTAMP, WITHOUT TIME ZONE [ISO/IEC 9075-2:2023
/// §6.43 SR2, SR3].
pub fn function_type(f: DatetimeFunction) -> DataType {
    let implied = if f.moment == Moment::Time { TIME_PRECISION } else { TIMESTAMP_PRECISION };
    type_of(f.moment, f.precision.unwrap_or(implied), !f.local)
}

/// The value of the `<datetime literal>` `w`, of the displacement `zone`
/// if it writes none [ISO/IEC 9075:1992 §5.3 SR18]: where a statement's
/// `Now` is not at hand.
pub fn literal_value(w: &Written, zone: i16) -> Value {
    Value::Datetime(Datetime::of_local(w.moment, w.local, w.zone.unwrap_or(zone), w.zone.is_some(), w.precision))
}

impl Binder<'_> {
    /// [ISO/IEC 9075:1992 §6.8] CURRENT_DATE, CURRENT_TIME or
    /// CURRENT_TIMESTAMP: the statement's instant, a constant of it [GR3];
    /// [ISO/IEC 9075-2:2023 §6.43 SR2, SR3] LOCALTIME or LOCALTIMESTAMP,
    /// that instant CAST to a time or timestamp WITHOUT TIME ZONE; in no
    /// constraint [§11.9 SR5, §11.34 SR6].
    pub(super) fn datetime_function(&self, f: DatetimeFunction) -> Result<(Expr, DataType)> {
        if self.purpose.constrains() {
            return Err(rule(
                "[ISO/IEC 9075:1992 §11.9 SR5, §11.34 SR6] a constraint's condition shall not contain CURRENT_DATE, CURRENT_TIME, CURRENT_TIMESTAMP, LOCALTIME or LOCALTIMESTAMP",
            ));
        }
        let t = function_type(f);
        let precision = if let DataType::Time { precision, .. } | DataType::Timestamp { precision, .. } = t { precision } else { 0 };
        // A statement kept with its instant a slot reads it from the slot,
        // filled at each execution (RULES.md D613).
        if let Some(index) = self.params.clock_slot() {
            let slot = Slot { index, data_type: type_of(Moment::Timestamp, TIMESTAMP_PRECISION, true), at: 0 };
            let at = Current { moment: f.moment, precision, local: f.local, to: t.clone() };
            return Ok((Expr::Call(ScalarRef(Arc::new(at)), vec![Expr::Dynamic(slot)]), t));
        }
        let now = self.now.instant();
        let value = if f.local { current_local(f.moment, precision, now) } else { current(f.moment, precision, now) };
        Ok((Expr::Const(value), t))
    }

    /// [ISO/IEC 9075:1992 §6.6] EXTRACT (`field` FROM `source`), its source
    /// a datetime that has the field \[SR3a\], or of TIMEZONE_HOUR or
    /// TIMEZONE_MINUTE one WITH TIME ZONE \[SR3b\]; exact, of scale 0, but of
    /// SECOND the source's precision \[SR4\].
    pub(super) fn extract(&self, field: ExtractField, source: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (e, t) = self.value_expression(source, scope)?;
        if let DataType::Interval(q) = t {
            return interval_extract(field, e, q);
        }
        let Some((moment, precision, zoned)) = fields(&t) else {
            return Err(rule(format!("[ISO/IEC 9075:1992 §6.6 SR3] EXTRACT is of a datetime or an interval, not {t}")));
        };
        let (valid, scale) = match field {
            ExtractField::Datetime(f) => (has(moment, f), if f == Field::Second { precision } else { 0 }),
            ExtractField::TimezoneHour | ExtractField::TimezoneMinute => (zoned, 0),
        };
        if !valid {
            return Err(rule(format!("[ISO/IEC 9075:1992 §6.6 SR3] {t} has no field {}", extract_name(field))));
        }
        let f = Extract { field, scale, digits: SMALLINT_PRECISION };
        let result = f.return_type(std::slice::from_ref(&t))?;
        Ok((Expr::Call(ScalarRef(Arc::new(f)), vec![shown(e, t, self.now)]), result))
    }

    /// [ISO/IEC 9075:1992 §6.10] CAST of `e`, of type `from`, to `to`, one
    /// of them a datetime or interval type: of the session's displacement and date.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if CAST takes no value of `from` to `to` [SR4].
    pub(super) fn datetime_cast(&self, e: Expr, from: &DataType, to: DataType) -> Result<Expr> {
        if !castable(from, &to) {
            return Err(SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075:1992 §6.10 SR4] CAST of {from} to {to} is not valid")));
        }
        // [§6.10 GR11c] Only a time CAST to a timestamp, or a string, may
        // read the current date; any other the displacement alone, which
        // what keeps a plan compares, so its instant is not held.
        let dated = matches!(from, DataType::Date | DataType::Timestamp { .. });
        let now = if dated { Now { utc: 0, zone: self.now.zone() } } else { self.now.instant() };
        Ok(Expr::Call(ScalarRef(Arc::new(CastAt { to, now })), vec![e]))
    }
}

/// [ISO/IEC 9075:1992 §6.8] CURRENT_DATE, CURRENT_TIME, CURRENT_TIMESTAMP,
/// LOCALTIME or LOCALTIMESTAMP of a statement kept with its instant a
/// slot: the value of `moment` and `precision` of the instant its one
/// argument gives, a TIMESTAMP WITH TIME ZONE, of type `to` (RULES.md D613).
struct Current {
    moment: Moment,
    precision: u32,
    local: bool,
    to: DataType,
}

impl ScalarFunction for Current {
    fn name(&self) -> &'static str {
        "CURRENT_TIMESTAMP"
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(self.to.clone())
    }
    fn raise_free(&self) -> bool {
        true
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(Value::Datetime(instant)) = args.first() else {
            return Err(SqlError::new(Code::Api, "[DESIGN] the statement's instant is a timestamp"));
        };
        let now = Now { utc: instant.utc, zone: instant.zone };
        Ok(if self.local { current_local(self.moment, self.precision, now) } else { current(self.moment, self.precision, now) })
    }
}

/// The key word of `field`: its `<datetime field>`'s, or TIMEZONE_HOUR or
/// TIMEZONE_MINUTE.
fn extract_name(field: ExtractField) -> &'static str {
    match field {
        ExtractField::Datetime(f) => field_name(f),
        ExtractField::TimezoneHour => "TIMEZONE_HOUR",
        ExtractField::TimezoneMinute => "TIMEZONE_MINUTE",
    }
}
