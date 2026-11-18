//! \[DESIGN\] The bodies of the records of definitions (RULES.md D25):
//! a `<schema>` processed, a schema statement of a transaction, a drop, and
//! where a sequence generator's counter goes on (RULES.md D62), as
//! `log/record.rs` lays them out.

use super::{Change, options, put_options, put_strs, put_table, table, zone};
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
const DROPPED_TRIGGER: u8 = 9;
const DROPPED_ROUTINE: u8 = 10;
const DROPPED_MODULE: u8 = 11;
/// The tag of a user-defined cast's drop.
const DROPPED_CAST: u8 = 12;
/// The tag of a user-defined ordering's drop.
const DROPPED_ORDERING: u8 = 13;
/// The tag of a transform group's drop, or of all a type's.
const DROPPED_TRANSFORM: u8 = 14;
/// The tag of an embedding model's drop.
const DROPPED_MODEL: u8 = 15;
/// The tag of a vector pipeline's drop.
const DROPPED_PIPELINE: u8 = 16;
/// The tag of a property graph's drop.
const DROPPED_GRAPH: u8 = 17;
/// The tag of a graph type's drop.
const DROPPED_GRAPH_TYPE: u8 = 18;
/// The tag of a GQL graph's drop.
const DROPPED_GQL_GRAPH: u8 = 19;

/// \[EXTENSION\] What a `Change::Drop` drops.
#[derive(Debug, Clone, PartialEq)]
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
    /// A trigger [ISO/IEC 9075-2:2023 §11.50].
    Trigger(QualifiedName),
    /// An SQL-invoked routine, by its specific name [ISO/IEC 9075-2:2023
    /// §11.62].
    Routine(QualifiedName),
    /// An SQL-server module [ISO/IEC 9075-4:2006 draft §9.22].
    Module(QualifiedName),
    /// A user-defined cast, by its source and target types [ISO/IEC
    /// 9075-2:2023 §11.64].
    Cast(Box<(crate::value::DataType, crate::value::DataType)>),
    /// A user-defined ordering, by its type [ISO/IEC 9075-2:2023 §11.66].
    Ordering(QualifiedName),
    /// A type's transform group, or all of them [ISO/IEC 9075-2:2023
    /// §11.71].
    Transform(QualifiedName, Option<String>),
    /// \[EXTENSION\] A property graph (RULES.md D-GRAPH-CATALOG).
    Graph(QualifiedName),
    /// \[EXTENSION\] An embedding model (the vector design's §12.1).
    Model(QualifiedName),
    /// \[EXTENSION\] A vector pipeline (§12.5).
    Pipeline(QualifiedName),
    /// \[EXTENSION\] A graph type (RULES.md D-GQL).
    GraphType(QualifiedName),
    /// \[EXTENSION\] A GQL graph, with its tables (RULES.md D-GQL).
    GqlGraph(QualifiedName),
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

