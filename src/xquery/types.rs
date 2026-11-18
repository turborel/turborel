//! The types a query names, its names resolved [XQuery §2.5]: the schema
//! types of the in-scope schema types [XQuery §2.5.1], the sequence types
//! a value is matched against [XQuery §2.5.3, §2.5.4], and the matching.
//!
//! \[DESIGN\] RULES.md D106: a node no schema validated has the type
//! annotation `xdt:untyped`, of an element, or `xdt:untypedAtomic`, of an
//! attribute [XQuery §2.2.1]; one a validation typed has its type's, a
//! built-in type's matched by the built-in hierarchy, a registered XML
//! Schema's by that schema's derivations if a schema import brought it
//! into the in-scope schema types (`types/defined.rs`), else as derived
//! from `xs:anyType` alone. An atomic value is of an
//! `xml::AtomicType`, every built-in atomic type of XML Schema and XQuery
//! one; the hierarchy above them is `xdt:anyAtomicType`,
//! `xs:anySimpleType` and `xs:anyType` [XQueryFO §1.6]. A value of a
//! user-defined atomic type is annotated with its name, matched by that
//! type's derivation in an imported schema, and by the built-in hierarchy
//! of the built-in type it restricts.

use std::fmt;
use std::sync::Arc;

pub use defined::{Defined, Imports};

use super::ast::Occurrence;
use crate::xml::{Atomic, AtomicType, Item, Kind, Node, QName, XS_NAMESPACE, XS_PREFIX};

/// The namespace of the types XQuery adds to XML Schema's, `xdt` [XQuery
/// §2].
pub const XDT_NAMESPACE: &str = "http://www.w3.org/2005/xpath-datatypes";
/// [XQuery §2] The prefix `xdt`, as `AtomicType::name` writes it.
pub const XDT_PREFIX: &str = "xdt";

/// [Schema2 §3.3.5, §3.3.10, §3.3.12] The built-in list types, which are
/// no atomic types.
const XS_LISTS: [&str; 3] = ["NMTOKENS", "IDREFS", "ENTITIES"];
/// [Schema2 §3.4.1, §3.4.2] `xs:anyType` and `xs:anySimpleType`.
const ANY_TYPE: &str = "anyType";
/// `xs:anySimpleType`.
const ANY_SIMPLE_TYPE: &str = "anySimpleType";
/// [XQueryDM §2.6] The local names of the types of `xdt` no value is of.
const ANY_ATOMIC_TYPE: &str = "anyAtomicType";
/// `xdt:untyped`.
const UNTYPED: &str = "untyped";

/// A schema type of the in-scope schema types [XQuery §2.5.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaType {
    /// `xs:anyType`, the root of the hierarchy.
    AnyType,
    /// `xdt:untyped`, of an element no schema validated.
    Untyped,
    /// `xs:anySimpleType`.
    AnySimpleType,
    /// `xdt:anyAtomicType`, of every atomic value.
    AnyAtomicType,
    /// An atomic type.
    Atomic(AtomicType),
    /// A built-in list type, by its local name in `xs`.
    List(&'static str),
    /// A type definition of a registered XML Schema imported [XQuery
    /// §4.10].
    Defined(Arc<Defined>),
}

impl SchemaType {
    /// The type `local` names in the namespace `uri`, if it is one of the
    /// in-scope schema types [XQuery §2.5.1].
    #[must_use]
    pub fn named(uri: Option<&str>, local: &str) -> Option<SchemaType> {
        let prefix = match uri {
            Some(XS_NAMESPACE) => match local {
                ANY_TYPE => return Some(SchemaType::AnyType),
                ANY_SIMPLE_TYPE => return Some(SchemaType::AnySimpleType),
                _ => {
                    if let Some(list) = XS_LISTS.iter().find(|l| **l == local) {
                        return Some(SchemaType::List(list));
                    }
                    XS_PREFIX
                }
            },
            Some(XDT_NAMESPACE) => match local {
                ANY_ATOMIC_TYPE => return Some(SchemaType::AnyAtomicType),
                UNTYPED => return Some(SchemaType::Untyped),
                _ => XDT_PREFIX,
            },
            Some(_) | None => return None,
        };
        AtomicType::all().find(|t| t.name().split_once(':') == Some((prefix, local))).map(SchemaType::Atomic)
    }

