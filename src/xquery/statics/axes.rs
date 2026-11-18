//! Axis steps typed [XQueryFS §4.2.1]: the axis applied to the type of the
//! context item [XQueryFS §8.2.2.1], then the node test [XQueryFS
//! §8.2.3.1], each to each item type of what it is applied to, the
//! structure of the type kept.
//!
//! The following-sibling and preceding-sibling axes are typed as their
//! normalization, the parent's children [XQueryFS §4.2.1.1]; the following
//! and preceding axes as theirs, the descendants or self of the siblings
//! of the ancestors or self. The descendant and ancestor axes are the
//! closures of the child and parent axes, each item type met once
//! [XQueryFS §8.2.2.1.7, §8.2.2.1.9].

use super::super::plan::{Axis, Namespace, StepTest};
use super::super::types::NodeKind;
use super::beyond;
use super::expand::expand;
use super::sequence::{any_document, any_element, node_type};
use super::subtype::{item_sub, map_items};
use super::types::{Item, Name, Node, Occ, Type, prime};
use crate::error::Result;

/// \[DESIGN\] How many item types the closure of an axis meets at most,
/// so it ends (RULES.md R-CF-6): a schema's element declarations are
/// finite, and each item type is met once; past this SQLCODE -900 (D18).
/// The value is no measurement.
pub const MAX_CLOSURE: usize = 4096;

/// [XQueryFS §4.2.1] `Axis NodeTest` of the context item's type `focus`.
///
/// # Errors
///
/// err:XPTY0020 if `focus` may be no node; SQLCODE -900 past turborel's
/// limits.
pub fn step(focus: &Type, axis: Axis, test: &StepTest) -> Result<Type> {
    for item in prime(focus)? {
        if let Item::Atomic(_) = item {
            return Err(super::super::error::error(
                super::super::error::ErrorCode::XPTY0020,
                &format!("the context item of an axis step may be of {focus}, no node [XQueryFS §4.2.1] [XQuery §3.2.1]"),
            ));
        }
    }
    let selected = axis_of(focus, axis)?;
    let principal = axis == Axis::Attribute;
    map_items(&selected, 0, &mut |i| node_test(i, test, principal))
}

/// [XQueryFS §8.2.2.1] `axis Axis of t`.
fn axis_of(t: &Type, axis: Axis) -> Result<Type> {
    match axis {
        Axis::SelfNode => Ok(t.clone()),
        Axis::Child => map_items(t, 0, &mut |i| one(i, Basic::Child)),
        Axis::Attribute => map_items(t, 0, &mut |i| one(i, Basic::Attribute)),
        Axis::Parent => map_items(t, 0, &mut |i| one(i, Basic::Parent)),
        Axis::Descendant => closure(t, Basic::Child, false),
        Axis::DescendantOrSelf => closure(t, Basic::Child, true),
        Axis::Ancestor => closure(t, Basic::Parent, false),
        Axis::AncestorOrSelf => closure(t, Basic::Parent, true),
        Axis::FollowingSibling | Axis::PrecedingSibling => {
            let parents = axis_of(t, Axis::Parent)?;
            let children = axis_of(&Type::of_prime(prime(&parents)?, Occ::One), Axis::Child)?;
            Ok(Type::of_prime(prime(&children)?, Occ::Star))
        }
        Axis::Following | Axis::Preceding => {
            let above = axis_of(t, Axis::AncestorOrSelf)?;
            let siblings = axis_of(&Type::of_prime(prime(&above)?, Occ::One), Axis::FollowingSibling)?;
            let below = axis_of(&Type::of_prime(prime(&siblings)?, Occ::One), Axis::DescendantOrSelf)?;
            Ok(Type::of_prime(prime(&below)?, Occ::Star))
        }
    }
}

/// The axes the others are typed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Basic {
    /// `child`.
    Child,
    /// `attribute`.
    Attribute,
    /// `parent`.
    Parent,
}

