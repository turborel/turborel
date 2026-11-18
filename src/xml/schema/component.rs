//! The schema components [Schema1 §2.2, §3]: element and attribute
//! declarations, complex type definitions, attribute uses, attribute
//! group and model group definitions, model groups, particles, wildcards,
//! identity-constraint definitions, notation declarations and annotations,
//! and the schema that holds them [Schema1 §3.15.1]. Each property is a
//! field named as \[Schema1\] names it.
//!
//! \[DESIGN\] RULES.md D86: components refer to one another by their place
//! in the schema's tables (`ElementId`, `ComplexId` and the others), so a
//! schema is one value, shared by the sessions that validate by it.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::automaton::Automaton;
use super::xpath::Path;
use crate::xml::{Atomic, AtomicType, NodeId, Tree, equal};

/// An expanded name [Schema1 §3.15.3]: a {target namespace}, absent or a
/// namespace name, and a {name}, an NCName.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Expanded {
    /// The namespace name, none if absent.
    pub uri: Option<Box<str>>,
    /// The local name.
    pub local: Box<str>,
}

impl Expanded {
    /// The name `local` of namespace `uri`.
    #[must_use]
    pub fn new(uri: Option<&str>, local: &str) -> Expanded {
        Expanded { uri: uri.map(Into::into), local: local.into() }
    }

    /// The name as `{uri}local`, or `local` of no namespace, for messages.
    #[must_use]
    pub fn text(&self) -> String {
        match &self.uri {
            Some(uri) => format!("{{{uri}}}{}", self.local),
            None => self.local.to_string(),
        }
    }
}

/// A set of derivation methods [Schema1 §3.3.1, §3.4.1, §3.14.1]: the
/// values of {final}, {prohibited substitutions}, {substitution group
/// exclusions} and {disallowed substitutions}.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Derivations(u8);

impl Derivations {
    /// The empty set.
    pub const NONE: Derivations = Derivations(0);
    /// extension.
    pub const EXTENSION: Derivations = Derivations(1);
    /// restriction.
    pub const RESTRICTION: Derivations = Derivations(2);
    /// substitution.
    pub const SUBSTITUTION: Derivations = Derivations(4);
    /// list.
    pub const LIST: Derivations = Derivations(8);
    /// union.
    pub const UNION: Derivations = Derivations(16);

    /// Whether it holds every method of `other`.
    #[must_use]
    pub fn has(self, other: Derivations) -> bool {
        self.0 & other.0 == other.0 && other.0 != 0
    }

    /// The union of it and `other`.
    #[must_use]
    pub fn with(self, other: Derivations) -> Derivations {
        Derivations(self.0 | other.0)
    }

    /// The methods of it also in `other`.
    #[must_use]
    pub fn within(self, other: Derivations) -> Derivations {
        Derivations(self.0 & other.0)
    }
}

/// A simple type definition [Schema1 §3.14] as turborel has them: the
/// simple ur-type definition, a built-in atomic datatype, or a built-in
/// list datatype [Schema1 §3.14.7] [Schema2 §3.2, §3.3].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Simple {
    /// `xs:anySimpleType`, the simple ur-type definition [Schema1 §3.14.7].
    AnySimple,
    /// A built-in atomic datatype.
    Atomic(AtomicType),
    /// A built-in list datatype of the item type given: `xs:NMTOKENS`,
    /// `xs:IDREFS` or `xs:ENTITIES` [Schema2 §3.3.5, §3.3.10, §3.3.12].
    List(AtomicType),
}

/// A type definition [Schema1 §2.2.1]: simple or complex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeDef {
    /// A simple type definition.
    Simple(Simple),
    /// A complex type definition.
    Complex(ComplexId),
}

/// A component's place in its schema's table of element declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ElementId(pub usize);
/// A component's place in its schema's table of attribute declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AttributeId(pub usize);
/// A component's place in its schema's table of complex type definitions;
/// the first is `xs:anyType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComplexId(pub usize);
/// A component's place in its schema's table of attribute group
/// definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AttributeGroupId(pub usize);
/// A component's place in its schema's table of model groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GroupId(pub usize);
/// A component's place in its schema's table of model group definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GroupDefId(pub usize);
/// A component's place in its schema's table of identity-constraint
/// definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConstraintId(pub usize);
/// A component's place in its schema's table of notation declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NotationId(pub usize);

impl ComplexId {
    /// `xs:anyType`, the ur-type definition [Schema1 §3.4.7], first in
    /// every schema.
    pub const ANY_TYPE: ComplexId = ComplexId(0);
}

