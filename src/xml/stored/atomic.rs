//! An atomic item of a stored value (RULES.md D106): its type's tag, its
//! canonical lexical form, of a qualified name its namespace URI, then
//! whether it is of a user-defined atomic type, and if so that type's name
//! [XQueryDM §2.6.1, §3.3.1.2]; and the names of the stored form, written
//! and read alike wherever they are.

use std::sync::Arc;

use super::{ATOMIC_TYPES, HAS_PREFIX, HAS_URI};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::xml::atomic::Primitive;
use crate::xml::sequence::{Atomic, AtomicType};
use crate::xml::tree::QName;

/// What follows an atomic item's lexical form, and its namespace URI of a
/// qualified name: none, of a value of a built-in type.
const BUILT_IN: u8 = 0;
/// A user-defined atomic type's name.
const USER_DEFINED: u8 = 1;

/// `name` written: its flags, its prefix and namespace URI if it has them,
/// and its local name.
pub(super) fn put_name(out: &mut Vec<u8>, name: &QName) {
    out.put_u8(if name.prefix.is_some() { HAS_PREFIX } else { 0 } | if name.uri.is_some() { HAS_URI } else { 0 });
    if let Some(prefix) = &name.prefix {
        out.put_str(prefix);
    }
    if let Some(uri) = &name.uri {
        out.put_str(uri);
    }
    out.put_str(&name.local);
}

/// A name read, as `put_name` writes it.
pub(super) fn read_name(reader: &mut Reader<'_>) -> Result<QName> {
    let flags = reader.u8()?;
    let prefix = if flags & HAS_PREFIX != 0 { Some(reader.str()?.into()) } else { None };
    let uri = if flags & HAS_URI != 0 { Some(reader.str()?.into()) } else { None };
    Ok(QName { prefix, uri, local: reader.str()?.into() })
}

/// Whether a value of `kind` is a qualified name, whose namespace URI is
/// stored after its lexical form [XQueryDM §3.3.3].
fn is_name(kind: AtomicType) -> bool {
    matches!(kind.primitive(), Primitive::QName | Primitive::Notation)
}

/// `atomic` written, after its item's tag.
pub(super) fn put_atomic(out: &mut Vec<u8>, atomic: &Atomic) {
    let tag = ATOMIC_TYPES.iter().position(|&t| t == atomic.kind).and_then(|p| u8::try_from(p).ok());
    out.put_u8(tag.unwrap_or(0));
    out.put_str(&atomic.lexical);
    if is_name(atomic.kind) {
        out.put_str(atomic.namespace.as_deref().unwrap_or_default());
    }
    match &atomic.annotation {
        Some(name) => {
            out.put_u8(USER_DEFINED);
            put_name(out, name);
        }
        None => out.put_u8(BUILT_IN),
    }
}

/// An atomic value read, as `put_atomic` writes it.
pub(super) fn decode_atomic(reader: &mut Reader<'_>) -> Result<Atomic> {
    let tag = reader.u8()?;
    let Some(&kind) = ATOMIC_TYPES.get(usize::from(tag)) else {
        return Err(reader.corrupt(&format!("an atomic value's type is {tag}")));
    };
    let mut atomic = Atomic::new(kind, reader.str()?);
    if is_name(kind) {
        atomic.namespace = Some(reader.str()?).filter(|uri| !uri.is_empty()).map(Into::into);
    }
    let annotation = match reader.u8()? {
        BUILT_IN => None,
        USER_DEFINED => Some(Arc::new(read_name(reader)?)),
        other => return Err(reader.corrupt(&format!("an atomic value's type is marked {other}"))),
    };
    Ok(atomic.annotated(annotation))
}
