//! \[DESIGN\] Log records (RULES.md D25): what the database writes to its
//! log, engine-neutral, as `TableStorage` speaks: the row changes of a
//! transaction, with the row id and the before- and after-images that redo
//! and undo need; its end, committed with its commit timestamp (D29) or
//! aborted; and changes of definitions. The format is the database's, the
//! same on every platform, and versioned:
//!
//! ```text
//! record:  version u8 | kind u8 | transaction u64 | body
//! insert:  table | row id u64 | after-image row
//! update:  table | row id u64 | before-image row | after-image row
//! delete:  table | row id u64 | before-image row
//! commit:  commit timestamp u64
//! abort:   (nothing)
//! checkpoint: (nothing)
//! undone:  position u64 of the statement's first change
//! schema:  owner | the `<schema>` text | tables (count u32, each:
//!          identifier, engine)
//! index:   name | table | columns (count u32, names) | method
//! setting: name | value, null for its default again
//! user:    authorization identifier | kept u8 | verifier bytes, if kept
//! owner:   authorization identifier
//! table:   authorization identifier | identifier
//! drop:    object u8 (1 table, 2 view, 3 schema, 4 domain) | table, view
//!          or domain, or the schema's name | cascade u8
//! defined: authorization identifier | default schema | the statement's
//!          text | placed u8, 1 for a table created, then its engine and
//!          its storage's generation u32
//! stale:   checkpoint's position u64 | index | row id u64 | key row
//! ```
//!
//! Strings are a length and UTF-8; rows and values as in `value/codec.rs`.
//! A record needs no checksum of its own: the block holding it has one.

use super::Lsn;
use crate::bytes::{Put, Reader};
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError};
use crate::storage::{Row, RowId};
use crate::txn::{Timestamp, TxId};
use crate::value::Value;
use crate::value::codec::{put_row, put_value, row, value};
use crate::value::types::TypeRegistry;

/// The version of the format this code writes and reads.
pub const RECORD_VERSION: u8 = 1;

/// The tags of the kinds of record.
const INSERT: u8 = 1;
const UPDATE: u8 = 2;
const DELETE: u8 = 3;
const COMMIT: u8 = 4;
const ABORT: u8 = 5;
const SCHEMA: u8 = 6;
const CREATE_INDEX: u8 = 7;
const STATEMENT_UNDONE: u8 = 8;
const CHECKPOINT: u8 = 9;
const DROP_INDEX: u8 = 10;
const DROP: u8 = 11;
const ALTER_TABLE: u8 = 12;
const SETTING: u8 = 13;
const ALTER_INDEX: u8 = 14;
const STALE: u8 = 15;
const USER: u8 = 16;
const OWNER: u8 = 17;
const DEFINED: u8 = 18;

/// The tags of the kinds of object a drop record names.
const DROPPED_TABLE: u8 = 1;
const DROPPED_VIEW: u8 = 2;
const DROPPED_SCHEMA: u8 = 3;
const DROPPED_DOMAIN: u8 = 4;
const DROPPED_ASSERTION: u8 = 5;

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
}

