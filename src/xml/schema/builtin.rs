//! The built-in type definitions present in every schema [Schema1 §3.4.7,
//! §3.14.7]: `xs:anyType`, `xs:anySimpleType`, the built-in atomic
//! datatypes and the three built-in list datatypes of XML Schema Part 2
//! [Schema2 §3.2, §3.3]; and String Valid [Schema1 §3.14.4], a string read
//! against one of them by Datatype Valid [Schema2 §4.1.4].
//!
//! \[DESIGN\] The atomic datatypes are `xml/atomic.rs`'s, whose lexical
//! spaces and facets that table fixes; `xdt:untypedAtomic` and the two
//! durations XQuery adds are of the namespace `xdt`, not XML Schema's, so
//! no schema document names them.

use super::component::{ComplexId, ComplexType, ContentType, Derivations, Expanded, Method, Particle, Process, Simple, Term, Typed};
use super::component::{NamespaceConstraint, TypeDef, Wildcard};
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Primitive, Version, WhiteSpace, XS_NAMESPACE, from_lexical, normalize, qname_parts};

/// [Schema1 §3.4.7] The local name of the ur-type definition.
const ANY_TYPE: &str = "anyType";
/// [Schema1 §3.14.7] The local name of the simple ur-type definition.
const ANY_SIMPLE_TYPE: &str = "anySimpleType";
/// [Schema2 §3.3.5] The local name of the list of NMTOKEN.
const NMTOKENS: &str = "NMTOKENS";
/// [Schema2 §3.3.10] The local name of the list of IDREF.
const IDREFS: &str = "IDREFS";
/// [Schema2 §3.3.12] The local name of the list of ENTITY.
const ENTITIES: &str = "ENTITIES";
/// The prefix of the names of `AtomicType` that are XML Schema's.
const XS_QUALIFIED: &str = "xs:";

/// The built-in type definition of the XML Schema namespace named `local`
/// [Schema1 §3.14.7], if there is one.
pub(super) fn named(local: &str) -> Option<TypeDef> {
    match local {
        ANY_TYPE => return Some(TypeDef::Complex(ComplexId::ANY_TYPE)),
        ANY_SIMPLE_TYPE => return Some(TypeDef::Simple(Simple::AnySimple)),
        NMTOKENS => return Some(TypeDef::Simple(Simple::List(AtomicType::NmToken))),
        IDREFS => return Some(TypeDef::Simple(Simple::List(AtomicType::IdRef))),
        ENTITIES => return Some(TypeDef::Simple(Simple::List(AtomicType::Entity))),
        _ => {}
    }
    AtomicType::all().find(|t| t.name().strip_prefix(XS_QUALIFIED) == Some(local)).map(|t| TypeDef::Simple(Simple::Atomic(t)))
}

/// The {name} and {target namespace} of simple type `s`.
pub fn simple_name(s: Simple) -> Expanded {
    let local = match s {
        Simple::AnySimple => ANY_SIMPLE_TYPE,
        Simple::Atomic(t) => t.name().strip_prefix(XS_QUALIFIED).unwrap_or(t.name()),
        Simple::List(AtomicType::IdRef) => IDREFS,
        Simple::List(AtomicType::Entity) => ENTITIES,
        Simple::List(_) => NMTOKENS,
    };
    Expanded::new(Some(XS_NAMESPACE), local)
}

/// The name of `xs:anyType`.
pub fn any_type_name() -> Expanded {
    Expanded::new(Some(XS_NAMESPACE), ANY_TYPE)
}

/// [Schema1 §3.4.7] The complex type definition of the ur-type: mixed, its
/// content a sequence of any number of elements of any namespace, any
/// attribute allowed, both laxly assessed.
pub(super) fn any_type(group: super::component::GroupId) -> ComplexType {
    let any = Wildcard { constraint: NamespaceConstraint::Any, process: Process::Lax, annotation: None };
    ComplexType {
        name: Some(any_type_name()),
        namespace: Some(XS_NAMESPACE.into()),
        base: TypeDef::Complex(ComplexId::ANY_TYPE),
        method: Method::Restriction,
        final_: Derivations::NONE,
        abstract_: false,
        attribute_uses: Vec::new(),
        wildcard: Some(any),
        content: ContentType::Mixed(Particle { min: 1, max: Some(1), term: Term::Group(group) }),
        prohibited: Derivations::NONE,
        annotations: Vec::new(),
    }
}

/// The particle of the sequence of `xs:anyType`'s content [Schema1
/// §3.4.7]: any number of elements of any namespace, laxly assessed.
pub(super) fn any_content() -> Particle {
    let any = Wildcard { constraint: NamespaceConstraint::Any, process: Process::Lax, annotation: None };
    Particle { min: 0, max: None, term: Term::Wildcard(any) }
}

