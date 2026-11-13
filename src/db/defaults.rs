//! \[EXTENSION\] Column defaults [ISO/IEC 9075:1992 §11.5]: the default
//! value a `<default clause>` puts in a column descriptor, the rows of a
//! table filled with them, and ALTER COLUMN, which sets or drops one
//! [§11.12-§11.14], or alters an identity column or a generated column.

use std::sync::Arc;

use super::sequences::Epochs;
use super::{State, rule};
use crate::ast::{AlterColumnAction, DefaultOption, Literal};
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
pub(super) fn default_of(subject: &DataType, option: &DefaultOption, zone: i16) -> Result<ColumnDefault> {
    let literal = match option {
        DefaultOption::Null => return Ok(ColumnDefault::Value(Value::Null)),
        // [ISO/IEC 9075-2:2023 §11.5 SR3e, §6.5 GR1] of an array type, the
        // array of no element.
        DefaultOption::Empty => {
            if !matches!(subject, DataType::Array(_)) {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.5 SR3e] ARRAY[] is the default only of an array type, not {subject}")));
            }
            return Ok(ColumnDefault::Value(Value::Array(Arc::from(Vec::new()))));
        }
        DefaultOption::User(_) => {
            let long = if let DataType::Character { length, .. } | DataType::Varying { length, .. } = subject {
                *length >= USER_LENGTH
            } else {
                false
            };
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
            if !subject.is_datetime() || !t.comparable(subject) {
                return Err(rule(format!("[ISO/IEC 9075:1992 §11.5 SR3c] the default {t} is not a value of {subject}")));
            }
            let precision = if let DataType::Time { precision, .. } | DataType::Timestamp { precision, .. } = t { precision } else { 0 };
            return Ok(ColumnDefault::Datetime(f.moment, precision));
        }
        DefaultOption::Literal(l) => l,
    };
    // [GR1b] its value, a fixed-length string padded.
    Ok(ColumnDefault::Value(assign(literal_value(literal, &subject.clone(), zone)?, subject, "ISO/IEC 9075:1992 §11.5 GR1b")?))
}

