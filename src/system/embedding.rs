//! \[EXTENSION\] The views of embedding models, vector pipelines and the
//! queue of embeddings after commit (the vector design's §16), in
//! INFORMATION_SCHEMA, each as SELECT on it is PUBLIC's:
//!
//! * EMBEDDING_MODELS: each model the current user owns or holds EXECUTE
//!   on: its plug-in, shape, version, file, and whether its plug-in is
//!   registered. Its options are not shown, as one may hold a secret.
//! * EMBEDDING_MODEL_PRIVILEGES: each grant of EXECUTE on a model that the
//!   current user made or holds, or PUBLIC holds, or of a model of a schema
//!   the user owns, as ROUTINE_PRIVILEGES shows a routine's.
//! * VECTOR_PIPELINES: each pipeline of a schema the current user owns.
//! * EMBEDDING_QUEUE: of each column the queue fills, of a table of a
//!   schema the current user owns, the rows that waited as its last round
//!   began, those given a vector and the rounds failed since the database
//!   opened, and the last round's error, if it failed (R12.6).

use std::sync::Arc;

use super::{INFORMATION_SCHEMA, Seen, SystemTable, cardinal, catalog_name, identifier, number, text, text_column, yes};
use crate::catalog::{ColumnDesc, ModelDesc, QualifiedName, owner_of, sole};
use crate::error::Result;
use crate::storage::Row;
use crate::value::Value;

/// The longest text of a chunking, a version tag or an error shown.
const TEXT_LENGTH: u32 = 1000;
/// The grantor of the privilege a model's owner holds.
const SYSTEM: &str = "_SYSTEM";

/// EMBEDDING_MODELS.
struct Models;
/// EMBEDDING_MODEL_PRIVILEGES.
struct Privileges;
/// VECTOR_PIPELINES.
struct Pipelines;
/// EMBEDDING_QUEUE.
struct Queue;

/// The views.
pub(super) fn tables() -> [Arc<dyn SystemTable>; 4] {
    [Arc::new(Models), Arc::new(Privileges), Arc::new(Pipelines), Arc::new(Queue)]
}

/// The columns of a name: its catalog, schema and name, each prefixed.
fn named(prefix: &str) -> Vec<ColumnDesc> {
    ["CATALOG", "SCHEMA", "NAME"].iter().map(|part| identifier(&format!("{prefix}_{part}"), true)).collect()
}

/// The values of `qn`, in the catalog `seen` shows.
fn name_values(seen: &Seen<'_>, qn: &QualifiedName) -> Vec<Value> {
    vec![catalog_name(seen), text(&qn.auth), text(&qn.ident)]
}

/// Whether the current user owns the schema `schema`.
fn owns(seen: &Seen<'_>, schema: &str) -> bool {
    sole(&*seen.state.catalog, seen.auth).includes(owner_of(&*seen.state.catalog, schema))
}

/// Whether the current user holds EXECUTE on `m`.
fn executes(seen: &Seen<'_>, m: &ModelDesc) -> bool {
    m.execute_of(sole(&*seen.state.catalog, seen.auth), owner_of(&*seen.state.catalog, &m.name.auth)).0
}

impl SystemTable for Models {
    fn schema(&self) -> &'static str {
        INFORMATION_SCHEMA
    }

    fn name(&self) -> &'static str {
        "EMBEDDING_MODELS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let mut all = named("MODEL");
        all.extend([identifier("MODEL_TYPE", true), cardinal("DIMENSIONS", true), identifier("ELEMENT_FORMAT", true)]);
        all.extend([identifier("LAYOUT", true), cardinal("MODEL_VERSION", false), text_column(("MODEL_FILE", TEXT_LENGTH), false)]);
        all.extend([identifier("IS_REGISTERED", true), identifier("MODEL_OWNER", true)]);
        all
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        for m in seen.state.catalog.embeddings().models.iter().filter(|m| executes(seen, m)) {
            let plugin = seen.state.embedders.named(&m.plugin)?;
            let version = m.version.or_else(|| plugin.as_ref().map(|p| p.version()));
            let mut row = name_values(seen, &m.name);
            row.extend([text(&m.plugin), number(usize::try_from(m.dims).unwrap_or(usize::MAX)), text(m.element.name())]);
            row.extend([text(m.layout.name()), version.map_or(Value::Null, |v| number(usize::try_from(v).unwrap_or(usize::MAX)))]);
            row.extend([
                m.file.as_deref().map_or(Value::Null, text),
                yes(plugin.is_some()),
                text(owner_of(&*seen.state.catalog, &m.name.auth)),
            ]);
            rows.push(row);
        }
        Ok(rows)
    }
}

