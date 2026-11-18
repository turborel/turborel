//! [ISO/IEC 9075-2:2023 §6.17, §8.19] TREAT and the `<type predicate>` of
//! a value of a family of types built in (`value/types/family.rs`,
//! RULES.md D-SPATIAL-FAMILY): of its most specific type, which its
//! family reads of it.

use super::ScalarFunction;
use crate::error::{Code, Result, SqlState, fail_with};
use crate::value::types::Kind;
use crate::value::types::family::Family;
use crate::value::{DataType, Value};

/// Whether the most specific type of `v`, of `family`, is one of `types`.
fn of_types(family: &dyn Family, v: &Value, types: &[Kind]) -> Option<bool> {
    let most = family.most_specific(v)?;
    Some(types.iter().any(|t| t.name() == most.name()))
}

/// [ISO/IEC 9075-2:2023 §6.17 GR1] TREAT of a value of a family as a
/// subtype: the value itself if its most specific type is the target or a
/// subtype of it, null of the null value; else invalid target type
/// specification.
#[derive(Debug)]
pub struct FamilyTreat {
    /// The family.
    pub family: &'static dyn Family,
    /// The target type and each of its proper subtypes.
    pub types: Vec<Kind>,
    /// The target type, the result's.
    pub result: DataType,
}

impl ScalarFunction for FamilyTreat {
    fn name(&self) -> &'static str {
        "TREAT"
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(v) = args.first().filter(|v| !v.is_null()) else { return Ok(Value::Null) };
        if of_types(self.family, v, &self.types) == Some(true) {
            return Ok(v.clone());
        }
        let most = self.family.most_specific(v).map_or("no type", |k| k.name());
        let msg = format!("[ISO/IEC 9075-2:2023 §6.17 GR1b] a value of most specific type {most} is no value of {}", self.result);
        fail_with(Code::Data, SqlState::INVALID_TARGET_TYPE, msg)
    }
    fn deterministic(&self) -> bool {
        true
    }
}

/// [ISO/IEC 9075-2:2023 §8.19 GR1, GR2] Whether a value of a family is of
/// one of `types`: null of the null value.
#[derive(Debug)]
pub struct FamilyTypeTest {
    /// The family.
    pub family: &'static dyn Family,
    /// The types a value of which is in the list.
    pub types: Vec<Kind>,
}

impl ScalarFunction for FamilyTypeTest {
    fn name(&self) -> &'static str {
        "IS OF"
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(v) = args.first().filter(|v| !v.is_null()) else { return Ok(Value::Null) };
        Ok(of_types(self.family, v, &self.types).map_or(Value::Null, Value::Bool))
    }
    fn deterministic(&self) -> bool {
        true
    }
}

/// [ISO/IEC 9075-2:2023 §6.33 GR15] SPECIFICTYPE of a value of a family:
/// the name of its most specific type, qualified by the catalog's name and
/// INFORMATION_SCHEMA, the schema of the types built in (RULES.md
/// D-SPATIAL-FAMILY); null of the null value.
#[derive(Debug)]
pub struct FamilySpecificType {
    /// The family.
    pub family: &'static dyn Family,
    /// The catalog's name.
    pub catalog: String,
}

impl ScalarFunction for FamilySpecificType {
    fn name(&self) -> &'static str {
        super::SPECIFICTYPE
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Varying { length: super::SPECIFICTYPE_LENGTH, units: crate::value::CharUnits::Characters })
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(v) = args.first().filter(|v| !v.is_null()) else { return Ok(Value::Null) };
        let most =
            self.family.most_specific(v).ok_or_else(|| crate::error::SqlError::new(Code::Corrupt, "a value of no type of its family"))?;
        let quoted = |part: &str| format!("\"{}\"", part.replace('"', "\"\""));
        Ok(Value::Char(format!("{}.{}.{}", quoted(&self.catalog), quoted(crate::system::INFORMATION_SCHEMA), quoted(most.name()))))
    }
    fn deterministic(&self) -> bool {
        true
    }
}

/// The kind `top` and each kind of `family` below it.
pub fn below(family: &dyn Family, top: &'static dyn crate::value::SqlType) -> Vec<Kind> {
    family.kinds().iter().copied().filter(|k| family.is_subtype(**k, top)).collect()
}
