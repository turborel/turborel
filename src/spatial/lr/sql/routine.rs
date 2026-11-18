//! A routine of §15 as SQL invokes it: its name, the types of its
//! parameters (SELF first of a method), its result, its null rule and
//! whether it reads the catalogs (`sql/catalog.rs`).

use std::borrow::Borrow;

use super::CONSTRAINT_LENGTH;
use super::args::{Args, MAX_ARRAY, P};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::Value;

/// A routine of §15.
#[derive(Debug, Clone, Copy)]
pub struct Routine {
    /// Its name, as an identifier folds to upper case.
    pub name: &'static str,
    /// Its parameters, SELF first of a method.
    pub params: &'static [P],
    /// Its result.
    pub result: P,
    /// RETURNS NULL ON NULL INPUT, as it declares it.
    pub null_call: bool,
    /// Whether it reads the LRMs and linear elements of the catalogs.
    pub catalog: bool,
    /// What it does, of its arguments.
    pub run: fn(&Args<'_>) -> Result<Value>,
}

/// A routine RETURNS NULL ON NULL INPUT.
pub const fn null_call(name: &'static str, params: &'static [P], result: P, run: fn(&Args<'_>) -> Result<Value>) -> Routine {
    Routine { name, params, result, null_call: true, catalog: false, run }
}

/// A routine CALLED ON NULL INPUT.
pub const fn called(name: &'static str, params: &'static [P], result: P, run: fn(&Args<'_>) -> Result<Value>) -> Routine {
    Routine { name, params, result, null_call: false, catalog: false, run }
}

impl Routine {
    /// It, reading the catalogs.
    #[must_use]
    pub const fn reading(self) -> Routine {
        Routine { catalog: true, ..self }
    }

    /// Whether it takes arguments of the declared types `types`.
    pub fn takes(&self, types: &[crate::value::DataType]) -> bool {
        self.params.len() == types.len() && self.params.iter().zip(types).all(|(p, t)| p.takes(t))
    }

    /// Whether it may be invoked of the values `values`, the null value
    /// taken anywhere.
    pub fn holds<V: Borrow<Value>>(&self, values: &[V]) -> bool {
        self.params.len() == values.len() && self.params.iter().zip(values).all(|(p, v)| v.borrow().is_null() || p.holds(v.borrow()))
    }

    /// The arguments `values` as its parameters take them: a character
    /// string of more characters than its parameter's length, past trailing
    /// spaces, raises 22001, and an array of more elements than its
    /// parameter's cardinality raises 2202F, as storing it would
    /// [ISO/IEC 9075-2:2023 §9.2 GR3b ii, GR3b xxii 2B].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22001 or 2202F.
    pub fn check_lengths(&self, values: &[Value]) -> Result<()> {
        for (p, v) in self.params.iter().zip(values) {
            let Value::Char(text) = v else { continue };
            if let P::Text(n) = p {
                fits(text, *n)?;
            }
        }
        for (p, v) in self.params.iter().zip(values) {
            let Value::Array(items) = v else { continue };
            if matches!(p, P::TextArray | P::LrArray(_)) {
                cardinality(items.len())?;
            }
            if matches!(p, P::TextArray) {
                items
                    .iter()
                    .filter_map(|i| if let Value::Char(t) = i { Some(t) } else { None })
                    .try_for_each(|t| fits(t, CONSTRAINT_LENGTH))?;
            }
        }
        Ok(())
    }
}

/// 22001 if `text`, past trailing spaces, has more than `length`
/// characters.
fn fits(text: &str, length: u32) -> Result<()> {
    let count = text.trim_end_matches(' ').chars().count();
    if u32::try_from(count).is_ok_and(|c| c <= length) {
        return Ok(());
    }
    let msg = format!("[ISO/IEC 9075-2:2023 §9.2 GR3b ii] a value of {count} characters does not fit CHARACTER VARYING({length})");
    Err(SqlError::new(Code::Data, msg).with_state(SqlState::STRING_DATA_RIGHT_TRUNCATION))
}

/// 2202F if an array of `count` elements is past `MAX_ARRAY`.
fn cardinality(count: usize) -> Result<()> {
    if u32::try_from(count).is_ok_and(|c| c <= MAX_ARRAY) {
        return Ok(());
    }
    let msg = format!("[ISO/IEC 9075-2:2023 §9.2 GR3b xxii 2B] an array of {count} elements does not fit an array of {MAX_ARRAY}");
    Err(SqlError::new(Code::Data, msg).with_state(SqlState::ARRAY_DATA_RIGHT_TRUNCATION))
}