/// What a transaction did, as a log record.
#[derive(Debug, Clone)]
pub enum Change {
    /// A row inserted, as it was inserted.
    Insert {
        /// Its table.
        table: QualifiedName,
        /// The generation of the table's storage (`TableDesc::generation`).
        generation: u32,
        /// Its row id.
        row: RowId,
        /// The row inserted.
        after: Row,
    },
    /// A row updated, as it was and as it became.
    Update {
        /// Its table.
        table: QualifiedName,
        /// The generation of the table's storage (`TableDesc::generation`).
        generation: u32,
        /// Its row id.
        row: RowId,
        /// The row before.
        before: Row,
        /// The row after.
        after: Row,
    },
    /// A row deleted, as it was.
    Delete {
        /// Its table.
        table: QualifiedName,
        /// The generation of the table's storage (`TableDesc::generation`).
        generation: u32,
        /// Its row id.
        row: RowId,
        /// The row deleted.
        before: Row,
    },
    /// The transaction committed.
    Commit {
        /// Its commit timestamp (RULES.md D29).
        at: Timestamp,
    },
    /// The transaction was rolled back.
    Abort,
    /// A statement of the transaction failed and was undone
    /// [X3.135-1986 §7.3 GR3b i]: its changes logged at and after `from`
    /// are cancelled.
    StatementUndone {
        /// The position of the statement's first change.
        from: Lsn,
    },
    /// \[DESIGN\] Every engine holds every change logged before it: written
    /// when the database is closed cleanly (`Database::close`).
    Checkpoint,
    /// \[DESIGN\] An index entry of a version awaiting collection when the
    /// checkpoint holding every change before `through` was taken: recovery
    /// from that checkpoint takes it back, as the crash lost the collection
    /// (`db/checkpoint.rs`).
    Stale {
        /// The position its checkpoint holds every change before.
        through: Lsn,
        /// The index.
        index: String,
        /// The row the entry is of.
        row: RowId,
        /// The entry's key.
        key: Row,
    },
    /// A `<schema>` processed [X3.135-1986 §6.1].
    Schema {
        /// Its text.
        sql: String,
        /// The `<authorization identifier>` that owns it: its AUTHORIZATION,
        /// or the session's [ISO/IEC 9075:1992 §11.1 SR2b], which the text
        /// does not say.
        owner: String,
        /// Each table it created, and the engine it was placed in: what
        /// replaying the text alone would not know (RULES.md D25).
        engines: Vec<(String, String)>,
        /// The time zone displacement, in minutes, of a datetime it writes
        /// without one [ISO/IEC 9075:1992 §5.3 SR18]: the session's, or of
        /// the operator's, the database's.
        zone: i16,
    },
    /// \[EXTENSION\] An index created.
    CreateIndex {
        /// Its name.
        name: String,
        /// Its table.
        table: QualifiedName,
        /// The columns it is on, in order.
        columns: Vec<String>,
        /// Its index method.
        method: String,
        /// \[EXTENSION\] The method's options it sets (RULES.md D41).
        options: Vec<(String, Value)>,
    },
    /// \[EXTENSION\] An index's options changed (RULES.md D41).
    AlterIndex {
        /// Its name.
        name: String,
        /// The options it sets now.
        options: Vec<(String, Value)>,
    },
    /// \[EXTENSION\] An index dropped.
    DropIndex {
        /// Its name.
        name: String,
    },
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §11.10] A table altered by its
    /// transaction, its storage given a new generation, which the
    /// definitions hold once it commits.
    AlterTable {
        /// The table.
        table: QualifiedName,
        /// The `<alter table action>` as written.
        action: String,
        /// The schema of the names it writes without one: the session's
        /// default schema [ISO/IEC 9075:1992 §5.4 SR8b].
        schema: String,
        /// The displacement, in minutes, of a datetime it writes without
        /// one: the session's [ISO/IEC 9075:1992 §5.3 SR18].
        zone: i16,
    },
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §11.2, §11.18, §11.20] A table,
    /// view or schema dropped by its transaction, which the definitions
    /// hold once it commits.
    Drop {
        /// What was dropped.
        object: DropTarget,
        /// CASCADE was written: what depends on it went with it.
        cascade: bool,
    },
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §11.3, §11.19, §11.36, §11.37] A
    /// table or view defined, or privileges granted or revoked, by a
    /// statement of its transaction (`ast::SchemaStatement`), which the
    /// definitions hold once it commits: made again from its text.
    Defined {
        /// The authorization identifier that ran it.
        auth: String,
        /// The session's default schema as it ran.
        schema: String,
        /// The session's time zone displacement as it ran, in minutes
        /// [ISO/IEC 9075:1992 §5.3 SR18].
        zone: i16,
        /// Its text.
        sql: String,
        /// Of a table it created, the engine it was placed in and the
        /// generation of its storage: what replaying the text alone would
        /// not know (RULES.md D25).
        placed: Option<(String, u32)>,
    },
    /// \[DESIGN\] RULES.md D36: a database setting set, outside any
    /// transaction.
    Setting {
        /// The setting's name.
        name: String,
        /// Its value; none for its default again.
        value: Option<Value>,
    },
    /// \[EXTENSION\] A user created, or given a new verifier; or, with
    /// none, dropped (`db/users.rs`). Made outside any transaction.
    User {
        /// Its authorization identifier.
        name: String,
        /// Its verifier, as its credentials plug-in made it.
        verifier: Option<Vec<u8>>,
    },
    /// \[EXTENSION\] The database's owner set (`db/users.rs`). Made outside
    /// any transaction.
    Owner {
        /// The owner's authorization identifier.
        name: String,
    },
}

