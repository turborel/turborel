//! Simple type definitions [Schema2 §4.1]: the built-in types, and types
//! derived from them by restriction, by list or by union [Schema2 §2.5,
//! §4.1.2], each with its constraining facets (`schema/facet.rs`), the
//! rules a restriction keeps (`schema/restrict.rs`), and the validation of
//! a literal and the cast of a value to one (`schema/validate.rs`).
//!
//! \[DESIGN\] RULES.md D86: a type is a value, its base, item and member
//! types shared, and its facets merged with its base's as it is made, so
//! validating a literal never walks its derivation. Lists and unions nest
//! at most `MAX_TYPE_NESTING` deep, so validating walks a bounded depth.
//!
//! XML Schema Part 1, Structures \[Schema1\], over those simple types: the schema components of
//! [Schema1 §3] (`schema/component.rs`), read from schema documents
//! [Schema1 §3.x.2, §4.2] (`schema/read.rs`), checked against the schema
//! component constraints and schema representation constraints [Schema1
//! §3.x.3, §3.x.6] (`schema/check.rs`), their content models compiled to
//! automata (`schema/automaton.rs`). A schema is what a registered XML
//! Schema holds [ISO/IEC 9075-14:2006 draft §4.2.6].
//!
//! \[DESIGN\] RULES.md D86: a schema is an arena of its components, each
//! kind in a table of its own and referring to others by their place in
//! it, so the cycles [Schema1 §3.1.1] allows (an element of a type whose
//! content holds the element) are indexes, not references. Simple types
//! are the built-in datatypes of XML Schema Part 2 (`xml/atomic.rs`); a
//! user-defined simple type definition [Schema2 §4.1] is refused until
//! Part 2's facets are built.
//!
//! \[DESIGN\] No external resource is ever fetched [Schema1 §4.3.2
//! clause 1]: an `<include>`, `<redefine>` or `<import>` names a schema
//! document only among those it is read with (`Library`), by its schema
//! location or, for an import, its namespace. Reading recurses through
//! nested declarations, derivation chains and references, each bounded by
//! a named depth, and a schema past one is SQLCODE -900 (RULES.md D18).

mod assess;
mod automaton;
mod builtin;
mod check;
mod component;
mod determinism;
mod facet;
mod read;
mod restrict;
mod validate;
mod wildcard;
mod xpath;

#[cfg(test)]
mod tests;

pub use assess::{Failure, INVALID, Mode, NOT_A_DOCUMENT, NOT_A_NODE, UNDECLARED, validate};
pub use automaton::{Automaton, MAX_POSITIONS, MAX_STATES, StateId};
pub use check::{substitutable, type_derives};
pub use component::{
    AnnotationRef, AttributeDecl, AttributeGroupDef, AttributeGroupId, AttributeId, AttributeUse, ComplexId, ComplexType, Compositor,
    ConstraintId, ContentType, Derivations, ElementDecl, ElementId, Expanded, GroupDefId, GroupId, IdentityCategory, IdentityConstraint,
    Method, ModelGroup, ModelGroupDef, NamespaceConstraint, Notation, NotationId, Particle, Process, Schema, Scope, Simple, Term, TypeDef,
    Typed, ValueConstraint, ValueKind, Wildcard,
};
pub use read::{Library, Source, read};
pub use xpath::{NameTest, Path, Step};

pub use facet::{Facet, FacetKind, FacetSpec, Facets, Held, Pattern, applicable};

use std::sync::Arc;

use super::{AtomicType, WhiteSpace};
use crate::error::{Code, Result, SqlError};

/// \[DESIGN\] How deep lists and unions nest in a type at most, so a
/// validation, which descends into each, is bounded (RULES.md R-CF-6). A
/// type nested deeper is SQLCODE -900 (D18). The value is no measurement:
/// a schema's types nest a few deep.
pub const MAX_TYPE_NESTING: usize = 32;