/// An annotation [Schema1 §3.13]: the `<annotation>` element of a schema
/// document, if one is written, and the element whose attributes of other
/// namespaces are its {attributes}. Its {application information} and
/// {user information} are the `<appinfo>` and `<documentation>` children
/// of the `<annotation>` element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnotationRef {
    /// The schema document, a place in the schema's documents.
    pub document: usize,
    /// The `<annotation>` element.
    pub node: NodeId,
}

/// Whether a value constraint is a default or fixed [Schema1 §3.2.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// default.
    Default,
    /// fixed.
    Fixed,
}

/// A {value constraint} [Schema1 §3.2.1, §3.3.1, §3.5.1]: a value and
/// whether it is a default or fixed. The value is kept as its typed value,
/// so it is compared as a value, not a string [Schema1 §3.2.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueConstraint {
    /// default or fixed.
    pub kind: ValueKind,
    /// The value, as validating its lexical form against its type gives
    /// it.
    pub value: Typed,
}

/// A string as String Valid [Schema1 §3.14.4] reads it against a simple
/// type: its normalized value [Schema1 §3.1.4] and the atomic values of
/// its actual value, one of an atomic type, each item of a list
/// [XQueryDM §3.3.1.2].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typed {
    /// The normalized value.
    pub normalized: Box<str>,
    /// The typed value.
    pub atoms: Vec<Atomic>,
}

impl Typed {
    /// Its canonical lexical representation [Schema2 §2.3.1]: the
    /// canonical form of each atomic value, a list's items separated by a
    /// space.
    #[must_use]
    pub fn canonical(&self) -> String {
        self.atoms.iter().map(|a| &*a.lexical).collect::<Vec<_>>().join(" ")
    }

    /// Whether it and `other` are the same value: as many atomic values,
    /// each equal to the other's by Equal [Schema2 §4.2.1].
    #[must_use]
    pub fn equal(&self, other: &Typed) -> bool {
        self.atoms.len() == other.atoms.len() && self.atoms.iter().zip(&other.atoms).all(|(a, b)| equal(a, b))
    }
}

/// The {scope} of a declaration [Schema1 §3.2.1, §3.3.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// global.
    Global,
    /// A complex type definition.
    Local(ComplexId),
    /// Absent: a declaration within a named group, whose scope is the
    /// type that uses it.
    Absent,
}

/// An element declaration [Schema1 §3.3.1].
#[derive(Debug, Clone)]
pub struct ElementDecl {
    /// {name} and {target namespace}.
    pub name: Expanded,
    /// {type definition}.
    pub type_def: TypeDef,
    /// {scope}.
    pub scope: Scope,
    /// {value constraint}.
    pub value: Option<ValueConstraint>,
    /// {nillable}.
    pub nillable: bool,
    /// {identity-constraint definitions}.
    pub constraints: Vec<ConstraintId>,
    /// {substitution group affiliation}.
    pub substitution_group: Option<ElementId>,
    /// {substitution group exclusions}: of extension and restriction.
    pub exclusions: Derivations,
    /// {disallowed substitutions}: of substitution, extension and
    /// restriction.
    pub disallowed: Derivations,
    /// {abstract}.
    pub abstract_: bool,
    /// {annotation}.
    pub annotation: Option<AnnotationRef>,
}

/// An attribute declaration [Schema1 §3.2.1].
#[derive(Debug, Clone)]
pub struct AttributeDecl {
    /// {name} and {target namespace}.
    pub name: Expanded,
    /// {type definition}.
    pub type_def: Simple,
    /// {scope}.
    pub scope: Scope,
    /// {value constraint}.
    pub value: Option<ValueConstraint>,
    /// {annotation}.
    pub annotation: Option<AnnotationRef>,
}

/// An attribute use [Schema1 §3.5.1].
#[derive(Debug, Clone)]
pub struct AttributeUse {
    /// {required}.
    pub required: bool,
    /// {attribute declaration}.
    pub decl: AttributeId,
    /// {value constraint}.
    pub value: Option<ValueConstraint>,
}

/// {derivation method} [Schema1 §3.4.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// extension.
    Extension,
    /// restriction.
    Restriction,
}

/// {content type} [Schema1 §3.4.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentType {
    /// empty.
    Empty,
    /// A simple type definition.
    Simple(Simple),
    /// A content model and mixed.
    Mixed(Particle),
    /// A content model and element-only.
    ElementOnly(Particle),
}

