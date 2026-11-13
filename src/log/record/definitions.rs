//! \[DESIGN\] The bodies of the records of definitions (RULES.md D25):
//! a `<schema>` processed, a schema statement of a transaction, a drop, and
//! where a sequence generator's counter goes on (RULES.md D62), as
//! `log/record.rs` lays them out.

use super::{Change, put_table, table, zone};
use crate::bytes::{Put, Reader};
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError};

/// The tags of the kinds of object a drop record names.
const DROPPED_TABLE: u8 = 1;
const DROPPED_VIEW: u8 = 2;
const DROPPED_SCHEMA: u8 = 3;
const DROPPED_DOMAIN: u8 = 4;
const DROPPED_ASSERTION: u8 = 5;
const DROPPED_COLLATION: u8 = 6;
const DROPPED_SEQUENCE: u8 = 7;
const DROPPED_TYPE: u8 = 8;

/// \[EXTENSION\] What a `Change::Drop` drops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropTarget {
    /// A base table [ISO/IEC 9075:1992 §11.18].
    Table(QualifiedName),
    /// A view [§11.20].
    View(QualifiedName),
    /// A schema, by its name [§11.2].
    Schema(String),
    /// A domain [§11.27].
    Domain(QualifiedName),
    /// An assertion [§11.35].
    Assertion(QualifiedName),
    /// A collation [ISO/IEC 9075-2:2023 §11.44].
    Collation(QualifiedName),
    /// A sequence generator [ISO/IEC 9075-2:2023 §11.74].
    Sequence(QualifiedName),
    /// A user-defined type [ISO/IEC 9075-2:2023 §11.59].
    Type(QualifiedName),
}

/// A schema's text, the engine of each table it created, its
/// displacement, and the epochs of the sequence generators it defined.
pub(super) fn put_schema(out: &mut Vec<u8>, (owner, sql, zone): (&str, &str, i16), engines: &[(String, String)], epochs: &[u64]) {
    out.put_str(owner);
    out.put_str(sql);
    out.put_u32(u32::try_from(engines.len()).unwrap_or(u32::MAX));
    for (table, engine) in engines {
        out.put_str(table);
        out.put_str(engine);
    }
    out.put_i128(i128::from(zone));
    put_epochs(out, epochs);
}

/// What `put_schema` wrote, as a change.
pub(super) fn schema(r: &mut Reader<'_>) -> Result<Change> {
    let (owner, sql) = (r.str()?, r.str()?);
    let count = r.u32()?;
    let mut engines = Vec::new();
    for _ in 0..count {
        engines.push((r.str()?, r.str()?));
    }
    Ok(Change::Schema { sql, owner, engines, zone: zone(r)?, epochs: epochs(r)? })
}

/// A schema statement's authorizer, default schema, text, created table's
/// engine and generation, displacement, and the epochs it gave.
pub(super) fn put_defined(
    out: &mut Vec<u8>,
    (auth, schema, sql, zone): (&str, &str, &str, i16),
    placed: Option<&(String, u32)>,
    epochs: &[u64],
) {
    out.put_str(auth);
    out.put_str(schema);
    out.put_str(sql);
    out.put_u8(u8::from(placed.is_some()));
    if let Some((engine, generation)) = placed {
        out.put_str(engine);
        out.put_u32(*generation);
    }
    out.put_i128(i128::from(zone));
    put_epochs(out, epochs);
}

/// An ALTER TABLE's table, action as written, default schema,
/// displacement, and the epochs it gave.
pub(super) fn put_altered(out: &mut Vec<u8>, (table, action, schema, zone): (&QualifiedName, &str, &str, i16), epochs: &[u64]) {
    put_table(out, table);
    out.put_str(action);
    out.put_str(schema);
    out.put_i128(i128::from(zone));
    put_epochs(out, epochs);
}

/// What `put_defined` wrote, as a change.
pub(super) fn defined(r: &mut Reader<'_>) -> Result<Change> {
    let (auth, schema, sql) = (r.str()?, r.str()?, r.str()?);
    let placed = if r.u8()? == 0 { None } else { Some((r.str()?, r.u32()?)) };
    Ok(Change::Defined { auth, schema, zone: zone(r)?, sql, placed, epochs: epochs(r)? })
}

/// The epochs of sequence generators' counters: a count u32, and each.
pub(super) fn put_epochs(out: &mut Vec<u8>, epochs: &[u64]) {
    out.put_u32(u32::try_from(epochs.len()).unwrap_or(u32::MAX));
    for e in epochs {
        out.put_u64(*e);
    }
}

/// What `put_epochs` wrote.
pub(super) fn epochs(r: &mut Reader<'_>) -> Result<Vec<u64>> {
    let count = r.u32()?;
    let mut epochs = Vec::new();
    for _ in 0..count {
        epochs.push(r.u64()?);
    }
    Ok(epochs)
}

/// A sequence generator's name, its counter's epoch, and where it goes on.
pub(super) fn put_sequence(out: &mut Vec<u8>, name: &QualifiedName, epoch: u64, resume: i128) {
    put_table(out, name);
    out.put_u64(epoch);
    out.put_i128(resume);
}

/// What `put_sequence` wrote, as a change.
pub(super) fn sequence(r: &mut Reader<'_>) -> Result<Change> {
    Ok(Change::Sequence { name: table(r)?, epoch: r.u64()?, resume: r.i128()? })
}

/// What a drop record drops, and whether with CASCADE.
pub(super) fn put_dropped(out: &mut Vec<u8>, object: &DropTarget, cascade: bool) {
    let (tag, name) = match object {
        DropTarget::Table(table) => (DROPPED_TABLE, table),
        DropTarget::View(view) => (DROPPED_VIEW, view),
        DropTarget::Schema(schema) => {
            out.put_u8(DROPPED_SCHEMA);
            out.put_str(schema);
            out.put_u8(u8::from(cascade));
            return;
        }
        DropTarget::Domain(domain) => (DROPPED_DOMAIN, domain),
        DropTarget::Assertion(assertion) => (DROPPED_ASSERTION, assertion),
        DropTarget::Collation(collation) => (DROPPED_COLLATION, collation),
        DropTarget::Sequence(sequence) => (DROPPED_SEQUENCE, sequence),
        DropTarget::Type(user_type) => (DROPPED_TYPE, user_type),
    };
    out.put_u8(tag);
    put_table(out, name);
    out.put_u8(u8::from(cascade));
}

/// What `put_dropped` wrote, before its CASCADE.
pub(super) fn dropped(r: &mut Reader<'_>) -> Result<DropTarget> {
    match r.u8()? {
        DROPPED_TABLE => Ok(DropTarget::Table(table(r)?)),
        DROPPED_VIEW => Ok(DropTarget::View(table(r)?)),
        DROPPED_SCHEMA => Ok(DropTarget::Schema(r.str()?)),
        DROPPED_DOMAIN => Ok(DropTarget::Domain(table(r)?)),
        DROPPED_ASSERTION => Ok(DropTarget::Assertion(table(r)?)),
        DROPPED_COLLATION => Ok(DropTarget::Collation(table(r)?)),
        DROPPED_SEQUENCE => Ok(DropTarget::Sequence(table(r)?)),
        DROPPED_TYPE => Ok(DropTarget::Type(table(r)?)),
        other => Err(SqlError::new(Code::Corrupt, format!("[DESIGN] a log record drops an unknown kind of object, {other}"))),
    }
}
