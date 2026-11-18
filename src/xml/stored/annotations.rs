//! The table of a stored node's type annotations (RULES.md D106): each
//! the number of its type's name and its typing, written once and
//! referred to by number from the events of the nodes it annotates.

use super::{ATOMIC_TYPES, count, name};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::xml::sequence::AtomicType;
use crate::xml::tree::{Annotation, QName, Typing};

// Typings of annotations.
/// `xs:anySimpleType`.
const ANY_SIMPLE: u8 = 0;
/// Mixed content.
const MIXED: u8 = 1;
/// Empty content.
const EMPTY: u8 = 2;
/// Element-only content.
const ELEMENT_ONLY: u8 = 3;
/// An atomic type, whose tag follows.
const ATOMIC_TYPING: u8 = 4;
/// A list of an atomic type, whose tag follows.
const LIST_TYPING: u8 = 5;
/// A list of items of several atomic types: their count, then each one's
/// tag, in order.
const ITEMS_TYPING: u8 = 6;

/// The tag of atomic type `t`: its place in `ATOMIC_TYPES`.
fn tag(t: AtomicType) -> u8 {
    ATOMIC_TYPES.iter().position(|&a| a == t).and_then(|p| u8::try_from(p).ok()).unwrap_or(0)
}

/// `typing` written to `out`: its tag, then its atomic type's or types'.
pub(super) fn put_typing(out: &mut Vec<u8>, typing: &Typing) {
    match typing {
        Typing::AnySimple => out.put_u8(ANY_SIMPLE),
        Typing::Mixed => out.put_u8(MIXED),
        Typing::Empty => out.put_u8(EMPTY),
        Typing::ElementOnly => out.put_u8(ELEMENT_ONLY),
        Typing::Atomic(t) => {
            out.put_u8(ATOMIC_TYPING);
            out.put_u8(tag(*t));
        }
        Typing::List(t) => {
            out.put_u8(LIST_TYPING);
            out.put_u8(tag(*t));
        }
        Typing::Items(kinds) => {
            out.put_u8(ITEMS_TYPING);
            out.put_u32(count(kinds.len()));
            for kind in kinds.iter() {
                out.put_u8(tag(*kind));
            }
        }
    }
}

/// An atomic type read by its tag.
fn atomic(reader: &mut Reader<'_>) -> Result<AtomicType> {
    let t = reader.u8()?;
    ATOMIC_TYPES.get(usize::from(t)).copied().ok_or_else(|| reader.corrupt(&format!("an annotation's atomic type is {t}")))
}

/// A typing read.
fn typing(reader: &mut Reader<'_>) -> Result<Typing> {
    let tag = reader.u8()?;
    Ok(match tag {
        ANY_SIMPLE => Typing::AnySimple,
        MIXED => Typing::Mixed,
        EMPTY => Typing::Empty,
        ELEMENT_ONLY => Typing::ElementOnly,
        ATOMIC_TYPING => Typing::Atomic(atomic(reader)?),
        LIST_TYPING => Typing::List(atomic(reader)?),
        ITEMS_TYPING => {
            let mut kinds = Vec::new();
            for _ in 0..reader.u32()? {
                kinds.push(atomic(reader)?);
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
        out.push(Annotation { name: type_name, typing: typing(reader)? });
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
        typing(reader)?;
    }
    Ok(())
}
