//! The implicit operations on values typed: atomization, `data on`
//! [XQueryFS §7.2.6]; the effective boolean value, `fn:boolean` [XQueryFS
//! §7.2.4]; and the conversion of untyped values to an expected type,
//! `fs:convert-simple-operand` [XQueryFS §7.1.2], with `convert_untypedAtomic`
//! [XQueryFS §7.2.10].

use super::expand::expand;
use super::sequence::untyped_atomic;
use super::subtype::map_items;
use super::type_error;
use super::types::{Atom, Item, Occ, Type, prime, quantifier};
use crate::error::Result;
use crate::xml::AtomicType;

/// [XQueryFS §7.2.6] `fn:data` of `t`: `data on` each item type of it,
/// their prime type and quantifiers combined.
///
/// # Errors
///
/// err:XPTY0004 of a node no rule gives a typed value of, an element of
/// element-only content; SQLCODE -900 past `MAX_TYPE_DEPTH`.
pub fn data(t: &Type) -> Result<Type> {
    let mut items = Vec::new();
    let mut each = Occ::One;
    let mut first = true;
    for item in prime(t)? {
        let d = data_on(&item)?;
        let q = quantifier(&d)?;
        each = if first { q } else { each.times(q) };
        first = false;
        for i in prime(&d)? {
            if !items.contains(&i) {
                items.push(i);
            }
        }
    }
    // Of no item type: of `none`, as of fn:error, none; else empty.
    if first {
        return Ok(if quantifier(t)?.zero() { Type::Empty } else { Type::None });
    }
    Ok(Type::of_prime(items, each.times(quantifier(t)?)))
}

/// [XQueryFS §7.2.6] `data on` item type `item`: an atomic type itself; a
/// comment or processing instruction `xs:string`; a text or document node
/// `xdt:untypedAtomic`; an element or attribute its type's typed value.
fn data_on(item: &Item) -> Result<Type> {
    Ok(match item {
        Item::Atomic(_) => Type::Item(item.clone()),
        Item::Comment | Item::Pi => Type::atomic(AtomicType::String),
        Item::Text | Item::Document(_) => untyped_atomic(),
        Item::Element(n) | Item::Attribute(n) => {
            let typed = expand(&n.of, false)?.data;
            if typed == Type::None {
                let why = format!("{item} has element-only content, so no typed value [XQueryFS §7.2.6]");
                return Err(type_error(&why));
            }
            typed
        }
    })
}

/// [XQueryFS §7.2.4] The check of `fn:boolean` of `t`: of the empty
/// sequence, nodes, or one `xs:boolean`, `xs:string`, `xdt:untypedAtomic`
/// or number, each alternative of a choice by itself.
///
/// # Errors
///
/// err:XPTY0004 if `t` is none of them; SQLCODE -900 past
/// `MAX_TYPE_DEPTH`.
pub fn boolean(t: &Type) -> Result<()> {
    if has_boolean(t)? {
        return Ok(());
    }
    if let Type::Choice(alternatives) = t {
        for a in alternatives {
            boolean(a)?;
        }
        return Ok(());
    }
    Err(type_error(&format!("the effective boolean value of {t} is not defined [XQueryFS §7.2.4]")))
}

/// Whether `t <: (empty | NodeType+ | xs:boolean | xs:string |
/// xdt:untypedAtomic | fs:numeric)` by its prime type and quantifier.
fn has_boolean(t: &Type) -> Result<bool> {
    let items = prime(t)?;
    let node = |i: &Item| !matches!(i, Item::Atomic(_));
    if items.iter().all(node) {
        return Ok(true);
    }
    let single = |i: &Item| match i {
        Item::Atomic(Atom::Numeric) => true,
        Item::Atomic(a @ (Atom::Of(_) | Atom::Defined(_))) => a.built_in().is_some_and(|a| {
            [AtomicType::Boolean, AtomicType::String, AtomicType::UntypedAtomic].iter().any(|b| a.derives_from(*b))
                || a.primitive().numeric()
        }),
        Item::Atomic(Atom::Any) | Item::Element(_) | Item::Attribute(_) | Item::Document(_) | Item::Text | Item::Comment | Item::Pi => {
            false
        }
    };
    Ok(items.iter().all(single) && !quantifier(t)?.many())
}

/// [XQueryFS §7.1.2, §7.2.10] `convert_untypedAtomic(t, to)`: `t` with
/// each `xdt:untypedAtomic` in it `to`.
///
/// # Errors
///
/// SQLCODE -900 past `MAX_TYPE_DEPTH`.
pub fn convert_untyped(t: &Type, to: &Type) -> Result<Type> {
    map_items(t, 0, &mut |i| Ok(if *i == Item::Atomic(Atom::Of(AtomicType::UntypedAtomic)) { to.clone() } else { Type::Item(i.clone()) }))
}
