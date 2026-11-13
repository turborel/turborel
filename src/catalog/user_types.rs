//! User-defined types, as the catalog describes them [ISO/IEC 9075-2:2023
//! §4.9]: distinct types alone, each with the two functions, casts and
//! transform its definition implies [§11.51 GR2] (RULES.md D69).

use super::QualifiedName;
use super::domains::{UsageDesc, usage_held};
use crate::value::DataType;

/// The suffix of the specific name of a distinct type's function from its
/// source type, after the type's name (RULES.md D69).
pub const TO_DISTINCT_SUFFIX: &str = "_TO_DISTINCT";
/// The suffix of the specific name of its function to its source type.
pub const TO_SOURCE_SUFFIX: &str = "_TO_SOURCE";

/// [ISO/IEC 9075-2:2023 §4.9.2] A distinct type's descriptor: the user-
/// defined type descriptor §11.51 GR1 makes, with the descriptors of the
/// routines GR2 makes, its casts and its transform.
#[derive(Debug, Clone, PartialEq)]
pub struct UserTypeDesc {
    /// The `<user-defined type name>`.
    pub name: QualifiedName,
    /// Its source type \[SR7a\]: a predefined type or an array type.
    pub source: DataType,
    /// The function from its source type, FNUDT \[SR7d\], which is its cast
    /// to it and its to-sql transform function.
    pub to_distinct: RoutineDesc,
    /// The function to its source type, FNSDT \[SR7e\], which is its cast to
    /// its source, its from-sql transform function and its ordering
    /// function.
    pub to_source: RoutineDesc,
    /// USAGE on it [§12.3, §11.51 GR1]: each grant.
    pub usage: Vec<UsageDesc>,
}

/// [ISO/IEC 9075-2:2023 §4.35] A routine a distinct type's definition
/// makes, as its descriptor holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutineDesc {
    /// The `<routine name>`'s identifier, in the type's schema.
    pub name: String,
    /// Its `<specific name>`'s identifier, in the type's schema
    /// (`TO_DISTINCT_SUFFIX`, `TO_SOURCE_SUFFIX`).
    pub specific: String,
    /// EXECUTE on it [§12.3, §11.60 GR]: each grant, as USAGE's are kept.
    pub execute: Vec<UsageDesc>,
}

/// Which of a distinct type's routines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Routine {
    /// FNUDT, from the source type.
    ToDistinct,
    /// FNSDT, to the source type.
    ToSource,
}

/// [ISO/IEC 9075-2:2023 §4.2] Whether `t` is based on the user-defined
/// type `name`: is it, or an array or row type, or a distinct type's
/// source, based on it. Each recursion is into a type `t` holds, so it
/// is bounded by how deep types nest (`types::MAX_TYPE_NESTING`).
pub fn based_on(t: &DataType, name: &QualifiedName) -> bool {
    match t {
        DataType::Distinct(d) => (d.schema == name.auth && d.name == name.ident) || based_on(&d.source, name),
        DataType::Array(a) => based_on(&a.element, name),
        DataType::Row(r) => r.fields.iter().any(|f| based_on(&f.data_type, name)),
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Extension(_) => false,
    }
}

/// The user-defined types `t` is based on, each once, in the order met.
pub fn types_in(t: &DataType, into: &mut Vec<QualifiedName>) {
    match t {
        DataType::Distinct(d) => {
            let name = QualifiedName { auth: d.schema.clone(), ident: d.name.clone() };
            if !into.contains(&name) {
                into.push(name);
            }
            types_in(&d.source, into);
        }
        DataType::Array(a) => types_in(&a.element, into),
        DataType::Row(r) => r.fields.iter().for_each(|f| types_in(&f.data_type, into)),
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Extension(_) => {}
    }
}

impl UserTypeDesc {
    /// The distinct type it describes.
    pub fn data_type(&self) -> DataType {
        DataType::distinct(&self.name.auth, &self.name.ident, self.source.clone())
    }

    /// Whether `auth` holds USAGE on it, and whether grantably, as of a
    /// domain (`DomainDesc::usage_of`).
    pub fn usage_of(&self, auth: &str, owner: &str) -> (bool, bool) {
        usage_held(&self.usage, auth, owner)
    }

    /// The routine `which`.
    pub fn routine(&self, which: Routine) -> &RoutineDesc {
        match which {
            Routine::ToDistinct => &self.to_distinct,
            Routine::ToSource => &self.to_source,
        }
    }

    /// The routine `which`, to change.
    pub fn routine_mut(&mut self, which: Routine) -> &mut RoutineDesc {
        match which {
            Routine::ToDistinct => &mut self.to_distinct,
            Routine::ToSource => &mut self.to_source,
        }
    }

    /// Whether `auth` holds EXECUTE on routine `which`, and whether
    /// grantably: its schema's owner `owner` grantably [§11.60 GR].
    pub fn execute_of(&self, which: Routine, auth: &str, owner: &str) -> (bool, bool) {
        usage_held(&self.routine(which).execute, auth, owner)
    }
}
