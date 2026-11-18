//! Subtyping [XQueryFS §8.3.2] and type promotion [XQueryFS §8.5.1].
//!
//! \[DESIGN\] RULES.md D106: \[XQueryFS\] defines `Type1 <: Type2` by the
//! values that match each, and leaves its computation to automata. The
//! types the rules compare a type with are of few shapes, so each is
//! decided by its own: a choice of item types and an occurrence indicator,
//! `prime · quantifier`, exactly, by the item types and the occurrence of
//! the type compared; a sequence of attributes then other nodes, as an
//! element constructor's content is, by the order of the attributes in it
//! too; an interleaving of item types none of which another matches, as a
//! document test's content is, part by part. A choice is a subtype if
//! each of its alternatives is. A comparison of any other shape is false,
//! so the analysis rejects, never admits, what it cannot decide.

use super::super::plan::Namespace;
use super::types::{Atom, Item, Name, Occ, Type, deeper, prime, quantifier};
use crate::error::Result;
use crate::xml::{AtomicType, Primitive};

/// `a <: b` [XQueryFS §8.3.2].
///
/// # Errors
///
/// SQLCODE -900 past `MAX_TYPE_DEPTH`.
pub fn subtype(a: &Type, b: &Type) -> Result<bool> {
    subtype_at(a, b, 0)
}

