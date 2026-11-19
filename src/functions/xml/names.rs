//! \[EXTENSION\] The functions by which an application invokes the mappings
//! of names [ISO/IEC 9075-14:2006 draft §9.1, §9.3], Feature X400 (RULES.md
//! D106): XMLNAME_TO_IDENTIFIER, an XML Name mapped to the delimited
//! identifier it stands for [§9.3], and IDENTIFIER_TO_XMLNAME, an SQL
//! identifier mapped to an XML Name, partially escaped or fully [§9.1].
//! The draft gives the mappings no syntax [§4.10.3 NOTE 10, §4.10.4
//! NOTE 11]; turborel's is written in RULES.md D106.
//!
//! Each takes a character string and gives a character varying string
//! as long as the mapping of its longest value can be, at most
//! `MAX_CHARACTER_LENGTH`; a value whose mapping is longer is SQLSTATE
//! 22001. The null value gives the null value. No other error arises: the
//! draft's one, an XML Name no character of SQL_TEXT holds [§9.3 GR4], is
//! never raised, as SQL_TEXT is Unicode.

use std::sync::Arc;

use super::super::strings::{character, text};
use super::ScalarFunction;
use crate::error::{Code, Result, SqlError, SqlState, fail};
use crate::params::ParameterType;
use crate::value::{CharUnits, DataType, MAX_CHARACTER_LENGTH, Value};
use crate::xml::{Escape, delimited, identifier_of, name_of};

/// The name XMLNAME_TO_IDENTIFIER is registered under.
pub const XMLNAME_TO_IDENTIFIER: &str = "XMLNAME_TO_IDENTIFIER";
/// The name IDENTIFIER_TO_XMLNAME, partially escaped, is registered under.
pub const IDENTIFIER_TO_XMLNAME: &str = "IDENTIFIER_TO_XMLNAME";
/// The name IDENTIFIER_TO_XMLNAME, fully escaped, is registered under:
/// what FULLY ESCAPED reads, no identifier one writes.
pub const IDENTIFIER_TO_XMLNAME_FULLY: &str = "IDENTIFIER_TO_XMLNAME FULLY ESCAPED";

/// [§9.1 GR4e ii] The most characters one character of an identifier maps
/// to: `_x`, six hexits and `_`.
const MOST_ESCAPED: u32 = 9;
/// [§9.3 GR5] The double quotes around a delimited identifier.
const QUOTES: u32 = 2;
/// [ISO/IEC 9075:1992 §5.2] A double quote of a delimited identifier is
/// written twice.
const DOUBLED: u32 = 2;

/// The two functions, to register.
pub fn functions() -> Vec<Arc<dyn ScalarFunction>> {
    vec![Arc::new(NameToIdentifier), Arc::new(IdentifierToName(Escape::Partial)), Arc::new(IdentifierToName(Escape::Full))]
}

/// The type of a mapping of a string of type `t` whose characters each
/// map to at most `per` characters, `added` more beside them.
fn mapped(name: &str, t: &DataType, per: u32, added: u32) -> Result<DataType> {
    let length = character(name, t)?.saturating_mul(per).saturating_add(added);
    Ok(DataType::Varying { length: length.min(MAX_CHARACTER_LENGTH), units: CharUnits::Characters })
}

/// `mapped`, a character string the function `name` gave, as a value: at
/// most `MAX_CHARACTER_LENGTH` characters.
///
/// # Errors
///
/// SQLSTATE 22001 if it is longer.
fn within(name: &str, mapped: String) -> Result<Value> {
    let n = mapped.chars().count();
    if u32::try_from(n).map_or(true, |n| n > MAX_CHARACTER_LENGTH) {
        let why = format!("[EXTENSION] {name} gives {n} characters, more than CHARACTER VARYING({MAX_CHARACTER_LENGTH}) holds");
        return Err(SqlError::new(Code::Data, why).with_state(SqlState::STRING_DATA_RIGHT_TRUNCATION));
    }
    Ok(Value::Char(mapped))
}

/// The one argument of `args`, of the function `name`.
fn one<'a>(name: &str, args: &'a [Value]) -> Result<&'a Value> {
    match args {
        [only] => Ok(only),
        _ => fail(Code::Api, format!("[DESIGN] {name} takes one string")),
    }
}

/// XMLNAME_TO_IDENTIFIER: [§9.3] the delimited identifier an XML Name
/// maps to [GR5], each escape of an assigned character read as it.
#[derive(Debug)]
pub struct NameToIdentifier;

impl ScalarFunction for NameToIdentifier {
    fn name(&self) -> &'static str {
        XMLNAME_TO_IDENTIFIER
    }
    /// Escapes only shorten a name, and each double quote is doubled, in
    /// double quotes.
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [t] = args else { return fail(Code::Api, "[DESIGN] XMLNAME_TO_IDENTIFIER takes one string") };
        mapped(self.name(), t, DOUBLED, QUOTES)
    }
    fn parameter_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        Some(ParameterType::Of(DataType::Varying { length: MAX_CHARACTER_LENGTH, units: CharUnits::Characters }))
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(name) = text(one(self.name(), args)?)? else { return Ok(Value::Null) };
        within(self.name(), delimited(&identifier_of(name)))
    }
}

/// IDENTIFIER_TO_XMLNAME: [§9.1] the XML Name an SQL identifier, its
/// characters, maps to by the escape variant it holds.
#[derive(Debug)]
pub struct IdentifierToName(pub Escape);

impl ScalarFunction for IdentifierToName {
    fn name(&self) -> &'static str {
        match self.0 {
            Escape::Partial => IDENTIFIER_TO_XMLNAME,
            Escape::Full => IDENTIFIER_TO_XMLNAME_FULLY,
        }
    }
    /// Each character maps to at most `MOST_ESCAPED`.
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [t] = args else { return fail(Code::Api, "[DESIGN] IDENTIFIER_TO_XMLNAME takes one string") };
        mapped(IDENTIFIER_TO_XMLNAME, t, MOST_ESCAPED, 0)
    }
    fn parameter_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        Some(ParameterType::Of(DataType::Varying { length: MAX_CHARACTER_LENGTH, units: CharUnits::Characters }))
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(identifier) = text(one(IDENTIFIER_TO_XMLNAME, args)?)? else { return Ok(Value::Null) };
        within(IDENTIFIER_TO_XMLNAME, name_of(identifier, self.0))
    }
}

#[cfg(test)]
mod tests;
