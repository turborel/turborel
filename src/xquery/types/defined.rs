//! The schema types a schema import brings into the in-scope schema types
//! [XQuery §4.10]: each type definition of a registered XML Schema
//! imported, by its name, and the schemas imported, against which a
//! node's type annotation is resolved [XQueryDM §3.3.1.1] and its
//! derivation checked by the schema's own Type Derivation OK [Schema1
//! §3.4.6, §3.14.6].

use std::fmt;
use std::sync::Arc;

use super::SchemaType;
use crate::error::Result;
use crate::xml::schema::{ComplexId, Derivations, Schema, Simple, TypeDef, Variety, annotated_type, built_in_type, type_derives};
use crate::xml::{Atomic, AtomicType, QName, SimpleType, xquery_error};

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
    #[must_use]
    pub fn atomic(&self) -> bool {
        self.datatype().is_some_and(|t| matches!(t.variety(), Variety::Atomic(_)))
    }

    /// The datatype it defines, of a simple type definition: a built-in
    /// atomic type's, or the one its schema made.
    fn datatype(&self) -> Option<Arc<SimpleType>> {
        match self.def {
            TypeDef::Simple(Simple::Atomic(t)) => Some(Arc::new(SimpleType::built_in(t))),
            TypeDef::Simple(Simple::Defined(id)) => self.schema.simple(id).map(|d| Arc::clone(&d.def)),
            TypeDef::Simple(Simple::AnySimple | Simple::List(_)) | TypeDef::Complex(_) => None,
        }
    }

    /// The built-in atomic type it restricts, if it is atomic.
    #[must_use]
    pub fn built_in(&self) -> Option<AtomicType> {
        self.datatype().and_then(|t| t.atomic())
    }

    /// `value` cast as it [XQuery §3.12.3] [XQueryFO §17.2]: a string or
    /// untyped value validated as a literal, another cast as its built-in
    /// type and checked against its facets, made a value of it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 10000: err:XPST0051 if it is no atomic type; as
    /// `SimpleType::cast`.
    pub fn cast(&self, value: &Atomic) -> Result<Atomic> {
        let Some(datatype) = self.datatype() else {
            return Err(xquery_error(&format!("{self} is no atomic type"), "[XQuery §2.5.4.2] err:XPST0051"));
        };
        datatype.cast(value)
    }
}

/// derives-from(the user-defined atomic type `name` of a value,
/// `expected`, a type of an imported schema) [XQuery §2.5.4.2]: the type
/// of that name in the schema `expected` is of, if it has one, derived
/// from `expected` by Type Derivation OK; else not, as the data model has
/// one type of a name [XQueryDM §2.6.1].
#[must_use]
pub fn value_derives(name: &QName, expected: &SchemaType) -> bool {
    let SchemaType::Defined(d) = expected else { return false };
    annotated_type(&d.schema, name)
        .is_some_and(|def| Defined { name: name.clone(), schema: Arc::clone(&d.schema), def }.derives_from(expected))
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
