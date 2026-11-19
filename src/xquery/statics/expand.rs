//! What a type reference expands to [XQueryFS §8.1.9], read from the
//! built-in types [XQueryFS §3.5.1] and from the schema a type of an
//! imported or registered XML Schema is of: an element's attributes and
//! children, for the attribute and child axes [XQueryFS §8.2.2.1.3,
//! §8.2.2.1.4], and its typed value, for `data on` [XQueryFS §7.2.6].
//!
//! A complex type of a schema expands to the union of itself and the types
//! derived from it in its schema [XQueryFS §8.1.10], as an element of it
//! may be of any of them; its content is its content model's particles
//! mapped as [XQueryFS Appendix C.1.4.2] maps occurrences, `?`, `*` or
//! `+`, interleaved with comments and processing instructions, and of
//! mixed content with text [XQueryFS §8.1.6, §8.1.7]; its attributes are
//! its attribute uses, those of its wildcard, and the four of `xsi`
//! [XQueryFS §8.1.8].
//!
//! \[DESIGN\] RULES.md D106: an atomic value carries a built-in type alone,
//! so the typed value of a node of a simple type a schema defines is of
//! the built-in type it is derived from, of each member of a union, and
//! of its item type for a list. A typed value [XQueryFS §7.2.6] that no
//! rule gives, of element-only content, is `none`, so `data on` refuses it.

use std::sync::Arc;

use super::super::analyze::{heads, schema_type};
use super::super::plan::Namespace;
use super::super::types::{Defined, SchemaType};
use super::names::{Namespaces, name_within, narrowed};
use super::sequence::{any_content, any_element, untyped_atomic};
use super::types::{Atom, Item, Name, Node, Occ, Type, deeper};
use crate::error::Result;
use crate::xml::schema::{
    ComplexId, Compositor, ContentType, NamespaceConstraint, Particle, Schema, Scope, Simple, Term, TypeDef, Wildcard, annotated_type,
};
use crate::xml::{AtomicType, QName, SimpleType, Variety, XSI_NAMESPACE};

/// [XQueryFS §8.1.8] The built-in attributes: `xsi:type`, `xsi:nil`,
/// `xsi:schemaLocation` and `xsi:noNamespaceSchemaLocation`, each with
/// the type its declaration gives.
const BUILT_IN_ATTRIBUTES: [(&str, Option<AtomicType>); 4] = [
    ("type", Some(AtomicType::QName)),
    ("nil", Some(AtomicType::Boolean)),
    ("schemaLocation", None),
    ("noNamespaceSchemaLocation", Some(AtomicType::AnyUri)),
];

/// [Schema2 §3.3.5, §3.3.10, §3.3.12] The item types of the built-in
/// lists, by their local names.
const LIST_ITEMS: [(&str, AtomicType); 3] =
    [("NMTOKENS", AtomicType::NmToken), ("IDREFS", AtomicType::IdRef), ("ENTITIES", AtomicType::Entity)];

/// What a type reference expands to.
pub struct Expanded {
    /// Its attributes.
    pub attributes: Type,
    /// Its children: has-node-content of each type of its union
    /// [XQueryFS §8.2.2.1.3].
    pub children: Type,
    /// Its typed value, `none` where no rule gives one.
    pub data: Type,
}

/// [XQueryFS §8.1.9] What `of type of`, nillable if `nillable`, expands
/// to: of a nillable one, its children optional.
///
/// # Errors
///
/// SQLCODE -900 past `MAX_TYPE_DEPTH`.
pub fn expand(of: &SchemaType, nillable: bool) -> Result<Expanded> {
    let mut e = expand_type(of)?;
    if nillable {
        e.children = Type::occur(e.children, Occ::Optional);
    }
    Ok(e)
}

