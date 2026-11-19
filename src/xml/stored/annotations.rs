//! The table of a stored node's type annotations (RULES.md D106): each
//! the number of its type's name and its typing, written once and
//! referred to by number from the events of the nodes it annotates. A
//! typing's atomic type is its tag, then the number of its user-defined
//! type's name plus 1, 0 of a built-in type.

use std::collections::HashMap;
use std::sync::Arc;

use super::{ATOMIC_TYPES, count, name};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::xml::sequence::AtomicType;
use crate::xml::tree::{Annotation, QName, Typing, ValueType};

// Typings of annotations.
/// `xs:anySimpleType`.
const ANY_SIMPLE: u8 = 0;
/// Mixed content.
const MIXED: u8 = 1;
/// Empty content.
const EMPTY: u8 = 2;
/// Element-only content.
const ELEMENT_ONLY: u8 = 3;
/// An atomic type, which follows.
const ATOMIC_TYPING: u8 = 4;
/// A list of an atomic type, which follows.
const LIST_TYPING: u8 = 5;
/// A list of items of several atomic types: their count, then each one,
/// in order.
const ITEMS_TYPING: u8 = 6;

/// The tag of atomic type `t`: its place in `ATOMIC_TYPES`.
fn tag(t: AtomicType) -> u8 {
    ATOMIC_TYPES.iter().position(|&a| a == t).and_then(|p| u8::try_from(p).ok()).unwrap_or(0)
}

/// The atomic types `typing` names, in order.
pub(super) fn value_types(typing: &Typing) -> &[ValueType] {
    match typing {
        Typing::Atomic(t) | Typing::List(t) => std::slice::from_ref(t),
        Typing::Items(kinds) => kinds,
        Typing::AnySimple | Typing::Mixed | Typing::Empty | Typing::ElementOnly => &[],
    }
}

/// Atomic type `t` written to `out`: its tag, then the number in `names`
/// of its user-defined type's name plus 1, 0 of none.
fn put_value_type(out: &mut Vec<u8>, t: &ValueType, names: &HashMap<&QName, u32>) {
    out.put_u8(tag(t.kind));
    let number = t.name.as_deref().and_then(|n| names.get(n)).map_or(0, |n| n.saturating_add(1));
    out.put_u32(number);
}

/// `typing` written to `out`: its tag, then its atomic type or types, the
/// names they hold numbered in `names`.
pub(super) fn put_typing(out: &mut Vec<u8>, typing: &Typing, names: &HashMap<&QName, u32>) {
    let typed = value_types(typing);
    match typing {
        Typing::AnySimple => out.put_u8(ANY_SIMPLE),
        Typing::Mixed => out.put_u8(MIXED),
        Typing::Empty => out.put_u8(EMPTY),
        Typing::ElementOnly => out.put_u8(ELEMENT_ONLY),
        Typing::Atomic(_) => out.put_u8(ATOMIC_TYPING),
        Typing::List(_) => out.put_u8(LIST_TYPING),
        Typing::Items(kinds) => {
            out.put_u8(ITEMS_TYPING);
            out.put_u32(count(kinds.len()));
        }
    }
    for t in typed {
        put_value_type(out, t, names);
    }
}

/// An atomic type read, its name looked up in `names`, or passed over if
/// there are none to look it up in.
fn value_type(reader: &mut Reader<'_>, names: Option<&[QName]>) -> Result<ValueType> {
    let t = reader.u8()?;
    let kind = ATOMIC_TYPES.get(usize::from(t)).copied().ok_or_else(|| reader.corrupt(&format!("an annotation's atomic type is {t}")))?;
    let number = reader.u32()?;
    let Some((index, names)) = number.checked_sub(1).zip(names) else { return Ok(kind.into()) };
    let found = usize::try_from(index)
        .ok()
        .and_then(|i| names.get(i))
        .ok_or_else(|| reader.corrupt(&format!("the name {index} is not numbered")))?;
    Ok(ValueType { kind, name: Some(Arc::new(found.clone())) })
}

/// A typing read, as `value_type` reads its atomic types.
fn typing(reader: &mut Reader<'_>, names: Option<&[QName]>) -> Result<Typing> {
    let tag = reader.u8()?;
    Ok(match tag {
        ANY_SIMPLE => Typing::AnySimple,
        MIXED => Typing::Mixed,
        EMPTY => Typing::Empty,
        ELEMENT_ONLY => Typing::ElementOnly,
        ATOMIC_TYPING => Typing::Atomic(value_type(reader, names)?),
        LIST_TYPING => Typing::List(value_type(reader, names)?),
        ITEMS_TYPING => {
            let mut kinds = Vec::new();
            for _ in 0..reader.u32()? {
                kinds.push(value_type(reader, names)?);
            }
            Typing::Items(kinds.into())
        }
        _ => return Err(reader.corrupt(&format!("an annotation's typing is {tag}"))),
    })
}

/// A node's type annotations read, their names numbered in `names`.
pub(super) fn decode_annotations(reader: &mut Reader<'_>, names: &[QName]) -> Result<Vec<Annotation>> {
    let mut out = Vec::new();
    for _ in 0..reader.u32()? {
        let type_name = name(reader, names)?.clone();
        out.push(Annotation { name: type_name, typing: typing(reader, Some(names))? });
    }
    Ok(out)
}

/// A node's table of type annotations passed over, as a reader of its
/// events alone does (`stored/events.rs`).
///
/// # Errors
///
/// SQLCODE -916 if it is cut short or holds a typing of no tag.
pub(super) fn skip_annotations(reader: &mut Reader<'_>) -> Result<()> {
    for _ in 0..reader.u32()? {
        reader.u32()?;
        typing(reader, None)?;
    }
    Ok(())
}
