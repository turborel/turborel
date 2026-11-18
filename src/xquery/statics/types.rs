//! The types of the static typing [XQueryFS §2.4]: item types, the
//! content models built of them, and the prime types and quantifiers of
//! [XQueryFS §8.4] that approximate a type by a choice of item types and
//! an occurrence indicator.
//!
//! \[DESIGN\] RULES.md R-CF-6: a type nests as the query and the schemas
//! it reads nest, so each walk of one counts its depth and is refused past
//! `MAX_TYPE_DEPTH`.

use std::fmt;

use super::super::plan::Namespace;
use super::super::types::SchemaType;
use super::names::{Narrowed, except_written, namespaces_written};
use crate::error::Result;
use crate::xml::AtomicType;

/// \[DESIGN\] How deep a type nests at most (RULES.md R-CF-6): a type is
/// built of the query's expressions, bounded by `xquery::MAX_NESTING`, and
/// of content models, bounded by `xml::schema::MAX_DEPTH`; past this a
/// walk of it is SQLCODE -900 (D18). The value is no measurement: it is
/// past both bounds together.
pub const MAX_TYPE_DEPTH: usize = 256;

/// The error past `MAX_TYPE_DEPTH`.
pub fn deeper(depth: usize) -> Result<usize> {
    let next = depth.saturating_add(1);
    if next > MAX_TYPE_DEPTH {
        return Err(super::beyond("a static type nests deeper than turborel's limit"));
    }
    Ok(next)
}

/// An occurrence indicator [XQueryFS §2.4.3]: `One` the auxiliary 1 of
/// [XQueryFS §8.4].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occ {
    /// Exactly one.
    One,
    /// `?`.
    Optional,
    /// `*`.
    Star,
    /// `+`.
    Plus,
}

impl Occ {
    /// Of whether it allows none and more than one.
    fn of(allows: (bool, bool)) -> Occ {
        match allows {
            (false, false) => Occ::One,
            (true, false) => Occ::Optional,
            (true, true) => Occ::Star,
            (false, true) => Occ::Plus,
        }
    }

    /// Whether it allows no item.
    #[must_use]
    pub fn zero(self) -> bool {
        matches!(self, Occ::Optional | Occ::Star)
    }

    /// Whether it allows more than one item.
    #[must_use]
    pub fn many(self) -> bool {
        matches!(self, Occ::Star | Occ::Plus)
    }

    /// [XQueryFS §8.4] The choice `|` and the product `·` of two, which the
    /// tables give alike: none if either allows none, many if either many.
    #[must_use]
    pub fn times(self, other: Occ) -> Occ {
        Occ::of((self.zero() || other.zero(), self.many() || other.many()))
    }

    /// [XQueryFS §8.5.1] `<=`: whether each number of items it allows the
    /// other allows.
    #[must_use]
    pub fn within(self, other: Occ) -> bool {
        (!self.zero() || other.zero()) && (!self.many() || other.many())
    }
}

/// An atomic type name [XQueryFS §2.4.2, §3.5.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Atom {
    /// `xdt:anyAtomicType`.
    Any,
    /// [XQueryFS §3.5.1] `fs:numeric`, `xs:decimal | xs:float | xs:double`.
    Numeric,
    /// A built-in atomic type.
    Of(AtomicType),
}

/// The name of an element or attribute type [XQueryFS §2.4.2]: a QName, or
/// a wildcard of its namespace, its local name, or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    /// Its namespace, `Any` of `*` and `*:NCName`.
    pub namespace: Namespace,
    /// Its local name, none of `*` and `Prefix:*`.
    pub local: Option<Box<str>>,
    /// \[EXTENSION\] What narrows it past these, of a schema's wildcard
    /// (`names.rs`).
    pub narrowed: Option<Box<Narrowed>>,
}

impl Name {
    /// `*`.
    pub const ANY: Name = Name { namespace: Namespace::Any, local: None, narrowed: None };

    /// The name `local` in the namespace `uri`, none for none.
    #[must_use]
    pub fn new(uri: Option<&str>, local: &str) -> Name {
        Name { namespace: Namespace::of(uri.map(Into::into)), local: Some(local.into()), narrowed: None }
    }

    /// The name a name test of `namespace` and `local` matches.
    #[must_use]
    pub fn tested(namespace: &Namespace, local: Option<&str>) -> Name {
        Name { namespace: namespace.clone(), local: local.map(Into::into), narrowed: None }
    }
}

/// An element or attribute type [XQueryFS §2.4.2]: its name, whether it is
/// nillable, and its type reference. A reference to a global declaration
/// is held resolved to the name, nillable and type it declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Its name.
    pub name: Name,
    /// Whether it is nillable; never of an attribute.
    pub nillable: bool,
    /// Its type reference: `of type T`.
    pub of: SchemaType,
}

