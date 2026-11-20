//! [ISO/IEC 9075-2:2023 §4.9.3, §6.17, §6.33, §8.19] The functions the
//! binder makes of a structured type's definition and of the expressions
//! of its values: observers and mutators of its attributes, its
//! constructor function, TREAT, the `<type predicate>`, SPECIFICTYPE, and
//! a value with its comparison key (RULES.md D81).
//!
//! Each is told at bind time what the catalog holds, the attribute's
//! position or the names of the types it tests, so none reads the catalog
//! as it runs.

use std::sync::Arc;

use super::ScalarFunction;
use crate::error::{Code, Result, SqlError, SqlState, fail, fail_with};
use crate::value::types::{Structured, UdtName};
use crate::value::{CharUnits, DataType, Value, assign};

/// [ISO/IEC 9075-2:2023 §6.33 SR19] (IL046) The most characters of the
/// result of SPECIFICTYPE: three identifiers of 128 characters, each
/// double quote doubled and each quoted, and the two periods between them.
pub const SPECIFICTYPE_LENGTH: u32 = 776;

/// The name a call of SPECIFICTYPE is written with.
pub const SPECIFICTYPE: &str = "SPECIFICTYPE";
/// The name a call of TREAT is written with.
const TREAT: &str = "TREAT";
/// The name a `<type predicate>` is shown with.
const TYPE_PREDICATE: &str = "IS OF";
/// The name a value given its comparison key is shown with.
const KEYED: &str = "KEYED";

/// The structured value `v` is, none if it is null.
fn structured<'v>(v: Option<&'v Value>, of: &str) -> Result<Option<&'v Structured>> {
    match v {
        Some(Value::Structured(s)) => Ok(Some(s)),
        None | Some(Value::Null) => Ok(None),
        Some(other) => fail(Code::Api, format!("[DESIGN] {of} was given {other}, not a value of a structured type")),
    }
}

/// [ISO/IEC 9075-2:2023 §4.9.3.2] The observer function of an attribute:
/// its value of an instance, null of the null value.
#[derive(Debug)]
pub struct Observer {
    /// The attribute's name.
    pub name: String,
    /// Its position among the type's attributes, inherited first.
    pub index: usize,
    /// Its declared type.
    pub result: DataType,
}

impl ScalarFunction for Observer {
    fn name(&self) -> &str {
        &self.name
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(s) = structured(args.first(), &self.name)? else { return Ok(Value::Null) };
        s.attributes.get(self.index).cloned().ok_or_else(|| missing(&self.name, s))
    }
}

/// An instance with no attribute at a position its type's definition has.
fn missing(attribute: &str, s: &Structured) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] an instance of {} has no attribute {attribute}", s.of))
}

/// [ISO/IEC 9075-2:2023 §4.9.3.3, §9.18 GR8a] The mutator function of an
/// attribute: a copy of the instance, the attribute set to the value
/// assigned to its type; of the null value, data exception, null value
/// substituted for mutator subject parameter.
#[derive(Debug)]
pub struct Mutator {
    /// The attribute's name.
    pub name: String,
    /// Its position among the type's attributes.
    pub index: usize,
    /// Its declared type.
    pub attribute: DataType,
    /// The declared type of the instance, the result's.
    pub result: DataType,
}

impl ScalarFunction for Mutator {
    fn name(&self) -> &str {
        &self.name
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(s) = structured(args.first(), &self.name)? else {
            let msg = format!("[ISO/IEC 9075-2:2023 §9.18 GR8a i 1] the mutator of attribute {} was given the null value", self.name);
            return fail_with(Code::Data, SqlState::NULL_MUTATOR_SUBJECT, msg);
        };
        let value = assign(args.get(1).cloned().unwrap_or(Value::Null), &self.attribute, "ISO/IEC 9075-2:2023 §4.9.3.3")?;
        let mut attributes = s.attributes.to_vec();
        let slot = attributes.get_mut(self.index).ok_or_else(|| missing(&self.name, s))?;
        *slot = value;
        Ok(Value::Structured(Arc::new(Structured::new(Arc::clone(&s.of), attributes.into()))))
    }
    fn deterministic(&self) -> bool {
        true
    }
}

/// [ISO/IEC 9075-2:2023 §11.51 GR3] The constructor function of a type:
/// an instance of it whose attributes are their defaults, the arguments
/// the binder gives it, each already of its attribute's type.
#[derive(Debug)]
pub struct Constructed {
    /// The type.
    pub of: Arc<UdtName>,
    /// Its declared type.
    pub result: DataType,
}

impl ScalarFunction for Constructed {
    fn name(&self) -> &str {
        &self.of.name
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        Ok(Value::Structured(Arc::new(Structured::new(Arc::clone(&self.of), args.to_vec().into()))))
    }
}

/// [ISO/IEC 9075-2:2023 §6.17 GR1] TREAT of a value as a subtype: the value
/// itself if its most specific type is one of `types`, the target and its
/// subtypes, null of the null value; else invalid target type
/// specification.
#[derive(Debug)]
pub struct Treat {
    /// The target type and each of its proper subtypes.
    pub types: Vec<UdtName>,
    /// The target type, the result's.
    pub result: DataType,
}

