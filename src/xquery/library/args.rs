//! The function conversion rules [XQuery §3.1.5], as each function applies
//! them to its arguments: atomization where a parameter is atomic
//! [XQueryFO §2.4], an untyped value cast to the parameter's type, a
//! number promoted [XQuery Appendix B.1], a URI promoted to a string; and
//! the values a function returns.

use super::super::collation::Collator;
use super::super::error::{ErrorCode, beyond, error};
use super::super::functions::Context;
use super::decimal::Decimal;
use super::{MAX_ITEMS, MAX_STRING};
use crate::error::{Result, SqlError};
use crate::xml::{Atomic, AtomicType, Item, Node, Primitive, Sequence, cast, double as double_value};

/// The type error of an argument that is not `expected`, `why`
/// [XQuery §3.1.5].
pub fn wrong_type(expected: &str, why: &str) -> SqlError {
    error(ErrorCode::XPTY0004, &format!("an argument is not {expected}: {why} [XQuery §3.1.5]"))
}

/// Argument `n` of `args`, the empty sequence if it is not given.
pub fn arg(args: &[Sequence], n: usize) -> &Sequence {
    const NONE: &Sequence = &Sequence::empty();
    args.get(n).unwrap_or(NONE)
}

/// [XQueryFO §2.4] The atomic values of `value`: each atomic value itself,
/// each node its typed value.
pub fn atomized(value: &Sequence) -> Vec<Atomic> {
    atomize(value.items())
}

/// [XQueryFO §2.4] [XQuery §2.4.2] `items` atomized: each atomic value
/// itself, each node its typed value.
pub fn atomize(items: &[Item]) -> Vec<Atomic> {
    items
        .iter()
        .map(|item| match item {
            Item::Atomic(a) => a.clone(),
            Item::Node(node) => node.typed_value(),
        })
        .collect()
}

/// `value` atomized, as a parameter of `xdt:anyAtomicType?` takes it.
///
/// # Errors
///
/// err:XPTY0004 of more than one value.
pub fn optional_atomic(value: &Sequence) -> Result<Option<Atomic>> {
    let mut values = atomized(value);
    if values.len() > 1 {
        return Err(wrong_type("one atomic value", &format!("it is {} of them", values.len())));
    }
    Ok(values.pop())
}

/// `value` atomized, as a parameter of `xdt:anyAtomicType` takes it.
///
/// # Errors
///
/// err:XPTY0004 of none, or more than one.
pub fn atomic(value: &Sequence) -> Result<Atomic> {
    optional_atomic(value)?.ok_or_else(|| wrong_type("one atomic value", "it is the empty sequence"))
}

/// The characters of `a` as a parameter of `xs:string` takes it: a string
/// or a value of a type derived from it, an untyped value cast, a URI
/// promoted [XQuery Appendix B.1].
///
/// # Errors
///
/// err:XPTY0004 of a value of another type.
pub fn text_of(a: &Atomic) -> Result<String> {
    match a.kind.primitive() {
        Primitive::String | Primitive::Untyped | Primitive::AnyUri => Ok(a.lexical.to_string()),
        Primitive::Boolean
        | Primitive::Decimal
        | Primitive::Integer
        | Primitive::Float
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
        | Primitive::QName
        | Primitive::Notation => Err(wrong_type("an xs:string", &format!("it is an {}", a.kind.name()))),
    }
}

/// `value` as a parameter of `xs:string?` takes it.
///
/// # Errors
///
/// err:XPTY0004 of more than one value, or one not a string.
pub fn optional_string(value: &Sequence) -> Result<Option<String>> {
    optional_atomic(value)?.map(|a| text_of(&a)).transpose()
}

/// `value` as a parameter of `xs:string?` takes it, the empty sequence the
/// zero-length string, as most functions of strings read it [XQueryFO
/// §7.4].
///
/// # Errors
///
/// As `optional_string`.
pub fn string_or_empty(value: &Sequence) -> Result<String> {
    Ok(optional_string(value)?.unwrap_or_default())
}

/// `value` as a parameter of `xs:string` takes it.
///
/// # Errors
///
/// err:XPTY0004 of no value, more than one, or one not a string.
pub fn string(value: &Sequence) -> Result<String> {
    text_of(&atomic(value)?)
}

/// `value` as a parameter of `xs:string*` takes it.
///
/// # Errors
///
/// err:XPTY0004 of a value not a string.
pub fn strings(value: &Sequence) -> Result<Vec<String>> {
    atomized(value).iter().map(text_of).collect()
}