/// The {base type definition} of simple type `s` [Schema1 §3.14.7]
/// [Schema2 §3.3]: none of the simple ur-type, whose base is the ur-type.
pub fn simple_base(s: Simple) -> Option<Simple> {
    match s {
        Simple::AnySimple => None,
        Simple::Atomic(t) => Some(t.base().filter(|_| is_schema_type(t)).map_or(Simple::AnySimple, Simple::Atomic)),
        Simple::List(_) => Some(Simple::AnySimple),
    }
}

/// Whether `t` is one of XML Schema's datatypes, not one XQuery adds.
fn is_schema_type(t: AtomicType) -> bool {
    t.name().starts_with(XS_QUALIFIED)
}

/// Whether simple type `s` is or is derived from `xs:ID` [Schema1 §3.2.6
/// a-props-correct.3, §3.4.6 ct-props-correct.5].
pub fn is_id(s: Simple) -> bool {
    match s {
        Simple::Atomic(t) => t.derives_from(AtomicType::Id),
        Simple::AnySimple | Simple::List(_) => false,
    }
}

/// The whiteSpace facet of simple type `s` [Schema2 §4.3.6]: preserve of
/// the simple ur-type, collapse of a list [Schema2 §4.3.6.1].
pub fn white_space(s: Simple) -> WhiteSpace {
    match s {
        Simple::AnySimple => WhiteSpace::Preserve,
        Simple::Atomic(t) => t.white_space(),
        Simple::List(_) => WhiteSpace::Collapse,
    }
}

/// String Valid [Schema1 §3.14.4]: `text`, an initial value, read against
/// simple type `s` by Datatype Valid [Schema2 §4.1.4], its QNames
/// resolved by the namespaces in scope `scope` [Schema1 §3.15.3]; none if
/// it is not valid. A built-in list has at least one item [Schema2
/// §3.3.5, §3.3.10, §3.3.12]. \[DESIGN\] An `xs:ENTITY` is not checked
/// against unparsed entities, which no tree keeps, as XQuery's validate
/// does not check them [XQuery §3.13].
///
/// # Errors
///
/// SQLSTATE 10000 of a date or duration too large to hold, as
/// `xml::from_lexical`.
pub fn string_valid(s: Simple, text: &str, scope: &[(Option<&str>, &str)]) -> Result<Option<Typed>> {
    let normalized: Box<str> = normalize(text, white_space(s)).into();
    let atoms = match s {
        Simple::AnySimple => Some(vec![Atomic::new(AtomicType::UntypedAtomic, normalized.clone())]),
        Simple::Atomic(t) => atomic(t, &normalized, scope)?.map(|a| vec![a]),
        Simple::List(item) => list(item, &normalized, scope)?,
    };
    Ok(atoms.map(|atoms| Typed { normalized, atoms }))
}

/// `text` read as a value of atomic type `t`, a QName's prefix bound in
/// `scope` [Schema1 §3.15.3 clause 1.1]; none if it is no value of `t`.
fn atomic(t: AtomicType, text: &str, scope: &[(Option<&str>, &str)]) -> Result<Option<Atomic>> {
    let Some(value) = from_lexical(text, t)? else { return Ok(None) };
    if matches!(t.primitive(), Primitive::QName | Primitive::Notation) && resolve(&value.lexical, scope).is_none() {
        return Ok(None);
    }
    Ok(Some(value))
}

/// The items of `text`, a list of `item` separated by spaces, each read as
/// a value of `item`; none if there is none or one is not of `item`.
fn list(item: AtomicType, text: &str, scope: &[(Option<&str>, &str)]) -> Result<Option<Vec<Atomic>>> {
    let mut atoms = Vec::new();
    for part in text.split(' ').filter(|p| !p.is_empty()) {
        let Some(value) = atomic(item, part, scope)? else { return Ok(None) };
        atoms.push(value);
    }
    Ok(Some(atoms).filter(|a| !a.is_empty()))
}

/// The namespace name and local name of the QName `text` [Schema1
/// §3.15.3]: its prefix's namespace in `scope`; else the default
/// namespace's, absent if none is in scope. None if its prefix is not in
/// scope, or it is no QName.
pub fn resolve(text: &str, scope: &[(Option<&str>, &str)]) -> Option<Expanded> {
    let (prefix, local) = qname_parts(Version::V11, text)?;
    let uri = scope.iter().find(|(p, _)| *p == prefix).map(|(_, uri)| *uri);
    match (prefix, uri) {
        (Some(_), None) => None,
        (_, uri) => Some(Expanded::new(uri, local)),
    }
}