impl ScalarFunction for Treat {
    fn name(&self) -> &str {
        TREAT
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(s) = structured(args.first(), TREAT)? else { return Ok(Value::Null) };
        if self.types.iter().any(|t| *t == *s.of) {
            return Ok(Value::Structured(Arc::new(Structured::new(Arc::clone(&s.of), Arc::clone(&s.attributes)))));
        }
        let msg = format!("[ISO/IEC 9075-2:2023 §6.17 GR1b] a value of most specific type {} is no value of {}", s.of, self.result);
        fail_with(Code::Data, SqlState::INVALID_TARGET_TYPE, msg)
    }
    fn deterministic(&self) -> bool {
        true
    }
}

/// [ISO/IEC 9075-2:2023 §6.17 GR, Feature S162] TREAT of a reference to
/// a reference type of a subtype: the reference itself, if the row it
/// identifies is of a table whose type is the target's referenced type or
/// a subtype of it. Its arguments are the reference, then whether a row of
/// such a table has it, then whether a row of any other table of its
/// referenced type has it, each true or null.
///
/// \[DESIGN\] RULES.md D81 a reference's most specific type is the
/// reference type of the type of the table whose row it identifies. One
/// that identifies no row keeps its declared type, `same` if that is the
/// target.
#[derive(Debug)]
pub struct TreatReference {
    /// The target type, the result's.
    pub result: DataType,
    /// Whether the target type is the operand's declared type.
    pub same: bool,
}

impl ScalarFunction for TreatReference {
    fn name(&self) -> &str {
        TREAT
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [r, fits, other] = args else { return fail(Code::Api, "[DESIGN] TREAT of a reference takes three arguments") };
        if r.is_null() || !fits.is_null() || (other.is_null() && self.same) {
            return Ok(r.clone());
        }
        let msg = format!("[ISO/IEC 9075-2:2023 §6.17 GR2b i] the reference identifies no row of a table of {}", self.result);
        fail_with(Code::Data, SqlState::INVALID_TARGET_TYPE, msg)
    }
    fn deterministic(&self) -> bool {
        false
    }
}

/// [ISO/IEC 9075-2:2023 §8.19 GR1, GR2] Whether a value's most specific
/// type is one of `types`, the types its `<type list>` names and, of each
/// not written with ONLY, their subtypes: null of the null value.
#[derive(Debug)]
pub struct TypeTest {
    /// The types a value of which is in the list.
    pub types: Vec<UdtName>,
}

impl ScalarFunction for TypeTest {
    fn name(&self) -> &str {
        TYPE_PREDICATE
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(s) = structured(args.first(), TYPE_PREDICATE)? else { return Ok(Value::Null) };
        Ok(Value::Bool(self.types.iter().any(|t| *t == *s.of)))
    }
    fn deterministic(&self) -> bool {
        true
    }
}

/// [ISO/IEC 9075-2:2023 §6.33 GR15] SPECIFICTYPE: the name of a value's
/// most specific type, "catalog"."schema"."name", each part's double
/// quotes doubled; null of the null value.
#[derive(Debug)]
pub struct SpecificType {
    /// The catalog's name.
    pub catalog: String,
}

/// `part` within double quotes, each double quote of it doubled.
fn quoted(part: &str) -> String {
    format!("\"{}\"", part.replace('"', "\"\""))
}

impl ScalarFunction for SpecificType {
    fn name(&self) -> &str {
        SPECIFICTYPE
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Varying { length: SPECIFICTYPE_LENGTH, units: CharUnits::Characters })
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(s) = structured(args.first(), SPECIFICTYPE)? else { return Ok(Value::Null) };
        Ok(Value::Char(format!("{}.{}.{}", quoted(&self.catalog), quoted(&s.of.schema), quoted(&s.of.name))))
    }
    fn deterministic(&self) -> bool {
        true
    }
}

/// \[DESIGN\] A value of a structured type with its comparison key, the
/// second argument (RULES.md D81): the value its ordering compares, as
/// comparison, grouping and sorting see it; null of the null value.
#[derive(Debug)]
pub struct Keyed {
    /// The value's declared type.
    pub result: DataType,
}

impl ScalarFunction for Keyed {
    fn name(&self) -> &str {
        KEYED
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(s) = structured(args.first(), KEYED)? else { return Ok(Value::Null) };
        let key = args.get(1).cloned().unwrap_or(Value::Null);
        Ok(Value::Structured(Arc::new(Structured { key: Some(key), ..s.clone() })))
    }
    fn deterministic(&self) -> bool {
        true
    }
}

/// [ISO/IEC 9075-2:2023 §6.24 GR] The row of a typed table, its
/// attributes' columns in order, as an instance of the table's type: of a
/// null row, the null value.
#[derive(Debug)]
pub struct Instanced {
    /// The table's type, the instance's most specific type.
    pub of: Arc<UdtName>,
    /// The declared type of the result, the scope table's type.
    pub result: DataType,
}

impl ScalarFunction for Instanced {
    fn name(&self) -> &str {
        &self.of.name
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        match args.first() {
            Some(Value::Row(attributes)) => Ok(Value::Structured(Arc::new(Structured::new(Arc::clone(&self.of), Arc::clone(attributes))))),
            None | Some(Value::Null) => Ok(Value::Null),
            Some(other) => fail(Code::Api, format!("[DESIGN] an instance of {} was made of {other}, not a row", self.of)),
        }
    }
    fn deterministic(&self) -> bool {
        true
    }
}

// Unit tests: the types the functions report, and their refusals.
#[cfg(test)]
mod tests;