/// `a <: b` at `depth`.
fn subtype_at(a: &Type, b: &Type, depth: usize) -> Result<bool> {
    let next = deeper(depth)?;
    if let Type::Choice(alternatives) = a {
        for x in alternatives {
            if !subtype_at(x, b, next)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if let Some((items, occ)) = factor(b, next)? {
        return within(a, &items, occ, next, item_sub);
    }
    match b {
        Type::Seq(parts) => attributes_then_nodes(a, parts, next),
        Type::All(parts) => interleaved(a, parts, next),
        Type::Choice(alternatives) => {
            for y in alternatives {
                if subtype_at(a, y, next)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Type::Empty | Type::None | Type::Item(_) | Type::Occur(..) => Ok(false),
    }
}

/// `b` as `prime · quantifier`, if it is exactly that [XQueryFS §8.4]:
/// an item type, the empty or none type, a choice of item types and the
/// empty type, or such a type with an occurrence indicator.
fn factor(b: &Type, depth: usize) -> Result<Option<(Vec<Item>, Occ)>> {
    let next = deeper(depth)?;
    Ok(match b {
        Type::Empty => Some((Vec::new(), Occ::Optional)),
        Type::None => Some((Vec::new(), Occ::One)),
        Type::Item(i) => Some((vec![i.clone()], Occ::One)),
        Type::Occur(inner, o) => factor(inner, next)?.map(|(items, q)| (items, q.times(*o))),
        Type::Seq(parts) | Type::All(parts) => match parts.as_slice() {
            [one] => factor(one, next)?,
            _ => None,
        },
        Type::Choice(alternatives) => {
            let mut items = Vec::new();
            let mut occ = Occ::One;
            for alternative in alternatives {
                let Some((more, q)) = factor(alternative, next)? else { return Ok(None) };
                if q.many() && alternatives.len() > 1 {
                    return Ok(None);
                }
                occ = occ.times(q);
                items.extend(more);
            }
            Some((items, occ))
        }
    })
}

/// Whether `a`'s quantifier is within `occ` and each item type of it is
/// `related` to one of `items`.
fn within(a: &Type, items: &[Item], occ: Occ, depth: usize, related: fn(&Item, &Item, usize) -> Result<bool>) -> Result<bool> {
    if !quantifier(a)?.within(occ) {
        return Ok(false);
    }
    for x in prime(a)? {
        if !any_of(&x, items, depth, related)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether `x` is `related` to one of `items`.
fn any_of(x: &Item, items: &[Item], depth: usize, related: fn(&Item, &Item, usize) -> Result<bool>) -> Result<bool> {
    for y in items {
        if related(x, y, depth)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `a <: (attributes, nodes)` of a sequence `parts` of two factored types,
/// the first of attributes alone, the second of none [XQueryFS §4.7.3.1]:
/// each item type of `a` one of theirs, as many attributes and as many
/// others as each allows, and no attribute after another node.
fn attributes_then_nodes(a: &Type, parts: &[Type], depth: usize) -> Result<bool> {
    let [first, second] = parts else { return Ok(false) };
    let (Some((attributes, q1)), Some((others, q2))) = (factor(first, depth)?, factor(second, depth)?) else { return Ok(false) };
    let is_attribute = |i: &Item| matches!(i, Item::Attribute(_));
    if !attributes.iter().all(is_attribute) || others.iter().any(is_attribute) {
        return Ok(false);
    }
    let attributes_of_a = project(a, &is_attribute, depth)?;
    let others_of_a = project(a, &|i: &Item| !is_attribute(i), depth)?;
    Ok(within(&attributes_of_a, &attributes, q1, depth, item_sub)?
        && within(&others_of_a, &others, q2, depth, item_sub)?
        && !order(a, depth)?.attribute_after_node)
}

/// `a <: (p1 & p2 & ...)` of factored parts no item type of one of which
/// another's matches: each item type of `a` one of theirs, and as many of
/// each part's as it allows.
fn interleaved(a: &Type, parts: &[Type], depth: usize) -> Result<bool> {
    let mut factored = Vec::with_capacity(parts.len());
    for p in parts {
        let Some(f) = factor(p, depth)? else { return Ok(false) };
        factored.push(f);
    }
    for x in prime(a)? {
        let mut found = false;
        for (items, _) in &factored {
            found = found || any_of(&x, items, depth, item_sub)?;
        }
        if !found {
            return Ok(false);
        }
    }
    for (items, occ) in &factored {
        let of_part = project_into(a, items, depth)?;
        if !within(&of_part, items, *occ, depth, item_sub)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `a` with each item type not one of `items` made the empty type.
fn project_into(a: &Type, items: &[Item], depth: usize) -> Result<Type> {
    map_items(a, depth, &mut |x| Ok(if any_of(x, items, depth, item_sub)? { Type::Item(x.clone()) } else { Type::Empty }))
}

/// `a` with each item type `keep` refuses made the empty type.
fn project(a: &Type, keep: &dyn Fn(&Item) -> bool, depth: usize) -> Result<Type> {
    map_items(a, depth, &mut |x| Ok(if keep(x) { Type::Item(x.clone()) } else { Type::Empty }))
}

/// `t` with each item type replaced by what `f` makes of it, its
/// structure kept.
pub fn map_items(t: &Type, depth: usize, f: &mut dyn FnMut(&Item) -> Result<Type>) -> Result<Type> {
    let next = deeper(depth)?;
    let mut all = |ts: &[Type]| ts.iter().map(|x| map_items(x, next, f)).collect::<Result<Vec<_>>>();
    Ok(match t {
        Type::Empty => Type::Empty,
        Type::None => Type::None,
        Type::Item(i) => f(i)?,
        Type::Seq(ts) => Type::Seq(all(ts)?),
        Type::Choice(ts) => Type::Choice(all(ts)?),
        Type::All(ts) => Type::All(all(ts)?),
        Type::Occur(inner, o) => Type::occur(map_items(inner, next, f)?, *o),
    })
}

/// What a type holds of attributes and other nodes, in what order.
#[derive(Debug, Clone, Copy, Default)]
struct Order {
    /// Whether a value of it may hold an attribute.
    attribute: bool,
    /// Whether one may hold an item that is no attribute.
    other: bool,
    /// Whether one may hold an attribute after an item that is not.
    attribute_after_node: bool,
}

/// The `Order` of `t`.
fn order(t: &Type, depth: usize) -> Result<Order> {
    let next = deeper(depth)?;
    Ok(match t {
        Type::Empty | Type::None => Order::default(),
        Type::Item(i) => {
            let attribute = matches!(i, Item::Attribute(_));
            Order { attribute, other: !attribute, attribute_after_node: false }
        }
        Type::Seq(ts) | Type::All(ts) => {
            let orders = ts.iter().map(|x| order(x, next)).collect::<Result<Vec<_>>>()?;
            let interleaved = matches!(t, Type::All(_));
            let mut acc = Order::default();
            for (at, o) in orders.iter().enumerate() {
                let before = orders.iter().take(at).any(|p| p.other);
                let after_others = interleaved && orders.iter().enumerate().any(|(j, p)| j != at && p.other);
                acc.attribute_after_node = acc.attribute_after_node || o.attribute_after_node || (o.attribute && (before || after_others));
                acc.attribute = acc.attribute || o.attribute;
                acc.other = acc.other || o.other;
            }
            acc
        }
        Type::Choice(ts) => {
            let mut acc = Order::default();
            for x in ts {
                let o = order(x, next)?;
                acc = Order {
                    attribute: acc.attribute || o.attribute,
                    other: acc.other || o.other,
                    attribute_after_node: acc.attribute_after_node || o.attribute_after_node,
                };
            }
            acc
        }
        Type::Occur(inner, o) => {
            let i = order(inner, next)?;
            Order { attribute_after_node: i.attribute_after_node || (o.many() && i.attribute && i.other), ..i }
        }
    })
}

/// Whether item type `x` is a subtype of `y` [XQueryFS §8.3.2]: of an
/// atomic type derived from the other's; of an element or attribute type
/// whose name the other's includes and whose type derives from the
/// other's [XQueryFS §8.1.1], nillable only if the other is; of a document
/// type whose content's type is a subtype of the other's; of a node kind,
/// the same kind.
///
/// # Errors
///
/// SQLCODE -900 past `MAX_TYPE_DEPTH`.
pub fn item_sub(x: &Item, y: &Item, depth: usize) -> Result<bool> {
    Ok(match (x, y) {
        (Item::Atomic(a), Item::Atomic(b)) => atom_sub(*a, *b),
        (Item::Element(a), Item::Element(b)) => name_within(&a.name, &b.name) && a.of.derives_from(&b.of) && (!a.nillable || b.nillable),
        (Item::Attribute(a), Item::Attribute(b)) => name_within(&a.name, &b.name) && a.of.derives_from(&b.of),
        (Item::Document(a), Item::Document(b)) => subtype_at(a, b, depth)?,
        (Item::Text, Item::Text) | (Item::Comment, Item::Comment) | (Item::Pi, Item::Pi) => true,
        (Item::Atomic(_) | Item::Element(_) | Item::Attribute(_) | Item::Document(_) | Item::Text | Item::Comment | Item::Pi, _) => false,
    })
}

/// Whether atomic type `a` derives from `b` [XQueryFS §8.1.1, §3.5.1].
#[must_use]
pub fn atom_sub(a: Atom, b: Atom) -> bool {
    match (a, b) {
        (_, Atom::Any) | (Atom::Numeric, Atom::Numeric) => true,
        (Atom::Of(t), Atom::Numeric) => t.primitive().numeric(),
        (Atom::Of(t), Atom::Of(u)) => t.derives_from(u),
        (Atom::Any | Atom::Numeric, Atom::Of(_)) | (Atom::Any, Atom::Numeric) => false,
    }
}

/// Whether every name `a` matches `b` matches.
#[must_use]
pub fn name_within(a: &Name, b: &Name) -> bool {
    let namespace = b.namespace == Namespace::Any || a.namespace == b.namespace;
    let local = b.local.is_none() || (a.local.is_some() && a.local == b.local);
    namespace && local
}

/// [XQueryFS §8.5.1] `a` can be promoted to `b`: as many items as `b`
/// allows, each of a type promoted to one of `b`'s, as `prime(Type1) can
/// be promoted to prime(Type2)` and `quantifier(Type1) <=
/// quantifier(Type2)` ask; of a `b` of another shape, `a <: b`.
///
/// # Errors
///
/// SQLCODE -900 past `MAX_TYPE_DEPTH`.
pub fn can_promote(a: &Type, b: &Type) -> Result<bool> {
    match factor(b, 0)? {
        Some((items, occ)) => within(a, &items, occ, 0, promoted),
        None => subtype(a, b),
    }
}

/// Whether item type `x` can be promoted to `y` [XQueryFS §8.5.1]: it is
/// a subtype of it; or `xdt:untypedAtomic`, promoted to any atomic type;
/// or derived from `xs:decimal`, promoted to `xs:float` and `xs:double`,
/// or from `xs:float`, to `xs:double`.
fn promoted(x: &Item, y: &Item, depth: usize) -> Result<bool> {
    if item_sub(x, y, depth)? {
        return Ok(true);
    }
    let (Item::Atomic(Atom::Of(from)), Item::Atomic(to)) = (x, y) else { return Ok(false) };
    let to_primitive = match to {
        Atom::Of(t) => Some(t.primitive()),
        Atom::Any | Atom::Numeric => None,
    };
    Ok(match from.primitive() {
        Primitive::Untyped => true,
        Primitive::Decimal | Primitive::Integer => matches!(to_primitive, Some(Primitive::Float | Primitive::Double)) && is_builtin(*to),
        Primitive::Float => to_primitive == Some(Primitive::Double) && is_builtin(*to),
        Primitive::String
        | Primitive::Boolean
        | Primitive::Double
        | Primitive::Duration
        | Primitive::YearMonthDuration
        | Primitive::DayTimeDuration
        | Primitive::DateTime
        | Primitive::Time
        | Primitive::Date
        | Primitive::GYearMonth
        | Primitive::GYear
        | Primitive::GMonthDay
        | Primitive::GDay
        | Primitive::GMonth
        | Primitive::HexBinary
        | Primitive::Base64Binary
        | Primitive::AnyUri
        | Primitive::QName
        | Primitive::Notation => false,
    })
}

/// Whether `to` is `xs:float` or `xs:double` itself, to which a number is
/// promoted, not a type derived from one.
fn is_builtin(to: Atom) -> bool {
    matches!(to, Atom::Of(AtomicType::Float | AtomicType::Double))
}