/// A log record: a change and the transaction that made it.
#[derive(Debug, Clone)]
pub struct Record {
    /// The transaction.
    pub tx: TxId,
    /// What it did.
    pub change: Change,
}

impl Record {
    /// The record's bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.put_u8(RECORD_VERSION);
        out.put_u8(kind(&self.change));
        out.put_u64(self.tx);
        match &self.change {
            Change::Insert { table, generation, row, after } => put_row_change(&mut out, (table, *generation, *row), &[after]),
            Change::Update { table, generation, row, before, after } => {
                put_row_change(&mut out, (table, *generation, *row), &[before, after]);
            }
            Change::Delete { table, generation, row, before } => put_row_change(&mut out, (table, *generation, *row), &[before]),
            Change::Commit { at } => out.put_u64(*at),
            Change::Abort | Change::Checkpoint => {}
            Change::StatementUndone { from } => out.put_u64(from.0),
            Change::Stale { through, index, row, key } => {
                out.put_u64(through.0);
                out.put_str(index);
                out.put_u64(*row);
                put_row(&mut out, key);
            }
            Change::Schema { sql, owner, engines, zone } => put_schema(&mut out, (owner, sql, *zone), engines),
            Change::Defined { auth, schema, zone, sql, placed } => put_defined(&mut out, (auth, schema, sql, *zone), placed.as_ref()),
            Change::CreateIndex { name, table, columns, method, options } => {
                out.put_str(name);
                put_table(&mut out, table);
                put_strs(&mut out, columns);
                out.put_str(method);
                put_options(&mut out, options);
            }
            Change::AlterIndex { name, options } => {
                out.put_str(name);
                put_options(&mut out, options);
            }
            Change::DropIndex { name } | Change::Owner { name } => out.put_str(name),
            Change::Drop { object, cascade } => put_dropped(&mut out, object, *cascade),
            Change::AlterTable { table, action, schema, zone } => {
                put_table(&mut out, table);
                out.put_str(action);
                out.put_str(schema);
                out.put_i128(i128::from(*zone));
            }
            Change::Setting { name, value } => {
                out.put_str(name);
                put_value(&mut out, value.as_ref().unwrap_or(&Value::Null));
            }
            Change::User { name, verifier } => put_user(&mut out, name, verifier.as_deref()),
        }
        out
    }

    /// The record `bytes` hold.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not a whole record of this format: too
    /// short, too long, of another version or an unknown kind, or holding a
    /// malformed field.
    pub fn decode(bytes: &[u8], types: &TypeRegistry) -> Result<Record> {
        let mut r = Reader::new(bytes, "log record").with_types(types);
        let version = r.u8()?;
        if version != RECORD_VERSION {
            return Err(SqlError::new(
                Code::Corrupt,
                format!("[DESIGN] a log record is of format version {version}, and only {RECORD_VERSION} is known"),
            ));
        }
        let tag = r.u8()?;
        let tx = r.u64()?;
        let change = match tag {
            INSERT => Change::Insert { table: table(&mut r)?, generation: r.u32()?, row: r.u64()?, after: row(&mut r)? },
            UPDATE => {
                Change::Update { table: table(&mut r)?, generation: r.u32()?, row: r.u64()?, before: row(&mut r)?, after: row(&mut r)? }
            }
            DELETE => Change::Delete { table: table(&mut r)?, generation: r.u32()?, row: r.u64()?, before: row(&mut r)? },
            COMMIT => Change::Commit { at: r.u64()? },
            ABORT => Change::Abort,
            CHECKPOINT => Change::Checkpoint,
            STATEMENT_UNDONE => Change::StatementUndone { from: Lsn(r.u64()?) },
            STALE => Change::Stale { through: Lsn(r.u64()?), index: r.str()?, row: r.u64()?, key: row(&mut r)? },
            SCHEMA => {
                let owner = r.str()?;
                let (sql, engines) = schema(&mut r)?;
                Change::Schema { sql, owner, engines, zone: zone(&mut r)? }
            }
            DEFINED => defined(&mut r)?,
            CREATE_INDEX => {
                let name = r.str()?;
                let table = table(&mut r)?;
                let count = r.u32()?;
                let mut columns = Vec::new();
                for _ in 0..count {
                    columns.push(r.str()?);
                }
                Change::CreateIndex { name, table, columns, method: r.str()?, options: options(&mut r)? }
            }
            ALTER_INDEX => Change::AlterIndex { name: r.str()?, options: options(&mut r)? },
            DROP_INDEX => Change::DropIndex { name: r.str()? },
            DROP => Change::Drop { object: dropped(&mut r)?, cascade: r.u8()? != 0 },
            ALTER_TABLE => Change::AlterTable { table: table(&mut r)?, action: r.str()?, schema: r.str()?, zone: zone(&mut r)? },
            // No setting takes the null value: it stands for the default.
            SETTING => Change::Setting { name: r.str()?, value: Some(value(&mut r)?).filter(|v| !v.is_null()) },
            USER => user(&mut r)?,
            OWNER => Change::Owner { name: r.str()? },
            other => return Err(r.corrupt(&format!("no kind of record has tag {other}"))),
        };
        r.finish()?;
        Ok(Record { tx, change })
    }
}