impl ContentType {
    /// Its particle, if it has a content model.
    #[must_use]
    pub fn particle(&self) -> Option<&Particle> {
        match self {
            ContentType::Mixed(p) | ContentType::ElementOnly(p) => Some(p),
            ContentType::Empty | ContentType::Simple(_) => None,
        }
    }
}

/// A complex type definition [Schema1 §3.4.1].
#[derive(Debug, Clone)]
pub struct ComplexType {
    /// {name} and {target namespace}: a name if it is not anonymous; else
    /// none, and the target namespace in `namespace`.
    pub name: Option<Expanded>,
    /// {target namespace} of an anonymous definition.
    pub namespace: Option<Box<str>>,
    /// {base type definition}.
    pub base: TypeDef,
    /// {derivation method}.
    pub method: Method,
    /// {final}.
    pub final_: Derivations,
    /// {abstract}.
    pub abstract_: bool,
    /// {attribute uses}.
    pub attribute_uses: Vec<AttributeUse>,
    /// {attribute wildcard}.
    pub wildcard: Option<Wildcard>,
    /// {content type}.
    pub content: ContentType,
    /// {prohibited substitutions}.
    pub prohibited: Derivations,
    /// {annotations}.
    pub annotations: Vec<AnnotationRef>,
}

/// An attribute group definition [Schema1 §3.6.1].
#[derive(Debug, Clone)]
pub struct AttributeGroupDef {
    /// {name} and {target namespace}.
    pub name: Expanded,
    /// {attribute uses}.
    pub attribute_uses: Vec<AttributeUse>,
    /// {attribute wildcard}.
    pub wildcard: Option<Wildcard>,
    /// {annotation}.
    pub annotation: Option<AnnotationRef>,
}

/// A model group definition [Schema1 §3.7.1].
#[derive(Debug, Clone)]
pub struct ModelGroupDef {
    /// {name} and {target namespace}.
    pub name: Expanded,
    /// {model group}.
    pub group: GroupId,
    /// {annotation}.
    pub annotation: Option<AnnotationRef>,
}

/// {compositor} [Schema1 §3.8.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compositor {
    /// all.
    All,
    /// choice.
    Choice,
    /// sequence.
    Sequence,
}

/// A model group [Schema1 §3.8.1].
#[derive(Debug, Clone)]
pub struct ModelGroup {
    /// {compositor}.
    pub compositor: Compositor,
    /// {particles}.
    pub particles: Vec<Particle>,
    /// {annotation}.
    pub annotation: Option<AnnotationRef>,
}

/// {term} [Schema1 §3.9.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    /// An element declaration.
    Element(ElementId),
    /// A wildcard.
    Wildcard(Wildcard),
    /// A model group.
    Group(GroupId),
}

/// A particle [Schema1 §3.9.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Particle {
    /// {min occurs}.
    pub min: u64,
    /// {max occurs}: none if unbounded.
    pub max: Option<u64>,
    /// {term}.
    pub term: Term,
}

/// {namespace constraint} [Schema1 §3.10.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamespaceConstraint {
    /// any.
    Any,
    /// A pair of not and a namespace name, or absent.
    Not(Option<Box<str>>),
    /// A set of namespace names and absent, in order, each once.
    Set(Vec<Option<Box<str>>>),
}

/// {process contents} [Schema1 §3.10.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Process {
    /// skip.
    Skip,
    /// lax.
    Lax,
    /// strict.
    Strict,
}

/// A wildcard [Schema1 §3.10.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wildcard {
    /// {namespace constraint}.
    pub constraint: NamespaceConstraint,
    /// {process contents}.
    pub process: Process,
    /// {annotation}.
    pub annotation: Option<AnnotationRef>,
}

/// {identity-constraint category} [Schema1 §3.11.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityCategory {
    /// key.
    Key,
    /// keyref.
    KeyRef,
    /// unique.
    Unique,
}

/// An identity-constraint definition [Schema1 §3.11.1].
#[derive(Debug, Clone)]
pub struct IdentityConstraint {
    /// {name} and {target namespace}.
    pub name: Expanded,
    /// {identity-constraint category}.
    pub category: IdentityCategory,
    /// {selector}: its paths, any of which selects.
    pub selector: Vec<Path>,
    /// {fields}: each its paths.
    pub fields: Vec<Vec<Path>>,
    /// {referenced key}.
    pub refer: Option<ConstraintId>,
    /// {annotation}.
    pub annotation: Option<AnnotationRef>,
}