/// What `of` expands to.
fn expand_type(of: &SchemaType) -> Result<Expanded> {
    let simple = |data: Type| Expanded { attributes: Type::Empty, children: text_optional(), data };
    Ok(match of {
        // [XQueryFS §3.5.1] xdt:untyped.
        SchemaType::Untyped => Expanded {
            attributes: any_attributes_of(&Type::atomic(AtomicType::UntypedAtomic)),
            children: untyped_content(),
            data: untyped_atomic(),
        },
        // [XQueryFS §3.5.1] xs:anyType, its typed value of any type derived
        // from it, an untyped value or atomic values.
        SchemaType::AnyType => Expanded {
            attributes: any_attributes_of(&Type::Item(Item::Atomic(Atom::Any))),
            children: any_content(),
            data: Type::occur(Type::Item(Item::Atomic(Atom::Any)), Occ::Star),
        },
        SchemaType::AnySimpleType => simple(Type::occur(Type::Item(Item::Atomic(Atom::Any)), Occ::Star)),
        SchemaType::AnyAtomicType => simple(Type::Item(Item::Atomic(Atom::Any))),
        SchemaType::Atomic(t) => simple(Type::atomic(*t)),
        SchemaType::List(local) => simple(list_items(local)),
        SchemaType::Defined(d) => match d.def {
            TypeDef::Simple(s) => simple(simple_data(&d.schema, s, 0)?),
            TypeDef::Complex(id) => complex_union(&d.schema, id)?,
        },
    })
}

/// `text?`, the children of an element of simple content [XQueryFS
/// §8.2.2.1.3].
fn text_optional() -> Type {
    Type::occur(Type::Item(Item::Text), Occ::Optional)
}

/// `attribute * of type T*`, of `T` the type of `data`'s item.
fn any_attributes_of(data: &Type) -> Type {
    let of = match data {
        Type::Item(Item::Atomic(Atom::Of(t))) => SchemaType::Atomic(*t),
        Type::Item(Item::Atomic(Atom::Defined(d))) => SchemaType::Defined(Arc::clone(d)),
        Type::Empty | Type::None | Type::Item(_) | Type::Seq(_) | Type::Choice(_) | Type::All(_) | Type::Occur(..) => {
            SchemaType::AnySimpleType
        }
    };
    Type::occur(Type::Item(Item::Attribute(Box::new(Node { name: Name::ANY, nillable: false, of }))), Occ::Star)
}

/// [XQueryFS §3.5.1] The content of `xdt:untyped`: `(element * of type
/// xdt:untyped | text | comment | processing-instruction)*`.
fn untyped_content() -> Type {
    let element = Item::Element(Box::new(Node { name: Name::ANY, nillable: false, of: SchemaType::Untyped }));
    Type::of_prime(vec![element, Item::Text, Item::Comment, Item::Pi], Occ::Star)
}

/// The values of the built-in list `local`: one or more of its item type.
fn list_items(local: &str) -> Type {
    let item = LIST_ITEMS.iter().find(|(l, _)| *l == local).map_or(AtomicType::String, |(_, t)| *t);
    Type::occur(Type::atomic(item), Occ::Plus)
}

/// The typed value of a node of simple type `s` of `schema`.
fn simple_data(schema: &Arc<Schema>, s: Simple, depth: usize) -> Result<Type> {
    Ok(match s {
        Simple::AnySimple => Type::occur(Type::Item(Item::Atomic(Atom::Any)), Occ::Star),
        Simple::Atomic(t) => Type::atomic(t),
        Simple::List(t) => Type::occur(Type::atomic(t), Occ::Plus),
        Simple::Defined(id) => match schema.simple(id) {
            Some(d) => variety_data(schema, &d.def, deeper(depth)?)?,
            None => Type::Item(Item::Atomic(Atom::Any)),
        },
    })
}

/// The typed value of a node of the datatype `t`: of its atomic type, its
/// list's items, or its union's members.
fn variety_data(schema: &Arc<Schema>, t: &SimpleType, depth: usize) -> Result<Type> {
    let next = deeper(depth)?;
    Ok(match t.variety() {
        Variety::Atomic(a) => Type::Item(Item::Atomic(user_defined(schema, t).unwrap_or(Atom::Of(*a)))),
        Variety::List(item) => Type::occur(variety_data(schema, item, next)?, Occ::Star),
        Variety::Union(members) => Type::choice(members.iter().map(|m| variety_data(schema, m, next)).collect::<Result<Vec<_>>>()?),
    })
}

/// [XQuery §2.5.4.2] [XQueryDM §3.3.1.2] The user-defined atomic type of
/// `schema` the datatype `t` is, of which a value it validates is
/// annotated (`SimpleType::annotation`); none of a built-in type.
fn user_defined(schema: &Arc<Schema>, t: &SimpleType) -> Option<Atom> {
    let name = t.annotation()?;
    let def = annotated_type(schema, name)?;
    Some(Atom::Defined(Arc::new(Defined { name: QName::clone(name), schema: Arc::clone(schema), def })))
}