/// A schema's text, the engine of each table it created, and its
/// displacement.
fn put_schema(out: &mut Vec<u8>, (owner, sql, zone): (&str, &str, i16), engines: &[(String, String)]) {
    out.put_str(owner);
    out.put_str(sql);
    out.put_u32(u32::try_from(engines.len()).unwrap_or(u32::MAX));
    for (table, engine) in engines {
        out.put_str(table);
        out.put_str(engine);
    }
    out.put_i128(i128::from(zone));
}

/// What `put_schema` wrote after the owner: the text, and the engines.
fn schema(r: &mut Reader<'_>) -> Result<(String, Vec<(String, String)>)> {
    let sql = r.str()?;
    let count = r.u32()?;
    let mut engines = Vec::new();
    for _ in 0..count {
        engines.push((r.str()?, r.str()?));
    }
    Ok((sql, engines))
}

/// A schema statement's authorizer, default schema, text, created table's
/// engine and generation, and displacement.
fn put_defined(out: &mut Vec<u8>, (auth, schema, sql, zone): (&str, &str, &str, i16), placed: Option<&(String, u32)>) {
    out.put_str(auth);
    out.put_str(schema);
    out.put_str(sql);
    out.put_u8(u8::from(placed.is_some()));
    if let Some((engine, generation)) = placed {
        out.put_str(engine);
        out.put_u32(*generation);
    }
    out.put_i128(i128::from(zone));
}

/// What `put_defined` wrote, as a change.
fn defined(r: &mut Reader<'_>) -> Result<Change> {
    let (auth, schema, sql) = (r.str()?, r.str()?, r.str()?);
    let placed = if r.u8()? == 0 { None } else { Some((r.str()?, r.u32()?)) };
    Ok(Change::Defined { auth, schema, zone: zone(r)?, sql, placed })
}

/// A time zone displacement, in minutes, as `put_i128` wrote it.
fn zone(r: &mut Reader<'_>) -> Result<i16> {
    let minutes = r.i128()?;
    i16::try_from(minutes).map_err(|_| r.corrupt("a time zone displacement is out of range"))
}

/// A user's name, and its verifier if it is kept.
fn put_user(out: &mut Vec<u8>, name: &str, verifier: Option<&[u8]>) {
    out.put_str(name);
    out.put_u8(u8::from(verifier.is_some()));
    if let Some(verifier) = verifier {
        out.put_bytes(verifier);
    }
}

/// What `put_user` wrote, as a change.
fn user(r: &mut Reader<'_>) -> Result<Change> {
    let name = r.str()?;
    let kept = r.u8()? != 0;
    Ok(Change::User { name, verifier: if kept { Some(r.bytes()?.to_vec()) } else { None } })
}

