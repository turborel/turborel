//! \[EXTENSION\] Column defaults [ISO/IEC 9075:1992 §11.5]: the default
//! value a `<default clause>` puts in a column descriptor, the rows of a
//! table filled with them, and ALTER COLUMN, which sets or drops one
//! [§11.12-§11.14].

use std::sync::Arc;

use super::{State, rule};
use crate::ast::{DefaultOption, Literal};
use crate::catalog::{ColumnDefault, ColumnDesc, QualifiedName, TableDesc};
use crate::error::Result;
use crate::storage::Row;
use crate::value::datetime::Now;
use crate::value::{DataType, Decimal, Value, assign, compare};

/// [ISO/IEC 9075:1992 §11.5 SR3b] The least length of a column whose
/// default is CURRENT_USER, SESSION_USER or SYSTEM_USER.
const USER_LENGTH: u32 = 128;

/// [ISO/IEC 9075:1992 §11.5] The default value `option` puts in the
/// descriptor of a column of type `subject` [SR1, GR1].
///
/// # Errors
///
/// SQLCODE -103 if the option does not suit the type [SR3].
pub(super) fn default_of(subject: DataType, option: &DefaultOption, zone: i16) -> Result<ColumnDefault> {
    let literal = match option {
        DefaultOption::Null => return Ok(ColumnDefault::Value(Value::Null)),
        DefaultOption::User(_) => {
            let long =
                if let DataType::Character { length } | DataType::Varying { length } = subject { length >= USER_LENGTH } else { false };
            if !long {
                return Err(rule(format!(
                    "[ISO/IEC 9075:1992 §11.5 SR3b] an authorization identifier is the default only of a character string of at least {USER_LENGTH} characters, not {subject}"
                )));
            }
            return Ok(ColumnDefault::User);
        }
        // [SR3c] of the same fields as the column; [GR1d] taken as the
        // statement that takes it runs.
        DefaultOption::Datetime(f) => {
            let t = crate::binder::datetime_function_type(*f);
            if !subject.is_datetime() || !t.comparable(&subject) {
                return Err(rule(format!("[ISO/IEC 9075:1992 §11.5 SR3c] the default {t} is not a value of {subject}")));
            }
            let precision = if let DataType::Time { precision, .. } | DataType::Timestamp { precision, .. } = t { precision } else { 0 };
            return Ok(ColumnDefault::Datetime(f.moment, precision));
        }
        DefaultOption::Literal(l) => l,
    };
    // [GR1b] its value, a fixed-length string padded.
    Ok(ColumnDefault::Value(assign(literal_value(literal, subject, zone)?, &subject, "ISO/IEC 9075:1992 §11.5 GR1b")?))
}

/// [ISO/IEC 9075:1992 §11.5 SR3a] The value of `literal`, the default of a
/// column of `subject`, before it is assigned to it.
///
/// # Errors
///
/// SQLCODE -103 if it does not suit the type.
fn literal_value(literal: &Literal, subject: DataType, zone: i16) -> Result<Value> {
    let not_of = || rule(format!("[ISO/IEC 9075:1992 §11.5 SR3a] the default is not a <literal> of {subject}"));
    match literal {
        Literal::Character(s) => character_default(s, subject).unwrap_or_else(|| Err(not_of())),
        Literal::Exact(d) => match subject {
            DataType::Exact { .. } | DataType::Approximate { .. } => exact_default(*d, subject),
            DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Bit { .. }
            | DataType::BitVarying { .. }
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Interval(_)
            | DataType::Extension(_) => Err(not_of()),
        },
        Literal::Approximate(f) => {
            if let DataType::Approximate { .. } = subject {
                Ok(Value::Approx(*f))
            } else {
                Err(not_of())
            }
        }
        // [SR3a ii] a bit string no longer than the column; [GR1b iv, v]
        // of a fixed length, extended with 0 bits, the statement placing
        // it completing with the warning (`db/padding.rs`).
        Literal::Bit(b) => {
            let (DataType::Bit { length } | DataType::BitVarying { length }) = subject else { return Err(not_of()) };
            if b.len() > length {
                return Err(rule(format!("[ISO/IEC 9075:1992 §11.5 SR3a ii] the default B'{b}' is longer than {subject}")));
            }
            subject.kind().cast(Value::Bits(b.clone()), &subject)
        }
        // [SR3a vi] an interval of the column's class, in its fields.
        Literal::Interval(i) => {
            if !subject.kind().comparable(&DataType::Interval(i.qualifier)) || !subject.is_interval() {
                return Err(rule(format!(
                    "[ISO/IEC 9075:1992 §11.5 SR3a vi] the default {} is not of {subject}",
                    Value::Interval(*i).to_literal()
                )));
            }
            Ok(Value::Interval(*i))
        }
        // [SR3a v] a datetime of the same fields; of the defining session's
        // displacement, if it writes none [§5.3 SR18].
        Literal::Datetime(w) => {
            let value = crate::binder::datetime_literal(w, zone);
            if !subject.is_datetime() || value.kind().is_none_or(|k| !k.comparable(&subject)) {
                return Err(rule(format!(
                    "[ISO/IEC 9075:1992 §11.5 SR3a v] the default {} is not of the fields of {subject}",
                    value.to_literal()
                )));
            }
            Ok(value)
        }
    }
}