/// A notation declaration [Schema1 §3.12.1].
#[derive(Debug, Clone)]
pub struct Notation {
    /// {name} and {target namespace}.
    pub name: Expanded,
    /// {system identifier}.
    pub system: Option<Box<str>>,
    /// {public identifier}.
    pub public: Option<Box<str>>,
    /// {annotation}.
    pub annotation: Option<AnnotationRef>,
}

/// A schema [Schema1 §3.15.1]: its components, the top-level ones by name,
/// and the schema documents it was read from.
#[derive(Debug, Clone, Default)]
pub struct Schema {
    /// Every element declaration, top-level and local.
    pub elements: Vec<ElementDecl>,
    /// Every attribute declaration, top-level and local.
    pub attributes: Vec<AttributeDecl>,
    /// Every complex type definition, named and anonymous, `xs:anyType`
    /// first.
    pub complex_types: Vec<ComplexType>,
    /// Every attribute group definition.
    pub attribute_groups: Vec<AttributeGroupDef>,
    /// Every model group.
    pub groups: Vec<ModelGroup>,
    /// Every model group definition.
    pub group_defs: Vec<ModelGroupDef>,
    /// Every identity-constraint definition.
    pub constraints: Vec<IdentityConstraint>,
    /// Every notation declaration.
    pub notations: Vec<Notation>,
    /// {type definitions}: the named ones by name, built-ins of the XML
    /// Schema namespace not among them [Schema1 §3.14.7].
    pub type_names: BTreeMap<Expanded, TypeDef>,
    /// {element declarations}, by name.
    pub element_names: BTreeMap<Expanded, ElementId>,
    /// {attribute declarations}, by name.
    pub attribute_names: BTreeMap<Expanded, AttributeId>,
    /// {attribute group definitions}, by name.
    pub attribute_group_names: BTreeMap<Expanded, AttributeGroupId>,
    /// {model group definitions}, by name.
    pub group_names: BTreeMap<Expanded, GroupDefId>,
    /// {notation declarations}, by name.
    pub notation_names: BTreeMap<Expanded, NotationId>,
    /// Identity-constraint definitions, by name, as their symbol space is
    /// one for the schema [Schema1 §3.11.1].
    pub constraint_names: BTreeMap<Expanded, ConstraintId>,
    /// {annotations} of the schema documents.
    pub annotations: Vec<AnnotationRef>,
    /// The automaton of each complex type's content model, by the type's
    /// place, none of a type with no content model or an all group.
    pub automata: Vec<Option<Automaton>>,
    /// The schema documents, each a tree, and its schema location.
    pub documents: Vec<(Arc<Tree>, Option<Box<str>>)>,
}

impl Schema {
    /// The element declaration `id`.
    #[must_use]
    pub fn element(&self, id: ElementId) -> Option<&ElementDecl> {
        self.elements.get(id.0)
    }

    /// The attribute declaration `id`.
    #[must_use]
    pub fn attribute(&self, id: AttributeId) -> Option<&AttributeDecl> {
        self.attributes.get(id.0)
    }

    /// The complex type definition `id`.
    #[must_use]
    pub fn complex(&self, id: ComplexId) -> Option<&ComplexType> {
        self.complex_types.get(id.0)
    }

    /// The model group `id`.
    #[must_use]
    pub fn group(&self, id: GroupId) -> Option<&ModelGroup> {
        self.groups.get(id.0)
    }

    /// The identity-constraint definition `id`.
    #[must_use]
    pub fn constraint(&self, id: ConstraintId) -> Option<&IdentityConstraint> {
        self.constraints.get(id.0)
    }

    /// The automaton of the content model of complex type `id`, if built.
    #[must_use]
    pub fn automaton(&self, id: ComplexId) -> Option<&Automaton> {
        self.automata.get(id.0).and_then(Option::as_ref)
    }

    /// The namespaces of its components [ISO/IEC 9075-14:2006 draft
    /// §4.2.6]: each {target namespace} of a top-level component, none
    /// for absent, in order.
    #[must_use]
    pub fn namespaces(&self) -> Vec<Option<&str>> {
        let mut out: Vec<Option<&str>> = self
            .type_names
            .keys()
            .chain(self.element_names.keys())
            .chain(self.attribute_names.keys())
            .chain(self.attribute_group_names.keys())
            .chain(self.group_names.keys())
            .chain(self.notation_names.keys())
            .map(|name| name.uri.as_deref())
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}
