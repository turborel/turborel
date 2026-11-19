//! Simple type definitions read from schema documents [Schema1 §3.14.1]
//! [Schema2 §4.1.1]: each one's properties as \[Schema1\] names them, its
//! values and facets the datatype it holds (`schema::SimpleType`).

use std::sync::Arc;

use super::{AnnotationRef, Derivations, Expanded, Schema, Simple};
use crate::xml::SimpleType;

/// A component's place in its schema's table of simple type definitions
/// read from its documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimpleId(pub usize);

/// A simple type definition of a schema document [Schema1 §3.14.1].
#[derive(Debug, Clone)]
pub struct SimpleTypeDef {
    /// {name} and {target namespace}: a name if it is not anonymous; else
    /// none, and the target namespace in `namespace`.
    pub name: Option<Expanded>,
    /// {target namespace} of an anonymous definition.
    pub namespace: Option<Box<str>>,
    /// {base type definition}: the simple ur-type of a list or union
    /// [Schema1 §3.14.2].
    pub base: Simple,
    /// {final}: of extension, restriction, list and union.
    pub final_: Derivations,
    /// {member type definitions} of a union, each a union among them
    /// replaced by its members [Schema1 §3.14.2]; none of another variety.
    pub members: Vec<Simple>,
    /// The datatype it defines: its {variety}, {facets} and the types its
    /// values are of [Schema2 §4.1.1].
    pub def: Arc<SimpleType>,
    /// {annotation}.
    pub annotation: Option<AnnotationRef>,
}

impl Schema {
    /// The simple type definition `id`.
    #[must_use]
    pub fn simple(&self, id: SimpleId) -> Option<&SimpleTypeDef> {
        self.simple_types.get(id.0)
    }

    /// The {final} of simple type `s`: a built-in one's is empty.
    #[must_use]
    pub fn simple_final(&self, s: Simple) -> Derivations {
        match s {
            Simple::Defined(id) => self.simple(id).map_or(Derivations::NONE, |d| d.final_),
            Simple::AnySimple | Simple::Atomic(_) | Simple::List(_) => Derivations::NONE,
        }
    }
}