    /// Whether it is an atomic type [XQuery §2.5.1]: a built-in one, or a
    /// simple type definition of the atomic variety of an imported schema.
    #[must_use]
    pub fn is_atomic(&self) -> bool {
        match self {
            SchemaType::AnyAtomicType | SchemaType::Atomic(_) => true,
            SchemaType::Defined(d) => d.atomic(),
            SchemaType::AnyType | SchemaType::Untyped | SchemaType::AnySimpleType | SchemaType::List(_) => false,
        }
    }

    /// The type it is derived from by restriction, none of `xs:anyType`
    /// [XQueryFO §1.6] [XQuery §2.5.1].
    fn base(&self) -> Option<SchemaType> {
        Some(match self {
            // `xs:anyType` is the root; a type of a schema imported has its
            // derivations in its schema (`Defined::derives_from`).
            SchemaType::AnyType | SchemaType::Defined(_) => return None,
            SchemaType::Untyped | SchemaType::AnySimpleType => SchemaType::AnyType,
            SchemaType::AnyAtomicType | SchemaType::List(_) => SchemaType::AnySimpleType,
            SchemaType::Atomic(t) => t.base().map_or(SchemaType::AnyAtomicType, SchemaType::Atomic),
        })
    }

    /// derives-from(self, `expected`) [XQuery §2.5.4]: whether it is
    /// `expected` or derived from it by restriction, through the bases of
    /// the built-in hierarchy, which ends at `xs:anyType` within
    /// `MAX_DERIVATION` steps.
    #[must_use]
    pub fn derives_from(&self, expected: &SchemaType) -> bool {
        if let SchemaType::Defined(d) = self {
            return d.derives_from(expected);
        }
        std::iter::successors(Some(self.clone()), SchemaType::base).take(MAX_DERIVATION.saturating_add(1)).any(|t| t == *expected)
    }
}

/// \[DESIGN\] How many types deep the hierarchy is at most: `xs:byte`'s
/// eight atomic types, then `xdt:anyAtomicType`, `xs:anySimpleType` and
/// `xs:anyType`, so a walk up from a type ends (RULES.md R-CF-6).
const MAX_DERIVATION: usize = 12;

impl fmt::Display for SchemaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaType::AnyType => write!(f, "xs:{ANY_TYPE}"),
            SchemaType::Untyped => write!(f, "xdt:{UNTYPED}"),
            SchemaType::AnySimpleType => write!(f, "xs:{ANY_SIMPLE_TYPE}"),
            SchemaType::AnyAtomicType => write!(f, "xdt:{ANY_ATOMIC_TYPE}"),
            SchemaType::Atomic(t) => write!(f, "{}", t.name()),
            SchemaType::List(local) => write!(f, "xs:{local}"),
            SchemaType::Defined(d) => write!(f, "{d}"),
        }
    }
}

/// The test of an element, or the element of a document node [XQuery
/// §2.5.4.3]: its name, any if none; its type annotation's, any if none;
/// and whether it may be nilled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementKind {
    /// The name it matches, none for any.
    pub name: Option<QName>,
    /// The names it matches besides `name`: of a schema element test, those
    /// of the substitution group its element heads [XQuery §2.5.4.4].
    pub substitutes: Vec<QName>,
    /// The type its annotation derives from, none for any.
    pub annotation: Option<SchemaType>,
    /// Whether a nilled element matches.
    pub nillable: bool,
    /// The schemas imported, against which its annotation is resolved.
    pub imports: Imports,
}