/// The child, attribute or parent axis of item type `item`.
fn one(item: &Item, axis: Basic) -> Result<Type> {
    let either = || Type::occur(Type::of_prime(vec![any_element(), any_document()], Occ::One), Occ::Optional);
    Ok(match (item, axis) {
        // [XQueryFS §8.2.2.1.3] an element's children by its type's
        // expansion; a document's content, with comments and processing
        // instructions.
        (Item::Element(n), Basic::Child) => expand(&n.of, n.nillable)?.children,
        (Item::Document(content), Basic::Child) => {
            Type::All(vec![(**content).clone(), Type::of_prime(vec![Item::Pi, Item::Comment], Occ::Star)])
        }
        // [XQueryFS §8.2.2.1.4]
        (Item::Element(n), Basic::Attribute) => expand(&n.of, false)?.attributes,
        // [XQueryFS §8.2.2.1.5]
        (Item::Element(_) | Item::Text | Item::Comment | Item::Pi, Basic::Parent) => either(),
        (Item::Attribute(_), Basic::Parent) => Type::occur(Type::Item(any_element()), Occ::Optional),
        (Item::Document(_) | Item::Atomic(_), Basic::Parent)
        | (Item::Attribute(_) | Item::Text | Item::Comment | Item::Pi | Item::Atomic(_), Basic::Child)
        | (Item::Document(_) | Item::Attribute(_) | Item::Text | Item::Comment | Item::Pi | Item::Atomic(_), Basic::Attribute) => {
            Type::Empty
        }
    })
}

/// [XQueryFS §8.2.2.1.7 to §8.2.2.1.10] The closure of the child or parent
/// axis `by` of `t`, with `t`'s own item types if `and_self`: item types
/// added until one more application adds none, each once.
fn closure(t: &Type, by: Basic, and_self: bool) -> Result<Type> {
    let start = prime(t)?;
    let mut seen: Vec<Item> = if and_self { start.clone() } else { Vec::new() };
    let mut left = start;
    while let Some(item) = left.pop() {
        for next in prime(&one(&item, by)?)? {
            if !seen.contains(&next) {
                if seen.len() >= MAX_CLOSURE {
                    return Err(beyond("an axis's closure holds more item types than turborel's limit"));
                }
                seen.push(next.clone());
                left.push(next);
            }
        }
    }
    Ok(Type::of_prime(seen, Occ::Star))
}

/// [XQueryFS §8.2.3.1] `test NodeTest with PrincipalNodeKind of item`, the
/// principal node kind attribute if `attribute`, else element.
fn node_test(item: &Item, test: &StepTest, attribute: bool) -> Result<Type> {
    match test {
        StepTest::Name(namespace, local) => Ok(name_test(item, &Name { namespace: namespace.clone(), local: local.clone() }, attribute)),
        // [XQueryFS §8.2.3.1.2] node() admits each node type as it is.
        StepTest::Kind(NodeKind::Any) => Ok(Type::Item(item.clone())),
        StepTest::Kind(kind) => kind_test(item, &node_type(kind)),
    }
}

/// [XQueryFS §8.2.3.1.1] A name test of item type `item`: of a node of the
/// principal node kind, of the names both match, the most specific; of
/// any other, or of names none matches, the empty type.
fn name_test(item: &Item, test: &Name, attribute: bool) -> Type {
    let node = match (item, attribute) {
        (Item::Element(n), false) | (Item::Attribute(n), true) => n,
        (Item::Element(_) | Item::Attribute(_) | Item::Document(_) | Item::Text | Item::Comment | Item::Pi | Item::Atomic(_), _) => {
            return Type::Empty;
        }
    };
    let Some(name) = names_both(&node.name, test) else { return Type::Empty };
    let kept = Node { name, ..(**node).clone() };
    Type::Item(if attribute { Item::Attribute(Box::new(kept)) } else { Item::Element(Box::new(kept)) })
}