/// `a` as a parameter of `xs:integer` takes it: an integer, or an untyped
/// value cast to one.
///
/// # Errors
///
/// err:XPTY0004 of a value of another type; err:FORG0001 of an untyped
/// value that is no integer.
pub fn integer_of(a: &Atomic) -> Result<Decimal> {
    let integer = match a.kind.primitive() {
        Primitive::Integer => a.clone(),
        Primitive::Untyped => cast(a, AtomicType::Integer)?,
        Primitive::String
        | Primitive::Boolean
        | Primitive::Decimal
        | Primitive::Float
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
        | Primitive::Notation => return Err(wrong_type("an xs:integer", &format!("it is an {}", a.kind.name()))),
    };
    Decimal::parse(&integer.lexical).ok_or_else(|| wrong_type("an xs:integer", &format!("'{}' is none", integer.lexical)))
}

/// `value` as a parameter of `xs:integer` takes it.
///
/// # Errors
///
/// As `atomic` and `integer_of`.
pub fn integer(value: &Sequence) -> Result<Decimal> {
    integer_of(&atomic(value)?)
}

/// `a` as a parameter of `xs:double` takes it: a number promoted, or an
/// untyped value cast [XQuery Appendix B.1].
///
/// # Errors
///
/// err:XPTY0004 of a value of another type; err:FORG0001 of an untyped
/// value that is no double.
pub fn double_of(a: &Atomic) -> Result<f64> {
    let double = match a.kind.primitive() {
        Primitive::Double | Primitive::Float => a.clone(),
        Primitive::Decimal | Primitive::Integer | Primitive::Untyped => cast(a, AtomicType::Double)?,
        Primitive::String
        | Primitive::Boolean
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
        | Primitive::Notation => return Err(wrong_type("an xs:double", &format!("it is an {}", a.kind.name()))),
    };
    double_value(&double.lexical).ok_or_else(|| wrong_type("an xs:double", &format!("'{}' is none", double.lexical)))
}

/// `value` as a parameter of `xs:double` takes it.
///
/// # Errors
///
/// As `atomic` and `double_of`.
pub fn double(value: &Sequence) -> Result<f64> {
    double_of(&atomic(value)?)
}

/// `a` as a parameter of the atomic type `expected` takes it: a value of
/// that type or of one derived from it, or an untyped value cast to it.
///
/// # Errors
///
/// err:XPTY0004 of a value of another type; the error of casting an
/// untyped value that is none of `expected`.
pub fn as_type(a: Atomic, expected: AtomicType) -> Result<Atomic> {
    if a.kind.derives_from(expected) {
        Ok(a)
    } else if a.kind == AtomicType::UntypedAtomic {
        cast(&a, expected)
    } else {
        Err(wrong_type(&format!("an {}", expected.name()), &format!("it is an {}", a.kind.name())))
    }
}

/// `value` as a parameter of the atomic type `expected` with `?` takes it.
///
/// # Errors
///
/// As `optional_atomic` and `as_type`.
pub fn optional_of(value: &Sequence, expected: AtomicType) -> Result<Option<Atomic>> {
    optional_atomic(value)?.map(|a| as_type(a, expected)).transpose()
}

/// `value` as a parameter of the atomic type `expected` takes it.
///
/// # Errors
///
/// As `atomic` and `as_type`.
pub fn one_of(value: &Sequence, expected: AtomicType) -> Result<Atomic> {
    as_type(atomic(value)?, expected)
}

/// `value` as a parameter of `node()?` takes it.
///
/// # Errors
///
/// err:XPTY0004 of more than one item, or an atomic value.
pub fn optional_node(value: &Sequence) -> Result<Option<Node>> {
    match value.items() {
        [] => Ok(None),
        [Item::Node(node)] => Ok(Some(node.clone())),
        [Item::Atomic(a)] => Err(wrong_type("a node", &format!("it is an {}", a.kind.name()))),
        [_, ..] => Err(wrong_type("one node", &format!("it is {} items", value.items().len()))),
    }
}

/// `value` as a parameter of `node()` takes it.
///
/// # Errors
///
/// err:XPTY0004 of anything but one node.
pub fn node(value: &Sequence) -> Result<Node> {
    optional_node(value)?.ok_or_else(|| wrong_type("a node", "it is the empty sequence"))
}

/// The context item of `focus` [XQuery §2.1.2].
///
/// # Errors
///
/// err:FONC0001 if there is none.
pub fn context_item<'a>(focus: &'a Context<'_>) -> Result<&'a Item> {
    focus.item().ok_or_else(|| error(ErrorCode::FONC0001, "the context item is undefined"))
}

