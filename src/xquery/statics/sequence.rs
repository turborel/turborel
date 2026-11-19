//! Sequence types normalized to types [XQueryFS §3.5.4], and the types
//! `node()` and `item()` stand for.

use std::sync::Arc;

use super::super::types::{AttributeKind, ElementKind, ItemKind, NodeKind, SchemaType, SeqType};
use super::types::{Atom, Item, Name, Node, Occ, Type};
use crate::xml::AtomicType;

/// `element * nillable of type xs:anyType` [XQueryFS §3.5.4]: of any
/// element, as `element()` matches any, nilled too [XQuery §2.5.4.3].
///
/// \[DESIGN\] RULES.md D106: \[XQueryFS §3.5.4\] normalizes `element()` to
/// a type that is not nillable, so of no nilled element [XQueryFS §8.3.1],
/// though XQuery's `element()` matches one; the type here is nillable, as
/// the analysis makes `element()`'s test.
#[must_use]
pub fn any_element() -> Item {
    Item::Element(Box::new(Node { name: Name::ANY, nillable: true, of: SchemaType::AnyType }))
}

/// `attribute * of type xs:anySimpleType` [XQueryFS §3.5.4].
#[must_use]
pub fn any_attribute() -> Item {
    Item::Attribute(Box::new(Node { name: Name::ANY, nillable: false, of: SchemaType::AnySimpleType }))
}

/// `(element * of type xs:anyType | text | comment | processing-instruction)*`,
/// the content of `document-node()` [XQueryFS §3.5.4].
#[must_use]
pub fn any_content() -> Type {
    Type::of_prime(vec![any_element(), Item::Text, Item::Comment, Item::Pi], Occ::Star)
}

/// `document { (element * of type xs:anyType | text | comment |
/// processing-instruction)* }`, of `document-node()`.
#[must_use]
pub fn any_document() -> Item {
    Item::Document(Box::new(any_content()))
}

/// The item types of `node()` [XQueryFS §3.5.4].
#[must_use]
pub fn node_items() -> Vec<Item> {
    vec![any_element(), any_attribute(), Item::Text, any_document(), Item::Comment, Item::Pi]
}

/// The item types of `item()` [XQueryFS §3.5.4].
#[must_use]
pub fn item_items() -> Vec<Item> {
    let mut items = node_items();
    items.push(Item::Atomic(Atom::Any));
    items
}

/// [XQueryFS §3.5.4] `[SequenceType]sequencetype`.
#[must_use]
pub fn normalized(t: &SeqType) -> Type {
    match t {
        SeqType::Empty => Type::Empty,
        SeqType::Of(kind, occurrence) => Type::occur(item_type(kind), Occ::from(*occurrence)),
    }
}

impl From<super::super::ast::Occurrence> for Occ {
    fn from(o: super::super::ast::Occurrence) -> Occ {
        match o {
            super::super::ast::Occurrence::One => Occ::One,
            super::super::ast::Occurrence::Optional => Occ::Optional,
            super::super::ast::Occurrence::Any => Occ::Star,
            super::super::ast::Occurrence::Many => Occ::Plus,
        }
    }
}

/// An item type normalized.
fn item_type(kind: &ItemKind) -> Type {
    match kind {
        ItemKind::Item => Type::of_prime(item_items(), Occ::One),
        ItemKind::Numeric => Type::Item(Item::Atomic(Atom::Numeric)),
        ItemKind::Atomic(SchemaType::Atomic(t)) => Type::atomic(*t),
        // [XQuery §2.5.4.2] A user-defined atomic type of an imported
        // schema, itself.
        ItemKind::Atomic(SchemaType::Defined(d)) => {
            Type::Item(Item::Atomic(if d.atomic() { Atom::Defined(Arc::clone(d)) } else { Atom::Any }))
        }
        ItemKind::Atomic(
            SchemaType::AnyAtomicType | SchemaType::AnyType | SchemaType::Untyped | SchemaType::AnySimpleType | SchemaType::List(_),
        ) => Type::Item(Item::Atomic(Atom::Any)),
        ItemKind::Node(node) => node_type(node),
    }
}

/// A kind test normalized [XQueryFS §3.5.4]: of `processing-instruction`
/// with a target, an optional one, as the type system has no targets.
#[must_use]
pub fn node_type(kind: &NodeKind) -> Type {
    match kind {
        NodeKind::Any => Type::of_prime(node_items(), Occ::One),
        NodeKind::Text => Type::Item(Item::Text),
        NodeKind::Comment => Type::Item(Item::Comment),
        NodeKind::Pi(None) => Type::Item(Item::Pi),
        NodeKind::Pi(Some(_)) => Type::occur(Type::Item(Item::Pi), Occ::Optional),
        NodeKind::Element(e) => element_type(e),
        NodeKind::Attribute(a) => attribute_type(a),
        NodeKind::Document(None) => Type::Item(any_document()),
        NodeKind::Document(Some(e)) => {
            let others = Type::of_prime(vec![Item::Pi, Item::Comment], Occ::Star);
            Type::Item(Item::Document(Box::new(Type::All(vec![element_type(e), others]))))
        }
    }
}

/// An element test or schema element test normalized [XQueryFS §3.5.4]:
/// of its name and those of the substitution group a schema element test's
/// element heads, of its type, `xs:anyType` of none, nillable as the test
/// lets a nilled element match.
#[must_use]
pub fn element_type(e: &ElementKind) -> Type {
    let of = e.annotation.clone().unwrap_or(SchemaType::AnyType);
    let names = match &e.name {
        None => vec![Name::ANY],
        Some(n) => std::iter::once(n).chain(&e.substitutes).map(|q| Name::new(q.uri.as_deref(), &q.local)).collect(),
    };
    let items = names.into_iter().map(|name| Item::Element(Box::new(Node { name, nillable: e.nillable, of: of.clone() })));
    Type::of_prime(items.collect(), Occ::One)
}

/// An attribute test normalized [XQueryFS §3.5.4]: `xs:anySimpleType` of
/// no type.
#[must_use]
pub fn attribute_type(a: &AttributeKind) -> Type {
    let name = a.name.as_ref().map_or(Name::ANY, |q| Name::new(q.uri.as_deref(), &q.local));
    let of = a.annotation.clone().unwrap_or(SchemaType::AnySimpleType);
    Type::Item(Item::Attribute(Box::new(Node { name, nillable: false, of })))
}

/// `xdt:untypedAtomic`.
#[must_use]
pub fn untyped_atomic() -> Type {
    Type::atomic(AtomicType::UntypedAtomic)
}