/// [XQueryFS §8.1.10] The union interpretation of complex type `id` of
/// `schema`: itself and each type derived from it, each expanded.
fn complex_union(schema: &Arc<Schema>, id: ComplexId) -> Result<Expanded> {
    let mut attributes = Vec::new();
    let mut children = Vec::new();
    let mut data = Vec::new();
    for at in 0..schema.complex_types.len() {
        let derived = ComplexId(at);
        if !derives(schema, derived, id) {
            continue;
        }
        let e = complex(schema, derived)?;
        attributes.push(e.attributes);
        children.push(e.children);
        data.push(e.data);
    }
    // [XQueryFS §7.2.6] `data on` a union is `data on` each of its types,
    // so none if any has none.
    let data = if data.contains(&Type::None) { Type::None } else { Type::choice(data) };
    Ok(Expanded { attributes: Type::choice(attributes), children: Type::choice(children), data })
}

/// Whether complex type `t` is `base` or derived from it, within
/// `MAX_CHAIN` steps of its bases.
fn derives(schema: &Schema, t: ComplexId, base: ComplexId) -> bool {
    let mut at = t;
    for _ in 0..crate::xml::schema::MAX_CHAIN {
        if at == base {
            return true;
        }
        // `xs:anyType` is its own base, so the walk ends there.
        let Some(TypeDef::Complex(next)) = schema.complex(at).map(|c| c.base) else { return false };
        if next == at {
            return false;
        }
        at = next;
    }
    false
}

/// Complex type `id` of `schema` expanded [XQueryFS §8.1.7].
fn complex(schema: &Arc<Schema>, id: ComplexId) -> Result<Expanded> {
    let Some(ct) = schema.complex(id) else {
        return Ok(Expanded { attributes: Type::Empty, children: Type::Empty, data: Type::Empty });
    };
    let mut attributes = built_in_attributes();
    for using in &ct.attribute_uses {
        let Some(decl) = schema.attribute(using.decl) else { continue };
        let node = Node {
            name: Name::new(decl.name.uri.as_deref(), &decl.name.local),
            nillable: false,
            of: schema_type(schema, TypeDef::Simple(decl.type_def)),
        };
        let occ = if using.required { Occ::One } else { Occ::Optional };
        attributes.push(Type::occur(Type::Item(Item::Attribute(Box::new(node))), occ));
    }
    // [Schema1 §3.4.4 Element Locally Valid (Complex Type) 3] An
    // attribute of an attribute use's name, or one of xsi's four, is not
    // the wildcard's.
    let declared = ct.attribute_uses.iter().filter_map(|u| schema.attribute(u.decl)).map(|d| (d.name.uri.clone(), d.name.local.clone()));
    let xsi = BUILT_IN_ATTRIBUTES.iter().map(|(local, _)| (Some(XSI_NAMESPACE.into()), (*local).into()));
    let except = declared.chain(xsi).map(|(uri, local)| (Namespace::of(uri), local)).collect();
    if let Some(name) = ct.wildcard.as_ref().and_then(|w| wildcard_name(w, except)) {
        let node = Node { name, nillable: false, of: SchemaType::AnySimpleType };
        attributes.push(Type::occur(Type::Item(Item::Attribute(Box::new(node))), Occ::Star));
    }
    let others = Type::of_prime(vec![Item::Comment, Item::Pi], Occ::Star);
    let (children, data) = match &ct.content {
        ContentType::Empty => (others, Type::Empty),
        ContentType::Simple(s) => (Type::All(vec![text_optional(), others]), simple_data(schema, *s, 0)?),
        ContentType::Mixed(p) => {
            let text = Type::occur(Type::Item(Item::Text), Occ::Star);
            (Type::All(vec![particle(schema, p, 0)?, text, others]), untyped_atomic())
        }
        ContentType::ElementOnly(p) => (Type::All(vec![particle(schema, p, 0)?, others]), Type::None),
    };
    Ok(Expanded { attributes: Type::All(attributes), children, data })
}