/// The collation of `focus` the optional argument `n` of `args` names,
/// else its default collation [XQueryFO §7.3.1].
///
/// # Errors
///
/// err:FOCH0002 if the argument names no collation of the statically
/// known collations; err:XPTY0004 if it is no string.
pub fn collator(focus: &Context<'_>, args: &[Sequence], n: usize) -> Result<Collator> {
    let Some(given) = args.get(n) else { return Ok(focus.collation().clone()) };
    let uri = string(given)?;
    focus
        .collation_named(&uri)
        .ok_or_else(|| error(ErrorCode::FOCH0002, &format!("the collation '{uri}' is not supported [XQueryFO §7.3.1]")))
}

/// The context item of `focus`, a node.
///
/// # Errors
///
/// err:FONC0001 if there is none; err:FOTY0011 if it is no node.
pub fn context_node(focus: &Context<'_>) -> Result<Node> {
    match context_item(focus)? {
        Item::Node(node) => Ok(node.clone()),
        Item::Atomic(a) => Err(error(ErrorCode::FOTY0011, &format!("the context item is an {}, not a node", a.kind.name()))),
    }
}

/// The argument `n` of `args` if given, else the context item, as a
/// sequence: what a function whose argument defaults to `.` reads.
///
/// # Errors
///
/// err:FONC0001 if it is not given and there is no context item.
pub fn or_context(args: &[Sequence], n: usize, focus: &Context<'_>) -> Result<Sequence> {
    match args.get(n) {
        Some(given) => Ok(given.clone()),
        None => Ok(Sequence::new(vec![context_item(focus)?.clone()])),
    }
}

/// The argument `n` of `args` if given, else the context item, which must
/// be a node, as a sequence.
///
/// # Errors
///
/// err:FONC0001 if it is not given and there is no context item;
/// err:FOTY0011 if it is not given and the context item is no node.
pub fn or_context_node(args: &[Sequence], n: usize, focus: &Context<'_>) -> Result<Sequence> {
    match args.get(n) {
        Some(given) => Ok(given.clone()),
        None => Ok(Sequence::new(vec![Item::Node(context_node(focus)?)])),
    }
}

/// The sequence of `a` alone.
pub fn single(a: Atomic) -> Sequence {
    Sequence::new(vec![Item::Atomic(a)])
}

/// The sequence of `a`, or the empty sequence.
pub fn optional(a: Option<Atomic>) -> Sequence {
    Sequence::new(a.map(Item::Atomic).into_iter().collect())
}

/// The `xs:boolean` `b`.
pub fn boolean(b: bool) -> Sequence {
    single(Atomic::new(AtomicType::Boolean, if b { "true" } else { "false" }))
}

/// The `xs:integer` `n`.
pub fn integer_value(n: &Decimal) -> Atomic {
    Atomic::new(AtomicType::Integer, n.text())
}

/// The value of `kind` of `text`, which must be its canonical form.
///
/// # Errors
///
/// SQLCODE -900 if `text` is longer than `MAX_STRING`.
pub fn typed(kind: AtomicType, text: String) -> Result<Atomic> {
    within_string(text.len())?;
    Ok(Atomic::new(kind, text))
}

/// The error if a string of `length` octets would be longer than
/// `MAX_STRING`.
///
/// # Errors
///
/// SQLCODE -900 if it would.
pub fn within_string(length: usize) -> Result<()> {
    if length > MAX_STRING { Err(beyond(&format!("a string of {length} octets is longer than {MAX_STRING}"))) } else { Ok(()) }
}

/// The `xs:string` `text`.
///
/// # Errors
///
/// As `typed`.
pub fn string_value(text: String) -> Result<Sequence> {
    Ok(single(typed(AtomicType::String, text)?))
}

/// The sequence of `items`.
///
/// # Errors
///
/// SQLCODE -900 if they are more than `MAX_ITEMS`.
pub fn items(items: Vec<Item>) -> Result<Sequence> {
    within_items(items.len())?;
    Ok(Sequence::new(items))
}

/// The error if a sequence of `count` items would be more than
/// `MAX_ITEMS`.
///
/// # Errors
///
/// SQLCODE -900 if it would.
pub fn within_items(count: usize) -> Result<()> {
    if count > MAX_ITEMS { Err(beyond(&format!("a sequence of {count} items is longer than {MAX_ITEMS}"))) } else { Ok(()) }
}

#[cfg(test)]
mod tests;