/// An item type [XQueryFS §2.4.2].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// An atomic type.
    Atomic(Atom),
    /// `element`.
    Element(Box<Node>),
    /// `attribute`.
    Attribute(Box<Node>),
    /// `document { Type }`.
    Document(Box<Type>),
    /// `text`.
    Text,
    /// `comment`.
    Comment,
    /// `processing-instruction`.
    Pi,
}

/// A type [XQueryFS §2.4.3].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    /// `empty`, the empty sequence's.
    Empty,
    /// `none`, of no value.
    None,
    /// An item type.
    Item(Item),
    /// `,`: a sequence.
    Seq(Vec<Type>),
    /// `|`: a choice.
    Choice(Vec<Type>),
    /// `&`: an interleaving.
    All(Vec<Type>),
    /// A type and an occurrence indicator other than one.
    Occur(Box<Type>, Occ),
}

impl Type {
    /// `t` with occurrence `occ`; a choice of no items' empty or none.
    #[must_use]
    pub fn occur(t: Type, occ: Occ) -> Type {
        match (t, occ) {
            (t, Occ::One) => t,
            (Type::Empty, _) => Type::Empty,
            (Type::None, o) => {
                if o.zero() {
                    Type::Empty
                } else {
                    Type::None
                }
            }
            (Type::Occur(inner, o), outer) => Type::Occur(inner, o.times(outer)),
            (t, o) => Type::Occur(Box::new(t), o),
        }
    }

    /// The type of an atomic type.
    #[must_use]
    pub fn atomic(t: AtomicType) -> Type {
        Type::Item(Item::Atomic(Atom::Of(t)))
    }

    /// `xs:boolean`.
    #[must_use]
    pub fn boolean() -> Type {
        Type::atomic(AtomicType::Boolean)
    }

    /// [XQueryFS §8.4] `PrimeType · Occurrence` of `items`: their choice,
    /// of none `empty` if `occ` allows none, else `none`.
    #[must_use]
    pub fn of_prime(items: Vec<Item>, occ: Occ) -> Type {
        let mut items = items;
        let t = match items.len() {
            0 => return if occ.zero() { Type::Empty } else { Type::None },
            1 => items.pop().map_or(Type::None, Type::Item),
            _ => Type::Choice(items.into_iter().map(Type::Item).collect()),
        };
        Type::occur(t, occ)
    }

    /// A choice of `types`.
    #[must_use]
    pub fn choice(types: Vec<Type>) -> Type {
        let mut types = types;
        if types.len() == 1 { types.pop().unwrap_or(Type::None) } else { Type::Choice(types) }
    }

    /// A sequence of `types`.
    #[must_use]
    pub fn seq(types: Vec<Type>) -> Type {
        let mut types = types;
        if types.len() == 1 { types.pop().unwrap_or(Type::Empty) } else { Type::Seq(types) }
    }
}

/// [XQueryFS §8.4] prime(`t`): the choice of the item types in it, each
/// once, in the order met.
///
/// # Errors
///
/// SQLCODE -900 past `MAX_TYPE_DEPTH`.
pub fn prime(t: &Type) -> Result<Vec<Item>> {
    let mut out = Vec::new();
    let mut left: Vec<(&Type, usize)> = vec![(t, 0)];
    while let Some((at, depth)) = left.pop() {
        match at {
            Type::Empty | Type::None => {}
            Type::Item(i) => {
                if !out.contains(i) {
                    out.push(i.clone());
                }
            }
            Type::Seq(ts) | Type::Choice(ts) | Type::All(ts) => {
                let next = deeper(depth)?;
                left.extend(ts.iter().rev().map(|part| (part, next)));
            }
            Type::Occur(inner, _) => left.push((inner, deeper(depth)?)),
        }
    }
    Ok(out)
}

/// [XQueryFS §8.4] quantifier(`t`).
///
/// \[DESIGN\] [XQuery §5.2.3.1] [XQueryFS §6.1.1] a static typing
/// extension, implementation-defined: the quantifier is that of the least
/// and most items `t`'s values hold, each none, one or many, so the empty
/// type adds none to a sequence, where [XQueryFS §8.4]'s `?` of `empty`
/// makes `(empty, Type)` of `+`; it is within \[XQueryFS\]'s, so a type
/// it gives is a subtype of the one \[XQueryFS\] gives.
///
/// # Errors
///
/// SQLCODE -900 past `MAX_TYPE_DEPTH`.
pub fn quantifier(t: &Type) -> Result<Occ> {
    let (least, most) = count(t, 0)?;
    Ok(Occ::of((least == NONE, most == MANY)))
}

/// No item, of `count`.
const NONE: u8 = 0;
/// One item.
const ONE: u8 = 1;
/// Many items.
const MANY: u8 = 2;