/// Strings, after how many.
fn put_strs(out: &mut Vec<u8>, strs: &[String]) {
    out.put_u32(u32::try_from(strs.len()).unwrap_or(u32::MAX));
    for s in strs {
        out.put_str(s);
    }
}

/// The tag of `change`'s kind.
fn kind(change: &Change) -> u8 {
    match change {
        Change::Insert { .. } => INSERT,
        Change::Update { .. } => UPDATE,
        Change::Delete { .. } => DELETE,
        Change::Commit { .. } => COMMIT,
        Change::Abort => ABORT,
        Change::Checkpoint => CHECKPOINT,
        Change::StatementUndone { .. } => STATEMENT_UNDONE,
        Change::Stale { .. } => STALE,
        Change::Schema { .. } => SCHEMA,
        Change::Defined { .. } => DEFINED,
        Change::CreateIndex { .. } => CREATE_INDEX,
        Change::DropIndex { .. } => DROP_INDEX,
        Change::AlterIndex { .. } => ALTER_INDEX,
        Change::Drop { .. } => DROP,
        Change::AlterTable { .. } => ALTER_TABLE,
        Change::Setting { .. } => SETTING,
        Change::User { .. } => USER,
        Change::Owner { .. } => OWNER,
    }
}

/// Options, each a name and a value: their count, then each.
fn put_options(out: &mut Vec<u8>, options: &[(String, Value)]) {
    out.put_u32(u32::try_from(options.len()).unwrap_or(u32::MAX));
    for (name, value) in options {
        out.put_str(name);
        put_value(out, value);
    }
}

/// The options `put_options` wrote.
fn options(r: &mut Reader<'_>) -> Result<Vec<(String, Value)>> {
    let count = r.u32()?;
    let mut out = Vec::new();
    for _ in 0..count {
        out.push((r.str()?, value(r)?));
    }
    Ok(out)
}

/// A row change's table, generation and row id, then its images.
fn put_row_change(out: &mut Vec<u8>, (table, generation, row): (&QualifiedName, u32, RowId), images: &[&Row]) {
    put_table(out, table);
    out.put_u32(generation);
    out.put_u64(row);
    for image in images {
        put_row(out, image);
    }
}

fn put_table(out: &mut Vec<u8>, table: &QualifiedName) {
    out.put_str(&table.auth);
    out.put_str(&table.ident);
}

fn table(r: &mut Reader<'_>) -> Result<QualifiedName> {
    Ok(QualifiedName { auth: r.str()?, ident: r.str()? })
}

/// What a drop dropped, its kind's tag then its name, and whether with
/// CASCADE.
fn put_dropped(out: &mut Vec<u8>, object: &DropTarget, cascade: bool) {
    match object {
        DropTarget::Table(table) => {
            out.put_u8(DROPPED_TABLE);
            put_table(out, table);
        }
        DropTarget::View(view) => {
            out.put_u8(DROPPED_VIEW);
            put_table(out, view);
        }
        DropTarget::Schema(schema) => {
            out.put_u8(DROPPED_SCHEMA);
            out.put_str(schema);
        }
        DropTarget::Domain(domain) => {
            out.put_u8(DROPPED_DOMAIN);
            put_table(out, domain);
        }
        DropTarget::Assertion(assertion) => {
            out.put_u8(DROPPED_ASSERTION);
            put_table(out, assertion);
        }
    }
    out.put_u8(u8::from(cascade));
}

/// What `put_dropped` wrote, before its CASCADE.
fn dropped(r: &mut Reader<'_>) -> Result<DropTarget> {
    match r.u8()? {
        DROPPED_TABLE => Ok(DropTarget::Table(table(r)?)),
        DROPPED_VIEW => Ok(DropTarget::View(table(r)?)),
        DROPPED_SCHEMA => Ok(DropTarget::Schema(r.str()?)),
        DROPPED_DOMAIN => Ok(DropTarget::Domain(table(r)?)),
        DROPPED_ASSERTION => Ok(DropTarget::Assertion(table(r)?)),
        other => Err(SqlError::new(Code::Corrupt, format!("[DESIGN] a log record drops an unknown kind of object, {other}"))),
    }
}

// Unit tests: every kind of record read back as written, and what is not a
// record refused (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