/// The test of an attribute [XQuery §2.5.4.5].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeKind {
    /// The name it matches, none for any.
    pub name: Option<QName>,
    /// The type its annotation derives from, none for any.
    pub annotation: Option<SchemaType>,
    /// The schemas imported, against which its annotation is resolved.
    pub imports: Imports,
}

/// A kind test, resolved [XQuery §2.5.3, §2.5.4.2].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// `node()`.
    Any,
    /// `document-node()`, and the test of its element if written.
    Document(Option<ElementKind>),
    /// `element()`.
    Element(ElementKind),
    /// `attribute()`.
    Attribute(AttributeKind),
    /// `processing-instruction()`, and its target if written.
    Pi(Option<Box<str>>),
    /// `comment()`.
    Comment,
    /// `text()`.
    Text,
}

/// An item type, resolved [XQuery §2.5.3].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemKind {
    /// `item()`.
    Item,
    /// An atomic type.
    Atomic(SchemaType),
    /// \[XQueryFO §1.4\] "numeric", of the signatures of functions: an
    /// `xs:integer`, `xs:decimal`, `xs:float` or `xs:double`, to which
    /// an untyped atomic value is cast as `xs:double` [XQuery §3.1.5].
    Numeric,
    /// A kind test.
    Node(NodeKind),
}

/// A sequence type, resolved [XQuery §2.5.3].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeqType {
    /// `empty-sequence()`.
    Empty,
    /// An item type and its occurrence indicator.
    Of(ItemKind, Occurrence),
}

impl SeqType {
    /// `item()*`, the type of what declares none [XQuery §4.15].
    pub const ANY: SeqType = SeqType::Of(ItemKind::Item, Occurrence::Any);

    /// Whether `items` match it [XQuery §2.5.4.1]: none, of
    /// `empty-sequence()`; else as many as its occurrence indicator allows,
    /// each matching its item type.
    #[must_use]
    pub fn matches(&self, items: &[Item]) -> bool {
        match self {
            SeqType::Empty => items.is_empty(),
            SeqType::Of(kind, occurrence) => counted(*occurrence, items.len()) && items.iter().all(|i| kind.matches(i)),
        }
    }
}

/// Whether `n` items are as many as `occurrence` allows [XQuery §2.5.4.1].
#[must_use]
pub fn counted(occurrence: Occurrence, n: usize) -> bool {
    match occurrence {
        Occurrence::One => n == 1,
        Occurrence::Optional => n <= 1,
        Occurrence::Any => true,
        Occurrence::Many => n >= 1,
    }
}

impl ItemKind {
    /// Whether `item` matches it [XQuery §2.5.4.2].
    #[must_use]
    pub fn matches(&self, item: &Item) -> bool {
        match (self, item) {
            (ItemKind::Item, _) => true,
            (ItemKind::Atomic(t), Item::Atomic(a)) => atomic_derives(a, t),
            (ItemKind::Numeric, Item::Atomic(a)) => numeric(a.kind),
            (ItemKind::Node(kind), Item::Node(node)) => kind.matches(node),
            (ItemKind::Atomic(_) | ItemKind::Numeric, Item::Node(_)) | (ItemKind::Node(_), Item::Atomic(_)) => false,
        }
    }
}

/// derives-from(the type of `a`, `expected`) [XQuery §2.5.4.2]: to a type
/// of an imported schema, by the derivation of `a`'s user-defined type in
/// that schema (`defined::value_derives`), so a value of a built-in type
/// derives from none; to a built-in type, by the built-in hierarchy of
/// the built-in type `a` is of or its type restricts.
fn atomic_derives(a: &Atomic, expected: &SchemaType) -> bool {
    if let (Some(name), SchemaType::Defined(_)) = (&a.annotation, expected) {
        return defined::value_derives(name, expected);
    }
    !matches!(expected, SchemaType::Defined(_)) && SchemaType::Atomic(a.kind).derives_from(expected)
}