/// The least and most items a value of `t` holds, each `NONE`, `ONE` or
/// `MANY`, at `depth`.
fn count(t: &Type, depth: usize) -> Result<(u8, u8)> {
    let next = deeper(depth)?;
    Ok(match t {
        Type::Item(_) | Type::None => (ONE, ONE),
        Type::Empty => (NONE, NONE),
        Type::Seq(ts) | Type::All(ts) => {
            let mut acc = (NONE, NONE);
            for part in ts {
                let (least, most) = count(part, next)?;
                acc = (acc.0.saturating_add(least).min(MANY), acc.1.saturating_add(most).min(MANY));
            }
            acc
        }
        Type::Choice(ts) => {
            let mut acc: Option<(u8, u8)> = None;
            for part in ts {
                let (least, most) = count(part, next)?;
                acc = Some(acc.map_or((least, most), |(l, m)| (l.min(least), m.max(most))));
            }
            acc.unwrap_or((NONE, NONE))
        }
        Type::Occur(inner, o) => {
            let (least, most) = count(inner, next)?;
            let (low, high) = (if o.zero() { NONE } else { ONE }, if o.many() { MANY } else { ONE });
            (least.saturating_mul(low).min(MANY), most.saturating_mul(high).min(MANY))
        }
    })
}

/// `prime(t) · quantifier(t)` [XQueryFS §8.4].
///
/// # Errors
///
/// SQLCODE -900 past `MAX_TYPE_DEPTH`.
pub fn factored(t: &Type) -> Result<Type> {
    Ok(Type::of_prime(prime(t)?, quantifier(t)?))
}

/// Whether `t` is the empty type, `t <: empty` with no item type in it, of
/// a value none or empty, as [XQuery §2.3.1] err:XPST0005 asks; `none`
/// alone, of what never returns, is not.
///
/// # Errors
///
/// SQLCODE -900 past `MAX_TYPE_DEPTH`.
pub fn is_empty(t: &Type) -> Result<bool> {
    Ok(prime(t)?.is_empty() && quantifier(t)?.zero())
}

impl fmt::Display for Occ {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Occ::One => "",
            Occ::Optional => "?",
            Occ::Star => "*",
            Occ::Plus => "+",
        })
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.namespace {
            // `*:NCName`, and `*` alone of no local name.
            Namespace::Any => match self.narrowed.as_ref().and_then(|n| n.namespaces.as_ref()) {
                Some(set) => f.write_str(&namespaces_written(set))?,
                None => {
                    if self.local.is_some() {
                        f.write_str("*:")?;
                    }
                }
            },
            Namespace::Absent => {}
            Namespace::Uri(uri) => write!(f, "{{{uri}}}")?,
        }
        f.write_str(self.local.as_deref().unwrap_or("*"))?;
        f.write_str(&except_written(self))
    }
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Item::Atomic(Atom::Any) => f.write_str("xdt:anyAtomicType"),
            Item::Atomic(Atom::Numeric) => f.write_str("fs:numeric"),
            Item::Atomic(Atom::Of(t)) => f.write_str(t.name()),
            Item::Element(n) => write!(f, "element {}{} of type {}", n.name, if n.nillable { " nillable" } else { "" }, n.of),
            Item::Attribute(n) => write!(f, "attribute {} of type {}", n.name, n.of),
            Item::Document(_) => f.write_str("document"),
            Item::Text => f.write_str("text"),
            Item::Comment => f.write_str("comment"),
            Item::Pi => f.write_str("processing-instruction"),
        }
    }
}

/// \[DESIGN\] How deep a type is written in a message, deeper parts written
/// `...`, so a message stays short and its writing is bounded (RULES.md
/// R-CF-6). The value is no measurement.
const SHOWN_DEPTH: usize = 6;

impl fmt::Display for Type {
    /// The type in the notation of [XQueryFS §2.4], cut `SHOWN_DEPTH` deep.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        show(self, 0, f)
    }
}

/// `t` written at `depth`.
fn show(t: &Type, depth: usize, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if depth > SHOWN_DEPTH {
        return f.write_str("...");
    }
    let next = depth.saturating_add(1);
    let joined = |out: &mut fmt::Formatter<'_>, ts: &[Type], by: &str| -> fmt::Result {
        out.write_str("(")?;
        for (at, part) in ts.iter().enumerate() {
            if at > 0 {
                out.write_str(by)?;
            }
            show(part, next, out)?;
        }
        out.write_str(")")
    };
    match t {
        Type::Empty => f.write_str("empty"),
        Type::None => f.write_str("none"),
        Type::Item(Item::Document(content)) => {
            f.write_str("document { ")?;
            show(content, next, f)?;
            f.write_str(" }")
        }
        Type::Item(i) => write!(f, "{i}"),
        Type::Seq(ts) => joined(f, ts, ", "),
        Type::Choice(ts) => joined(f, ts, " | "),
        Type::All(ts) => joined(f, ts, " & "),
        Type::Occur(inner, o) => {
            f.write_str("(")?;
            show(inner, next, f)?;
            write!(f, "){o}")
        }
    }
}