/// A schema statement's authorizer and its role (u8 1 and str, or u8 0 for
/// none), default schema, text, created table's engine and generation,
/// displacement, and the epochs it gave.
pub(super) fn put_defined(
    out: &mut Vec<u8>,
    ((auth, role), schema, sql, zone): ((&str, Option<&str>), &str, &str, i16),
    placed: Option<&(String, u32)>,
    epochs: &[u64],
) {
    out.put_str(auth);
    out.put_u8(u8::from(role.is_some()));
    if let Some(role) = role {
        out.put_str(role);
    }
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
    let auth = r.str()?;
    let role = if r.u8()? == 0 { None } else { Some(r.str()?) };
    let (schema, sql) = (r.str()?, r.str()?);
    let placed = if r.u8()? == 0 { None } else { Some((r.str()?, r.u32()?)) };
    Ok(Change::Defined { auth, role, schema, zone: zone(r)?, sql, placed, epochs: epochs(r)? })
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
        DropTarget::Trigger(trigger) => (DROPPED_TRIGGER, trigger),
        DropTarget::Routine(specific) => (DROPPED_ROUTINE, specific),
        DropTarget::Module(module) => (DROPPED_MODULE, module),
        DropTarget::Ordering(udt) => (DROPPED_ORDERING, udt),
        DropTarget::Graph(graph) => (DROPPED_GRAPH, graph),
        DropTarget::Model(model) => (DROPPED_MODEL, model),
        DropTarget::Pipeline(pipeline) => (DROPPED_PIPELINE, pipeline),
        DropTarget::GraphType(t) => (DROPPED_GRAPH_TYPE, t),
        DropTarget::GqlGraph(graph) => (DROPPED_GQL_GRAPH, graph),
        DropTarget::Cast(types) => {
            out.put_u8(DROPPED_CAST);
            crate::value::types::tags::put_type(out, &types.0);
            crate::value::types::tags::put_type(out, &types.1);
            out.put_u8(u8::from(cascade));
            return;
        }
        DropTarget::Transform(udt, group) => {
            out.put_u8(DROPPED_TRANSFORM);
            put_table(out, udt);
            out.put_u8(u8::from(group.is_some()));
            if let Some(g) = group {
                out.put_str(g);
            }
            out.put_u8(u8::from(cascade));
            return;
        }
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
        DROPPED_TRIGGER => Ok(DropTarget::Trigger(table(r)?)),
        DROPPED_ROUTINE => Ok(DropTarget::Routine(table(r)?)),
        DROPPED_MODULE => Ok(DropTarget::Module(table(r)?)),
        DROPPED_ORDERING => Ok(DropTarget::Ordering(table(r)?)),
        DROPPED_GRAPH => Ok(DropTarget::Graph(table(r)?)),
        DROPPED_MODEL => Ok(DropTarget::Model(table(r)?)),
        DROPPED_PIPELINE => Ok(DropTarget::Pipeline(table(r)?)),
        DROPPED_GRAPH_TYPE => Ok(DropTarget::GraphType(table(r)?)),
        DROPPED_GQL_GRAPH => Ok(DropTarget::GqlGraph(table(r)?)),
        DROPPED_CAST => {
            let source = crate::value::types::tags::data_type(r)?;
            Ok(DropTarget::Cast(Box::new((source, crate::value::types::tags::data_type(r)?))))
        }
        DROPPED_TRANSFORM => {
            let udt = table(r)?;
            let group = if r.u8()? == 0 { None } else { Some(r.str()?) };
            Ok(DropTarget::Transform(udt, group))
        }
        other => Err(SqlError::new(Code::Corrupt, format!("[DESIGN] a log record drops an unknown kind of object, {other}"))),
    }
}

/// A CREATE INDEX change's bytes: its name, table, columns, method,
/// options, and its key parts that are expressions (RULES.md
/// D92), each its place and text. Any other change, none.
pub(super) fn put_create_index(out: &mut Vec<u8>, change: &Change) {
    let Change::CreateIndex { name, table, columns, method, options, expressions } = change else { return };
    out.put_str(name);
    put_table(out, table);
    put_strs(out, columns);
    out.put_str(method);
    put_options(out, options);
    out.put_u32(u32::try_from(expressions.len()).unwrap_or(u32::MAX));
    for (part, text) in expressions {
        out.put_u32(u32::try_from(*part).unwrap_or(u32::MAX));
        out.put_str(text);
    }
}

/// What `put_create_index` wrote, as a change.
pub(super) fn create_index(r: &mut Reader<'_>) -> Result<Change> {
    let (name, table) = (r.str()?, table(r)?);
    let columns = (0..r.u32()?).map(|_| r.str()).collect::<Result<_>>()?;
    let (method, options) = (r.str()?, options(r)?);
    let mut expressions = Vec::new();
    for _ in 0..r.u32()? {
        expressions.push((usize::try_from(r.u32()?).unwrap_or(usize::MAX), r.str()?));
    }
    Ok(Change::CreateIndex { name, table, columns, method, options, expressions })
}
