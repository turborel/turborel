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
use crate::value::{DataType, Value, assign, compare};

/// [ISO/IEC 9075:1992 §11.5 SR3b] The least length of a column whose
/// default is CURRENT_USER, SESSION_USER or SYSTEM_USER.
const USER_LENGTH: u32 = 128;

/// [ISO/IEC 9075:1992 §11.5] The default value `option` puts in the
/// descriptor of a column of type `subject` [SR1, GR1].
///
/// # Errors
///
/// SQLCODE -103 if the option does not suit the type [SR3].
pub(super) fn default_of(subject: DataType, option: &DefaultOption) -> Result<ColumnDefault> {
    let literal = match option {
        DefaultOption::Null => return Ok(ColumnDefault::Value(Value::Null)),
        DefaultOption::User(_) => {
            let long = if let DataType::Character { length } = subject { length >= USER_LENGTH } else { false };
            if !long {
                return Err(rule(format!(
                    "[ISO/IEC 9075:1992 §11.5 SR3b] an authorization identifier is the default only of a character string of at least {USER_LENGTH} characters, not {subject}"
                )));
            }
            return Ok(ColumnDefault::User);
        }
        DefaultOption::Literal(l) => l,
    };
    let value = match (literal, subject) {
        (Literal::Character(s), DataType::Character { length }) => {
            // [SR3a i] no longer than the column.
            if s.chars().count() > usize::try_from(length).unwrap_or(usize::MAX) {
                return Err(rule(format!("[ISO/IEC 9075:1992 §11.5 SR3a i] the default '{s}' is longer than {subject}")));
            }
            Value::Char(s.clone())
        }
        // [SR3a iii] an exact number the type holds without losing a significant digit.
        (Literal::Exact(d), DataType::Exact { .. }) => {
            let exact = Value::Exact(*d);
            let kept = assign(exact.clone(), &subject, "ISO/IEC 9075:1992 §11.5 SR3a iii")
                .ok()
                .filter(|v| compare(v, &exact) == Some(std::cmp::Ordering::Equal));
            kept.ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.5 SR3a iii] the default {d} loses digits in {subject}")))?
        }
        // [SR3a iv] any number.
        (Literal::Exact(d), DataType::Approximate { .. }) => Value::Exact(*d),
        (Literal::Approximate(f), DataType::Approximate { .. }) => Value::Approx(*f),
        // \[EXTENSION\] Of a type beyond the standard, a character string
        // literal, as CAST to it makes it (RULES.md D53).
        (Literal::Character(s), DataType::Extension(e)) => e.kind.cast(Value::Char(s.clone()), &subject)?,
        (Literal::Exact(_) | Literal::Approximate(_), DataType::Extension(_))
        | (Literal::Character(_), DataType::Exact { .. } | DataType::Approximate { .. })
        | (Literal::Exact(_) | Literal::Approximate(_), DataType::Character { .. })
        | (Literal::Approximate(_), DataType::Exact { .. }) => {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.5 SR3a] the default is not a <literal> of {subject}")));
        }
    };
    // [GR1b] its value, a fixed-length string padded.
    Ok(ColumnDefault::Value(assign(value, &subject, "ISO/IEC 9075:1992 §11.5 GR1b")?))
}

/// [ISO/IEC 9075:1992 §11.5 GR2] The default value of each column of
/// `desc`, for a statement run under `auth`, as store assignment gives it
/// to the column [§9.2].
///
/// # Errors
///
/// SQLSTATE 22001 if the authorization identifier is longer than a column
/// whose default it is.
pub(super) fn default_row(desc: &TableDesc, auth: &str) -> Result<Row> {
    desc.columns.iter().map(|c| default_value(c, auth)).collect()
}

/// The default value of column `c`, as `default_row` gives it.
///
/// # Errors
///
/// As `default_row`.
pub(super) fn default_value(c: &ColumnDesc, auth: &str) -> Result<Value> {
    assign(c.default_value(auth), &c.data_type, "ISO/IEC 9075:1992 §11.5 GR2")
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
    pub(super) fn alter_column(&mut self, qn: &QualifiedName, column: &str, option: Option<&DefaultOption>) -> Result<()> {
        let mut desc = self.base_table(qn)?;
        let c = desc
            .columns
            .iter_mut()
            .find(|c| c.name == column)
            .ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.12 SR3] {qn} has no column {column}")))?;
        c.default = match (option, &c.default) {
            (Some(o), _) => Some(default_of(c.data_type, o)?),
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