/// [ISO/IEC 9075:1992 §11.5 SR3a iii, iv] The value of the exact numeric
/// literal `d` as the default of a column of `subject`, a numeric type: of
/// an exact type, if it holds it without losing a significant digit; of
/// an approximate type, any number.
///
/// # Errors
///
/// SQLCODE -103 if an exact type would lose a digit.
fn exact_default(d: Decimal, subject: DataType) -> Result<Value> {
    let exact = Value::Exact(d);
    if !subject.is_numeric() || matches!(subject, DataType::Approximate { .. }) {
        return Ok(exact);
    }
    let kept = assign(exact.clone(), &subject, "ISO/IEC 9075:1992 §11.5 SR3a iii")
        .ok()
        .filter(|v| compare(v, &exact) == Some(std::cmp::Ordering::Equal));
    kept.ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.5 SR3a iii] the default {d} loses digits in {subject}")))
}

/// [ISO/IEC 9075:1992 §11.5 SR3a i] The value of the character string
/// literal `s` as the default of a column of `subject`: of a character
/// string type, no longer than it; \[EXTENSION\] of a type beyond the
/// standard, as CAST to it makes it (RULES.md D53); none of another type.
///
/// # Errors
///
/// SQLCODE -103 if it is longer than the column; CAST's errors.
fn character_default(s: &str, subject: DataType) -> Option<Result<Value>> {
    match subject {
        DataType::Character { length } | DataType::Varying { length } => {
            if s.chars().count() > usize::try_from(length).unwrap_or(usize::MAX) {
                return Some(Err(rule(format!("[ISO/IEC 9075:1992 §11.5 SR3a i] the default '{s}' is longer than {subject}"))));
            }
            Some(Ok(Value::Char(s.to_owned())))
        }
        DataType::Extension(e) => Some(e.kind.cast(Value::Char(s.to_owned()), &subject)),
        DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::Bit { .. }
        | DataType::BitVarying { .. }
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_) => None,
    }
}

/// [ISO/IEC 9075:1992 §11.5 GR2] The default value of each column of
/// `desc`, for a statement run under `auth`, as store assignment gives it
/// to the column [§9.2].
///
/// # Errors
///
/// SQLSTATE 22001 if the authorization identifier is longer than a column
/// whose default it is.
pub(super) fn default_row(desc: &TableDesc, auth: &str, now: Now) -> Result<Row> {
    desc.columns.iter().map(|c| default_value(c, auth, now)).collect()
}

/// The default value of column `c`, as `default_row` gives it.
///
/// # Errors
///
/// As `default_row`.
pub(super) fn default_value(c: &ColumnDesc, auth: &str, now: Now) -> Result<Value> {
    assign(c.default_value(auth, now.into()), &c.data_type, "ISO/IEC 9075:1992 §11.5 GR2")
}

impl State {
    /// [ISO/IEC 9075:1992 §11.12] Column `column` of base table `qn` given
    /// the default `option` [§11.13 GR2], or, with none, its default dropped
    /// [§11.14 GR1]: its descriptor alone changes.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the table has no such column [§11.12 SR3], the
    /// default does not suit it [§11.5 SR3], or it has none to drop
    /// [§11.14 SR2].
    pub(super) fn alter_column(&mut self, (qn, zone): (&QualifiedName, i16), column: &str, option: Option<&DefaultOption>) -> Result<()> {
        let mut desc = self.base_table(qn)?;
        let c = desc
            .columns
            .iter_mut()
            .find(|c| c.name == column)
            .ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.12 SR3] {qn} has no column {column}")))?;
        c.default = match (option, &c.default) {
            (Some(o), _) => Some(default_of(c.data_type, o, zone)?),
            (None, Some(_)) => None,
            (None, None) => {
                return Err(rule(format!("[ISO/IEC 9075:1992 §11.14 SR2] column {column} of {qn} has no default to drop")));
            }
        };
        let mut catalog = self.catalog.duplicate();
        catalog.replace_table(desc);
        self.catalog = Arc::from(catalog);
        Ok(())
    }
}