impl SystemTable for Privileges {
    fn schema(&self) -> &'static str {
        INFORMATION_SCHEMA
    }

    fn name(&self) -> &'static str {
        "EMBEDDING_MODEL_PRIVILEGES"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let mut all = vec![identifier("GRANTOR", true), identifier("GRANTEE", true)];
        all.extend(named("MODEL"));
        all.extend([identifier("PRIVILEGE_TYPE", true), identifier("IS_GRANTABLE", true)]);
        all
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let who = sole(&*seen.state.catalog, seen.auth);
        let mut rows = Vec::new();
        for m in &seen.state.catalog.embeddings().models {
            let owner = owner_of(&*seen.state.catalog, &m.name.auth);
            let first = std::iter::once((SYSTEM, owner, true));
            let granted = m.execute.iter().map(|u| (u.grantor.as_str(), u.grantee.as_deref().unwrap_or("PUBLIC"), u.grantable));
            for (grantor, grantee, grantable) in first.chain(granted) {
                if !(who.includes(owner) || who.includes(grantor) || who.includes(grantee) || grantee == "PUBLIC") {
                    continue;
                }
                let mut row = vec![text(grantor), text(grantee)];
                row.extend(name_values(seen, &m.name));
                row.extend([text("EXECUTE"), yes(grantable)]);
                rows.push(row);
            }
        }
        Ok(rows)
    }
}

impl SystemTable for Pipelines {
    fn schema(&self) -> &'static str {
        INFORMATION_SCHEMA
    }

    fn name(&self) -> &'static str {
        "VECTOR_PIPELINES"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let mut all = named("PIPELINE");
        all.extend([identifier("SOURCE_SCHEMA", true), identifier("SOURCE_NAME", true), identifier("TEXT_COLUMN", true)]);
        all.extend([text_column(("CHUNKING", TEXT_LENGTH), true), identifier("MODEL_SCHEMA", true), identifier("MODEL_NAME", true)]);
        all.extend([identifier("TARGET_SCHEMA", true), identifier("TARGET_NAME", true), identifier("MODE", true)]);
        all
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        for p in seen.state.catalog.embeddings().pipelines.iter().filter(|p| owns(seen, &p.name.auth)) {
            let mut row = name_values(seen, &p.name);
            row.extend([text(&p.source.auth), text(&p.source.ident), text(&p.text), text(&p.chunk)]);
            row.extend([text(&p.model.auth), text(&p.model.ident), text(&p.target.auth), text(&p.target.ident)]);
            row.push(text(if p.asynchronous { "ASYNC" } else { "SYNC" }));
            rows.push(row);
        }
        Ok(rows)
    }
}

impl SystemTable for Queue {
    fn schema(&self) -> &'static str {
        INFORMATION_SCHEMA
    }

    fn name(&self) -> &'static str {
        "EMBEDDING_QUEUE"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let mut all = vec![identifier("TABLE_SCHEMA", true), identifier("TABLE_NAME", true), identifier("COLUMN_NAME", true)];
        all.extend([text_column(("MODEL_VERSION", TEXT_LENGTH), true), cardinal("PENDING", true), cardinal("EMBEDDED", true)]);
        all.extend([cardinal("FAILURES", true), text_column(("LAST_ERROR", TEXT_LENGTH), false)]);
        all
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let count = |n: u64| number(usize::try_from(n).unwrap_or(usize::MAX));
        let error = |e: &str| text(&e.chars().take(usize::try_from(TEXT_LENGTH).unwrap_or(0)).collect::<String>());
        let mut rows = Vec::new();
        for s in seen.state.embedders.queue()?.into_iter().filter(|s| owns(seen, &s.schema) || seen.is_owner()) {
            let mut row = vec![text(&s.schema), text(&s.table), text(&s.column), text(&s.model)];
            row.extend([count(s.pending), count(s.embedded), count(s.failed), s.error.as_deref().map_or(Value::Null, error)]);
            rows.push(row);
        }
        Ok(rows)
    }
}