/// [Schema2 §3.3.5, §3.3.10, §3.3.12] The built-in list types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltInList {
    /// `xs:NMTOKENS`, of `xs:NMTOKEN`.
    NmTokens,
    /// `xs:IDREFS`, of `xs:IDREF`.
    IdRefs,
    /// `xs:ENTITIES`, of `xs:ENTITY`.
    Entities,
}

/// What a simple type's values are [Schema2 §2.5.1, §4.1.1 {variety}].
#[derive(Debug, Clone)]
pub enum Variety {
    /// Atomic: values of the built-in type named, which it is or is
    /// derived from by restriction.
    Atomic(AtomicType),
    /// List: sequences of values of its item type, atomic or a union.
    List(Arc<SimpleType>),
    /// Union: the values of each member type, none of them a union.
    Union(Vec<Arc<SimpleType>>),
}

/// A simple type definition [Schema2 §4.1.1].
#[derive(Debug, Clone)]
pub struct SimpleType {
    name: Box<str>,
    variety: Variety,
    facets: Facets,
    depth: usize,
}

/// The error of a type definition no valid one, `why`, by `rule`: a
/// constraint on schemas broken [Schema2 §5].
fn invalid_type(rule: &str, why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[Schema2 {rule}] the simple type definition is not valid: {why}"))
}

impl SimpleType {
    /// The built-in atomic type `t` [Schema2 §3.2, §3.3, Appendix A].
    #[must_use]
    pub fn built_in(t: AtomicType) -> SimpleType {
        SimpleType { name: t.name().into(), variety: Variety::Atomic(t), facets: Facets::built_in(t), depth: 0 }
    }

    /// The built-in list type `list` [Schema2 Appendix A]: a list of its
    /// item type, of one item at least.
    #[must_use]
    pub fn built_in_list(list: BuiltInList) -> SimpleType {
        let (name, item) = match list {
            BuiltInList::NmTokens => ("xs:NMTOKENS", AtomicType::NmToken),
            BuiltInList::IdRefs => ("xs:IDREFS", AtomicType::IdRef),
            BuiltInList::Entities => ("xs:ENTITIES", AtomicType::Entity),
        };
        let facets = Facets {
            white_space: Some(Held { value: WhiteSpace::Collapse, fixed: true }),
            min_length: Some(Held { value: 1, fixed: false }),
            ..Facets::default()
        };
        SimpleType { name: name.into(), variety: Variety::List(Arc::new(SimpleType::built_in(item))), facets, depth: 1 }
    }

    /// The list type of `item` [Schema2 §4.1.2.2]: its whiteSpace collapse
    /// and fixed [Schema2 §4.3.6].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if `item` is a list [Schema2 §4.1.5 list of atomic];
    /// SQLCODE -900 if it nests past `MAX_TYPE_NESTING`.
    pub fn list(item: Arc<SimpleType>) -> Result<SimpleType> {
        if matches!(item.variety, Variety::List(_)) {
            return Err(invalid_type("§4.1.5 list of atomic", &format!("the item type {} is a list", item.name)));
        }
        let depth = nested(item.depth)?;
        let facets = Facets { white_space: Some(Held { value: WhiteSpace::Collapse, fixed: true }), ..Facets::default() };
        Ok(SimpleType { name: format!("a list of {}", item.name).into(), variety: Variety::List(item), facets, depth })
    }