/// [ISO/IEC 9075:1992 §11.5 SR3a] The value of `literal`, the default of a
/// column of `subject`, before it is assigned to it.
///
/// # Errors
///
/// SQLCODE -103 if it does not suit the type.
fn literal_value(literal: &Literal, subject: &DataType, zone: i16) -> Result<Value> {
    let not_of = || rule(format!("[ISO/IEC 9075:1992 §11.5 SR3a] the default is not a <literal> of {subject}"));
    match literal {
        Literal::Character(s) => character_default(s, &subject.clone()).unwrap_or_else(|| Err(not_of())),
        Literal::Exact(d) => match subject {
            DataType::Exact { .. } | DataType::Approximate { .. } | DataType::DecFloat { .. } => exact_default(*d, &subject.clone()),
            DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Binary { .. }
            | DataType::VarBinary { .. }
            | DataType::Boolean
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Interval(_)
            | DataType::Array(_)
            | DataType::Row(_)
            | DataType::Extension(_) => Err(not_of()),
        },
        // [ISO/IEC 9075-2:2023 §11.5 SR4a iv] of an approximate type;
        // \[EXTENSION\] of DECFLOAT too, of which SR4a says nothing.
        Literal::Approximate(f) => {
            matches!(subject, DataType::Approximate { .. } | DataType::DecFloat { .. }).then_some(Value::Approx(*f)).ok_or_else(not_of)
        }
        // [ISO/IEC 9075-2:2023 §11.5 SR4a vii] a <boolean literal> of a
        // Boolean column; UNKNOWN the null value [§4.6.1].
        Literal::Boolean(b) => (*subject == DataType::Boolean).then(|| b.map_or(Value::Null, Value::Bool)).ok_or_else(not_of),
        // [ISO/IEC 9075-2:2023 §11.5 SR4a ii] a binary string no longer
        // than the column; [GR2a iv, GR2a v] of a fixed length, extended
        // with X'00' octets, with no warning.
        Literal::Binary(b) => {
            let (DataType::Binary { length } | DataType::VarBinary { length }) = subject else { return Err(not_of()) };
            if b.len() > *length {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.5 SR4a ii] the default X'{b}' is longer than {subject}")));
            }
            subject.kind().cast(Value::Binary(b.clone()), subject)
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
            if !subject.is_datetime() || value.kind().is_none_or(|k| !k.comparable(subject)) {
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
fn exact_default(d: Decimal, subject: &DataType) -> Result<Value> {
    let exact = Value::Exact(d);
    if !subject.is_numeric() || matches!(subject, DataType::Approximate { .. } | DataType::DecFloat { .. }) {
        return Ok(exact);
    }
    let kept = assign(exact.clone(), subject, "ISO/IEC 9075:1992 §11.5 SR3a iii")
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
fn character_default(s: &str, subject: &DataType) -> Option<Result<Value>> {
    match subject {
        DataType::Character { length, units } | DataType::Varying { length, units } => {
            if units.count(s) > usize::try_from(*length).unwrap_or(usize::MAX) {
                return Some(Err(rule(format!("[ISO/IEC 9075:1992 §11.5 SR3a i] the default '{s}' is longer than {subject}"))));
            }
            Some(Ok(Value::Char(s.to_owned())))
        }
        DataType::Extension(e) => Some(e.kind.cast(Value::Char(s.to_owned()), subject)),
        DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Row(_) => None,
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
    /// [ISO/IEC 9075:1992 §11.12] Column `column` of base table `qn`,
    /// written at `at`, altered as `action` says: given a default [§11.13
    /// GR2], or its default dropped [§11.14 GR1], or altered as an identity
    /// column or a generated column is (`db/alter/identity.rs`), a
    /// restart's epoch from `epochs`. Its descriptor alone changes.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the table has no such column [§11.12 SR3], the
    /// default does not suit it [§11.5 SR3], it has none to drop [§11.14
    /// SR2], or the action is not of the column [ISO/IEC 9075-2:2023 §11.12
    /// SR6 to SR9].
    pub(super) fn alter_column(
        &mut self,
        (qn, zone): (&QualifiedName, i16),
        (column, at): (&str, usize),
        (action, epochs): (&AlterColumnAction, &mut Epochs),
    ) -> Result<()> {
        let mut desc = self.base_table(qn)?;
        let c = desc
            .columns
            .iter_mut()
            .find(|c| c.name == column)
            .ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.12 SR3] {qn} has no column {column}")))?;
        let defaulted = matches!(action, AlterColumnAction::SetDefault(_) | AlterColumnAction::DropDefault);
        // [ISO/IEC 9075-2:2023 §11.12 SR6] an identity column's identity alone.
        if defaulted && c.identity().is_some() {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.12 SR6] column {column} of {qn} is an identity column, so it has no default");
            return Err(rule(msg).or_at(at));
        }
        match action {
            AlterColumnAction::SetDefault(o) => {
                // [ISO/IEC 9075-2:2023 §11.12 SR9] no generated column's.
                if c.generation().is_some() {
                    let msg =
                        format!("[ISO/IEC 9075-2:2023 §11.12 SR9] column {column} of {qn} is a generated column, so it has no default");
                    return Err(rule(msg));
                }
                c.default = Some(default_of(&c.data_type.clone(), o, zone)?);
            }
            AlterColumnAction::DropDefault => {
                if c.default.is_none() {
                    return Err(rule(format!("[ISO/IEC 9075:1992 §11.14 SR2] column {column} of {qn} has no default to drop")));
                }
                c.default = None;
            }
            AlterColumnAction::Identity { .. } | AlterColumnAction::DropIdentity | AlterColumnAction::DropExpression => {
                self.alter_generation((qn, c), (action, at), epochs)?;
            }
        }
        let mut catalog = self.catalog.duplicate();
        catalog.replace_table(desc);
        self.catalog = Arc::from(catalog);
        Ok(())
    }
}