/// [XQueryFS §8.1.8] The built-in attributes, each optional.
fn built_in_attributes() -> Vec<Type> {
    BUILT_IN_ATTRIBUTES
        .iter()
        .map(|(local, t)| {
            let of = t.map_or(SchemaType::AnySimpleType, SchemaType::Atomic);
            let node = Node { name: Name::new(Some(XSI_NAMESPACE), local), nillable: false, of };
            Type::occur(Type::Item(Item::Attribute(Box::new(node))), Occ::Optional)
        })
        .collect()
}

/// \[EXTENSION\] RULES.md D106: the names wildcard `w` admits but
/// `except`, of the namespaces its {namespace constraint} allows [Schema1
/// §3.10.4 Wildcard allows Namespace Name]: of not, neither its namespace
/// nor none [2.2, 2.3]; none if it admits none.
fn wildcard_name(w: &Wildcard, except: Vec<(Namespace, Box<str>)>) -> Option<Name> {
    let namespaces = match &w.constraint {
        NamespaceConstraint::Any => Namespaces::NotAmong(Vec::new()),
        NamespaceConstraint::Not(ns) => Namespaces::NotAmong(vec![Namespace::of(ns.clone()), Namespace::Absent]),
        NamespaceConstraint::Set(set) => Namespaces::Among(set.iter().cloned().map(Namespace::of).collect()),
    };
    narrowed(namespaces, None, except)
}

/// [XQueryFS Appendix C.1.4.2] Particle `p` of `schema` as a type: its
/// term, of the occurrence its least and most occurrences approximate.
fn particle(schema: &Arc<Schema>, p: &Particle, depth: usize) -> Result<Type> {
    let next = deeper(depth)?;
    let occ = match (p.min, p.max) {
        (_, Some(0)) => return Ok(Type::Empty),
        (0, Some(1)) => Occ::Optional,
        (0, _) => Occ::Star,
        (_, Some(1)) => Occ::One,
        (_, _) => Occ::Plus,
    };
    let term = match &p.term {
        Term::Element(id) => element_particle(schema, *id),
        Term::Wildcard(w) => match (any_element(), wildcard_name(w, Vec::new())) {
            (Item::Element(node), Some(name)) => Type::Item(Item::Element(Box::new(Node { name, ..*node }))),
            // Of none, its particle is satisfied by no occurrence alone.
            (_, None) => return Ok(if p.min == 0 { Type::Empty } else { Type::None }),
            (other, Some(_)) => Type::Item(other),
        },
        Term::Group(id) => match schema.group(*id) {
            Some(g) => {
                let parts = g.particles.iter().map(|inner| particle(schema, inner, next)).collect::<Result<Vec<_>>>()?;
                match g.compositor {
                    Compositor::Sequence => Type::seq(parts),
                    Compositor::Choice => Type::choice(parts),
                    Compositor::All => Type::All(parts),
                }
            }
            None => Type::Empty,
        },
    };
    Ok(Type::occur(term, occ))
}

/// The element declaration `id` of `schema` as an element type: of a
/// global one, a choice of it and of the declarations of the substitution
/// group it heads [XQueryFS §8.1.2], none abstract.
fn element_particle(schema: &Arc<Schema>, id: crate::xml::schema::ElementId) -> Type {
    let Some(decl) = schema.element(id) else { return Type::None };
    let members: Vec<_> = if decl.scope == Scope::Global {
        schema.element_names.values().copied().filter(|&e| e == id || heads(schema, e, id)).collect()
    } else {
        vec![id]
    };
    let items = members
        .into_iter()
        .filter_map(|e| schema.element(e))
        .filter(|d| !d.abstract_)
        .map(|d| {
            let name = Name::new(d.name.uri.as_deref(), &d.name.local);
            Item::Element(Box::new(Node { name, nillable: d.nillable, of: schema_type(schema, d.type_def) }))
        })
        .collect();
    Type::of_prime(items, Occ::One)
}

/// The element types of the global element declarations of `schema`
/// named `name`, or of all of them if none is named, each with the
/// substitution group it heads: what a validation against `schema` may
/// make of an element [XQueryFS §8.6.1].
#[must_use]
pub fn declared_elements(schema: &Arc<Schema>, name: Option<&Name>) -> Type {
    let found = schema.element_names.iter().filter(|(n, _)| name.is_none_or(|w| name_within(&Name::new(n.uri.as_deref(), &n.local), w)));
    Type::choice(found.map(|(_, &id)| element_particle(schema, id)).collect())
}