/// The names both `a` and `b` match, the most specific name of them, if
/// any.
fn names_both(a: &Name, b: &Name) -> Option<Name> {
    let namespace = match (&a.namespace, &b.namespace) {
        (Namespace::Any, other) | (other, Namespace::Any) => other.clone(),
        (x, y) => {
            if x != y {
                return None;
            }
            x.clone()
        }
    };
    let local = match (&a.local, &b.local) {
        (None, other) | (other, None) => other.clone(),
        (Some(x), Some(y)) => {
            if x != y {
                return None;
            }
            Some(x.clone())
        }
    };
    Some(Name { namespace, local })
}

/// [XQueryFS §8.2.3.1.2] A kind test of item type `item`, the test
/// normalized to `test`: the item type if each of its values matches;
/// optional, the test's item type if each value of that is the item's;
/// optional, of an element or attribute, what both admit, the names both
/// match and the type derived from the other; of a document type, the
/// document of its content's elements the test admits; else the empty
/// type.
fn kind_test(item: &Item, test: &Type) -> Result<Type> {
    let wanted = prime(test)?;
    let mut out = Vec::new();
    for t in &wanted {
        if item_sub(item, t, 0)? {
            return Ok(Type::Item(item.clone()));
        }
        if item_sub(t, item, 0)? {
            out.push(Type::Item(t.clone()));
            continue;
        }
        if let Some(both) = admitted(item, t)? {
            out.push(both);
        }
    }
    let optional = Type::occur(Type::choice(out), Occ::Optional);
    Ok(if prime(&optional)?.is_empty() { Type::Empty } else { optional })
}

/// What both `item` and `t` admit, of two elements, two attributes or two
/// documents that neither is a subtype of the other.
fn admitted(item: &Item, t: &Item) -> Result<Option<Type>> {
    let both = |a: &Node, b: &Node| -> Option<Node> {
        let name = names_both(&a.name, &b.name)?;
        let of = if a.of.derives_from(&b.of) {
            a.of.clone()
        } else if b.of.derives_from(&a.of) {
            b.of.clone()
        } else {
            return None;
        };
        Some(Node { name, nillable: a.nillable && b.nillable, of })
    };
    Ok(match (item, t) {
        (Item::Element(a), Item::Element(b)) => both(a, b).map(|n| Type::Item(Item::Element(Box::new(n)))),
        (Item::Attribute(a), Item::Attribute(b)) => both(a, b).map(|n| Type::Item(Item::Attribute(Box::new(n)))),
        (Item::Document(content), Item::Document(wanted)) => {
            let elements: Vec<Item> = prime(wanted)?.into_iter().filter(|i| matches!(i, Item::Element(_))).collect();
            let filtered = map_items(content, 0, &mut |i| kind_test(i, &Type::of_prime(elements.clone(), Occ::One)))?;
            if prime(&filtered)?.is_empty() { None } else { Some(Type::Item(Item::Document(Box::new(filtered)))) }
        }
        (Item::Element(_) | Item::Attribute(_) | Item::Document(_) | Item::Text | Item::Comment | Item::Pi | Item::Atomic(_), _) => None,
    })
}

/// The step of `axis` and `test` as XQuery writes it, its kind test
/// `kind-test()`, for messages.
#[must_use]
pub fn step_text(axis: Axis, test: &StepTest) -> String {
    let name = match axis {
        Axis::Child => "child",
        Axis::Descendant => "descendant",
        Axis::Attribute => "attribute",
        Axis::SelfNode => "self",
        Axis::DescendantOrSelf => "descendant-or-self",
        Axis::FollowingSibling => "following-sibling",
        Axis::Following => "following",
        Axis::Parent => "parent",
        Axis::Ancestor => "ancestor",
        Axis::PrecedingSibling => "preceding-sibling",
        Axis::Preceding => "preceding",
        Axis::AncestorOrSelf => "ancestor-or-self",
    };
    match test {
        StepTest::Name(namespace, local) => format!("{name}::{}", Name { namespace: namespace.clone(), local: local.clone() }),
        StepTest::Kind(_) => format!("{name}::kind-test()"),
    }
}