    /// The union of `members` [Schema2 §4.1.2.3], each that is a union
    /// replaced by its members, in order.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if there are none [Schema2 §4.1.3]; SQLCODE -900 if
    /// they nest past `MAX_TYPE_NESTING`.
    pub fn union(members: Vec<Arc<SimpleType>>) -> Result<SimpleType> {
        let mut flat = Vec::with_capacity(members.len());
        for member in members {
            match &member.variety {
                Variety::Union(inner) => flat.extend(inner.iter().cloned()),
                Variety::Atomic(_) | Variety::List(_) => flat.push(member),
            }
        }
        let deepest = flat.iter().map(|m| m.depth).max().ok_or_else(|| invalid_type("§4.1.3", "a union has no member type"))?;
        let names: Vec<&str> = flat.iter().map(|m| &*m.name).collect();
        let name = format!("a union of {}", names.join(", "));
        Ok(SimpleType { name: name.into(), variety: Variety::Union(flat), facets: Facets::default(), depth: nested(deepest)? })
    }

    /// It named `name`.
    #[must_use]
    pub fn named(self, name: &str) -> SimpleType {
        SimpleType { name: name.into(), ..self }
    }

    /// Its name, or a description of an anonymous type.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its variety.
    #[must_use]
    pub fn variety(&self) -> &Variety {
        &self.variety
    }

    /// Its facets, its own and its base's [Schema2 §4.1.1 {facets}].
    #[must_use]
    pub fn facets(&self) -> &Facets {
        &self.facets
    }

    /// The built-in type it is or restricts, if it is atomic.
    #[must_use]
    pub fn atomic(&self) -> Option<AtomicType> {
        match self.variety {
            Variety::Atomic(t) => Some(t),
            Variety::List(_) | Variety::Union(_) => None,
        }
    }

    /// How its literals are normalized [Schema2 §4.3.6]: by its
    /// whiteSpace facet; a union's not at all, as the member it is
    /// validated by normalizes them.
    #[must_use]
    pub fn white_space(&self) -> WhiteSpace {
        self.facets.white_space.as_ref().map_or(WhiteSpace::Preserve, |held| held.value)
    }
}

/// The depth of a list or union whose deepest part is `depth` deep.
///
/// # Errors
///
/// SQLCODE -900 past `MAX_TYPE_NESTING` (RULES.md D18).
fn nested(depth: usize) -> Result<usize> {
    let deeper = depth.saturating_add(1);
    if deeper > MAX_TYPE_NESTING {
        return Err(SqlError::new(
            Code::Unsupported,
            format!("[DESIGN] the simple type is beyond turborel's limits: lists and unions nest more than {MAX_TYPE_NESTING} deep"),
        ));
    }
    Ok(deeper)
}

/// \[DESIGN\] How deeply the elements of a schema document nest at most
/// where reading them recurses: local declarations within anonymous types
/// within model groups (RULES.md R-CF-6). \[Schema1\] sets no limit; the
/// value is no measurement, but a bound well past the nesting of the
/// schemas applications write, each level of which takes a few frames of
/// the reader's stack.
pub const MAX_DEPTH: usize = 128;

/// \[DESIGN\] How many type definitions, substitution group affiliations or
/// attribute group references a chain of them follows at most (RULES.md
/// R-CF-6): a derivation [Schema1 §3.4.6 ct-props-correct.3], a
/// substitution group [Schema1 §3.3.6] or attribute groups referring to
/// others [Schema1 §3.6.3 src-attribute_group.3].
pub const MAX_CHAIN: usize = 64;

/// \[DESIGN\] How many schema documents one schema is read from at most,
/// its own and those it includes, redefines or imports [Schema1 §4.2],
/// each counted once.
pub const MAX_DOCUMENTS: usize = 256;

/// The error of a text that is no schema document, or of components that
/// are no schema, by the constraint `rule` it breaks, `why`: SQLSTATE
/// 22000. \[EXTENSION\] Part 14 leaves how an XML Schema is registered to
/// the implementation [ISO/IEC 9075-14:2006 draft §4.2.6], and so the
/// condition of a schema that cannot be registered.
fn invalid(rule: &str, why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] {rule} the schema is not valid: {why}"))
}

/// The error of a schema beyond turborel's limits, `why` (RULES.md D18).
fn beyond(why: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[DESIGN] the XML Schema is beyond turborel's limits: {why}"))
}