/// Whether `t` is a numeric type, or derived from one [XQuery Appendix
/// B.2].
#[must_use]
pub fn numeric(t: AtomicType) -> bool {
    t.primitive().numeric()
}

/// Whether the name of `node` is `name`, none matching any [XQuery
/// §2.5.4.3].
fn named(node: &Node, name: Option<&QName>) -> bool {
    name.is_none_or(|wanted| node.tree().name(node.id()).is_some_and(|n| n.same(wanted)))
}

impl NodeKind {
    /// Whether `node` matches it [XQuery §2.5.4.2 to §2.5.4.5]: by its
    /// name and its type annotation (`annotation_derives`).
    #[must_use]
    pub fn matches(&self, node: &Node) -> bool {
        let kind = node.kind();
        match self {
            NodeKind::Any => true,
            NodeKind::Text => kind == Some(Kind::Text),
            NodeKind::Comment => kind == Some(Kind::Comment),
            NodeKind::Pi(target) => {
                kind == Some(Kind::ProcessingInstruction)
                    && target.as_deref().is_none_or(|t| node.tree().name(node.id()).is_some_and(|n| &*n.local == t))
            }
            NodeKind::Element(test) => kind == Some(Kind::Element) && element_matches(test, node),
            NodeKind::Attribute(test) => {
                let untyped = SchemaType::Atomic(AtomicType::UntypedAtomic);
                let typed = test.annotation.as_ref().is_none_or(|t| annotation_derives(node, &untyped, t, &test.imports));
                kind == Some(Kind::Attribute) && named(node, test.name.as_ref()) && typed
            }
            NodeKind::Document(test) => kind == Some(Kind::Document) && test.as_ref().is_none_or(|t| document_matches(t, node)),
        }
    }
}

/// Whether element `node` matches `test` [XQuery §2.5.3, §2.5.4.4]: its
/// name, or one of the substitution group of a schema element test; its
/// type annotation; and, if the test does not allow it, not nilled.
fn element_matches(test: &ElementKind, node: &Node) -> bool {
    let substitute = node.tree().name(node.id()).is_some_and(|n| test.substitutes.iter().any(|s| n.same(s)));
    let typed = test.annotation.as_ref().is_none_or(|t| annotation_derives(node, &SchemaType::Untyped, t, &test.imports));
    (named(node, test.name.as_ref()) || substitute) && typed && (test.nillable || !node.tree().nilled(node.id()))
}

/// derives-from(the type annotation of `node`, `expected`) [XQuery
/// §2.5.4]: of a node no schema validated, `untyped`'s; of a built-in
/// type, by the built-in hierarchy; of a type of a registered XML Schema,
/// by its schema's derivations, the schema `expected`'s or one of
/// `imports` (`defined::annotation_derives`).
fn annotation_derives(node: &Node, untyped: &SchemaType, expected: &SchemaType, imports: &Imports) -> bool {
    let Some(annotation) = node.tree().annotation(node.id()) else { return untyped.derives_from(expected) };
    match SchemaType::named(annotation.name.uri.as_deref(), &annotation.name.local) {
        Some(t) => t.derives_from(expected),
        None => defined::annotation_derives(&annotation.name, expected, imports),
    }
}

/// Whether document `node` has exactly one element child, matching
/// `test`, among comments and processing instructions [XQuery §2.5.4.2].
fn document_matches(test: &ElementKind, node: &Node) -> bool {
    let tree = node.tree();
    let children: Vec<_> = tree.children(node.id()).collect();
    let others = children.iter().all(|&c| matches!(tree.kind(c), Some(Kind::Element | Kind::Comment | Kind::ProcessingInstruction)));
    let elements: Vec<_> = children.into_iter().filter(|&c| tree.kind(c) == Some(Kind::Element)).collect();
    match elements.as_slice() {
        [only] => others && element_matches(test, &node.at(*only)),
        _ => false,
    }
}

mod defined;
#[cfg(test)]
mod tests;
