//! The schema types a schema import brings into the in-scope schema types
//! [XQuery §4.10]: each type definition of a registered XML Schema
//! imported, by its name, and the schemas imported, against which a
//! node's type annotation is resolved [XQueryDM §3.3.1.1] and its
//! derivation checked by the schema's own Type Derivation OK [Schema1
//! §3.4.6, §3.14.6].

use std::fmt;
use std::sync::Arc;

use super::SchemaType;
use crate::xml::QName;
use crate::xml::schema::{ComplexId, Derivations, Schema, Simple, TypeDef, Variety, annotated_type, built_in_type, type_derives};

/// A type definition of an imported schema [XQuery §4.10].
#[derive(Clone)]
pub struct Defined {
    /// Its name.
    pub name: QName,
    /// The schema imported.
    pub schema: Arc<Schema>,
    /// The definition.
    pub def: TypeDef,
}

impl PartialEq for Defined {
    /// Of one name, of one schema.
    fn eq(&self, other: &Defined) -> bool {
        self.name == other.name && Arc::ptr_eq(&self.schema, &other.schema)
    }
}

impl Eq for Defined {}

impl fmt::Debug for Defined {
    /// Its name.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Defined({self})")
    }
}

impl fmt::Display for Defined {
    /// Its name, its namespace in braces before it if it has one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name.uri {
            Some(uri) => write!(f, "{{{uri}}}{}", self.name.local),
            None => write!(f, "{}", self.name.local),
        }
    }
}

impl Defined {
    /// derives-from(it, `expected`) [XQuery §2.5.4]: Type Derivation OK
    /// [Schema1 §3.4.6, §3.14.6] in its schema, of no method blocked; of a
    /// type of another schema, whether it has that type's name.
    #[must_use]
    pub fn derives_from(&self, expected: &SchemaType) -> bool {
        let target = match expected {
            SchemaType::Defined(e) => {
                if !Arc::ptr_eq(&e.schema, &self.schema) {
                    return e.name == self.name;
                }
                e.def
            }
            SchemaType::AnyType => TypeDef::Complex(ComplexId::ANY_TYPE),
            SchemaType::AnySimpleType => TypeDef::Simple(Simple::AnySimple),
            SchemaType::Atomic(t) => TypeDef::Simple(Simple::Atomic(*t)),
            SchemaType::List(local) => match built_in_type(local) {
                Some(t) => t,
                None => return false,
            },
            SchemaType::AnyAtomicType => return self.atomic(),
            SchemaType::Untyped => return false,
        };
        type_derives(&self.schema, self.def, target, Derivations::NONE)
    }

    /// Whether it is a simple type of the atomic variety, so derived from
    /// `xdt:anyAtomicType` [XQueryFO §1.6].
    fn atomic(&self) -> bool {
        match self.def {
            TypeDef::Simple(Simple::Atomic(_)) => true,
            TypeDef::Simple(Simple::Defined(id)) => self.schema.simple(id).is_some_and(|d| matches!(d.def.variety(), Variety::Atomic(_))),
            TypeDef::Simple(Simple::AnySimple | Simple::List(_)) | TypeDef::Complex(_) => false,
        }
    }
}

/// The schemas a query imports [XQuery §4.10], against which the type
/// annotation of a node is resolved; none of a query that imports none.
#[derive(Debug, Clone, Default)]
pub struct Imports(pub Arc<[Arc<Schema>]>);

impl PartialEq for Imports {
    /// Of the same schemas.
    fn eq(&self, other: &Imports) -> bool {
        self.0.len() == other.0.len() && self.0.iter().zip(other.0.iter()).all(|(a, b)| Arc::ptr_eq(a, b))
    }
}

impl Eq for Imports {}

/// derives-from(the type annotation `name` of a node, `expected`) [XQuery
/// §2.5.4], `name` naming no built-in type: the type resolved in the
/// schema of `expected`, if it is one imported, else in the first of
/// `imports` that has it; if none has it, derived from `xs:anyType`
/// alone.
#[must_use]
pub fn annotation_derives(name: &QName, expected: &SchemaType, imports: &Imports) -> bool {
    let own = match expected {
        SchemaType::Defined(d) => Some(&d.schema),
        SchemaType::AnyType
        | SchemaType::Untyped
        | SchemaType::AnySimpleType
        | SchemaType::AnyAtomicType
        | SchemaType::Atomic(_)
        | SchemaType::List(_) => None,
    };
    let found = own.into_iter().chain(imports.0.iter()).find_map(|s| annotated_type(s, name).map(|def| (s, def)));
    match found {
        Some((schema, def)) => Defined { name: name.clone(), schema: Arc::clone(schema), def }.derives_from(expected),
        None => *expected == SchemaType::AnyType,
    }
}
