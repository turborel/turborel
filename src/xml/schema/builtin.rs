//! The built-in type definitions present in every schema [Schema1 §3.4.7,
//! §3.14.7]: `xs:anyType`, `xs:anySimpleType`, the built-in atomic
//! datatypes and the three built-in list datatypes of XML Schema Part 2
//! [Schema2 §3.2, §3.3]; and String Valid [Schema1 §3.14.4], a string read
//! against a simple type, built in or defined, by Datatype Valid [Schema2
//! §4.1.4] (`SimpleType::validate`).
//!
//! \[DESIGN\] The atomic datatypes are `xml/atomic.rs`'s, whose lexical
//! spaces and facets that table fixes; `xdt:untypedAtomic` and the two
//! durations XQuery adds are of the namespace `xdt`, not XML Schema's, so
//! no schema document names them.

use std::sync::Arc;

use super::beyond;
use super::component::{
    ComplexId, ComplexType, ContentType, Derivations, Expanded, Method, Particle, Process, Schema, Simple, Term, Typed,
};
use super::component::{NamespaceConstraint, TypeDef, Wildcard};
use super::{BuiltInList, SimpleType};
use crate::error::{Result, SqlError, SqlState};
use crate::xml::{Atomic, AtomicType, Primitive, Version, XS_NAMESPACE, qname_parts};

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
/// [Schema2 §3.3.5, §3.3.10, §3.3.12] The built-in list datatypes: the
/// item type of each, the list, and its local name.
const BUILT_IN_LISTS: [(AtomicType, BuiltInList, &str); 3] = [
    (AtomicType::NmToken, BuiltInList::NmTokens, NMTOKENS),
    (AtomicType::IdRef, BuiltInList::IdRefs, IDREFS),
    (AtomicType::Entity, BuiltInList::Entities, ENTITIES),
];

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

/// The {name} and {target namespace} of simple type `s` of `schema`;
/// none of an anonymous one.
pub fn simple_name(schema: &Schema, s: Simple) -> Option<Expanded> {
    let local = match s {
        Simple::AnySimple => ANY_SIMPLE_TYPE,
        Simple::Atomic(t) => t.name().strip_prefix(XS_QUALIFIED).unwrap_or(t.name()),
        Simple::List(t) => BUILT_IN_LISTS.iter().find(|(item, ..)| *item == t).map_or(NMTOKENS, |(_, _, name)| *name),
        Simple::Defined(id) => return schema.simple(id).and_then(|d| d.name.clone()),
    };
    Some(Expanded::new(Some(XS_NAMESPACE), local))
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

/// The {base type definition} of simple type `s` of `schema` [Schema1
/// §3.14.1, §3.14.7] [Schema2 §3.3]: none of the simple ur-type, whose
/// base is the ur-type.
pub fn simple_base(schema: &Schema, s: Simple) -> Option<Simple> {
    match s {
        Simple::AnySimple => None,
        Simple::Atomic(t) => Some(t.base().filter(|_| is_schema_type(t)).map_or(Simple::AnySimple, Simple::Atomic)),
        Simple::List(_) => Some(Simple::AnySimple),
        Simple::Defined(id) => Some(schema.simple(id).map_or(Simple::AnySimple, |d| d.base)),
    }
}

/// Whether `t` is one of XML Schema's datatypes, not one XQuery adds.
fn is_schema_type(t: AtomicType) -> bool {
    t.name().starts_with(XS_QUALIFIED)
}

/// Whether simple type `s` of `schema` is or is derived from `xs:ID`
/// [Schema1 §3.2.6 a-props-correct.3, §3.4.6 ct-props-correct.5]: an
/// atomic type restricting it.
pub fn is_id(schema: &Schema, s: Simple) -> bool {
    match s {
        Simple::Atomic(t) => t.derives_from(AtomicType::Id),
        Simple::Defined(id) => schema.simple(id).and_then(|d| d.def.atomic()).is_some_and(|t| t.derives_from(AtomicType::Id)),
        Simple::AnySimple | Simple::List(_) => false,
    }
}

/// The datatype of simple type `s` of `schema` [Schema2 §4.1]: a built-in
/// one's, of a built-in list its list type [Schema2 Appendix A], of a
/// list of another type the list of it; a definition's own; none of the
/// simple ur-type, which has no facets [Schema1 §3.14.1].
///
/// # Errors
///
/// SQLCODE -900 if a definition is missing, which no schema read has.
pub fn simple_type(schema: &Schema, s: Simple) -> Result<Option<Arc<SimpleType>>> {
    let ty = match s {
        Simple::AnySimple => return Ok(None),
        Simple::Atomic(t) => Arc::new(SimpleType::built_in(t)),
        Simple::List(t) => match BUILT_IN_LISTS.iter().find(|(item, ..)| *item == t) {
            Some((_, list, _)) => Arc::new(SimpleType::built_in_list(*list)),
            None => Arc::new(SimpleType::list(Arc::new(SimpleType::built_in(t)))?),
        },
        Simple::Defined(id) => Arc::clone(&schema.simple(id).ok_or_else(|| beyond("a simple type definition is missing"))?.def),
    };
    Ok(Some(ty))
}

/// String Valid [Schema1 §3.14.4]: `text`, an initial value, read against
/// simple type `s` of `schema` by Datatype Valid [Schema2 §4.1.4], its
/// QNames' and NOTATIONs' prefixes in scope `scope` [Schema1 §3.15.3]; none
/// if it is not valid. Of the simple ur-type, its value unnormalized as
/// an `xdt:untypedAtomic` [XQueryDM §3.3.1.2]. \[DESIGN\] An `xs:ENTITY`
/// is not checked against unparsed entities, which no tree keeps, as
/// XQuery's validate does not check them [XQuery §3.13]; nor a NOTATION
/// against the notations the schema declares.
///
/// # Errors
///
/// SQLCODE -900 if `s`'s lists and unions nest past `MAX_TYPE_NESTING`;
/// as the matching of a pattern.
pub fn string_valid(schema: &Schema, s: Simple, text: &str, scope: &[(Option<&str>, &str)]) -> Result<Option<Typed>> {
    let Some(ty) = simple_type(schema, s)? else {
        return Ok(Some(Typed { normalized: text.into(), atoms: vec![Atomic::new(AtomicType::UntypedAtomic, text)] }));
    };
    let (normalized, atoms) = match ty.validate_normalized(text) {
        Ok(read) => read,
        Err(e) => return not_valid(e),
    };
    let unbound = |a: &Atomic| matches!(a.kind.primitive(), Primitive::QName | Primitive::Notation) && resolve(&a.lexical, scope).is_none();
    if atoms.iter().any(unbound) {
        return Ok(None);
    }
    Ok(Some(Typed { normalized: normalized.into(), atoms }))
}

/// Error `e` of a validation: none, the literal not valid, if it is
/// err:FORG0001, SQLSTATE 10000 [Schema2 §4.1.4] (`SimpleType::validate`);
/// else the error.
///
/// # Errors
///
/// `e`, if it is not SQLSTATE 10000.
fn not_valid(e: SqlError) -> Result<Option<Typed>> {
    if e.sqlstate() == SqlState::XQUERY_ERROR { Ok(None) } else { Err(e) }
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
