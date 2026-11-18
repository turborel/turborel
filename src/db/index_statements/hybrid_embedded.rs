//! \[EXTENSION\] A hybrid index that embeds its text itself (the vector
//! design document §11.1): CREATE HYBRID VECTOR INDEX `i` ON `t` (`text`)
//! with a MODEL, and a CHUNKER and a MODE if any. The index stores its
//! text's chunks, their embeddings and an index of both, kept as the
//! text changes, synchronously or after commit (R12.6).
//!
//! It is made of what the embedding models' design gives (`db/embedding.rs`).
//! A table `i_CHUNKS` (DOC_ID, CHUNK_ID, CHUNK_TEXT, EMBEDDING) holds the
//! chunks, of the types CHUNK_TEXT and EMBED of the model give. A vector
//! pipeline `i_PIPELINE` keeps it in step with `t`, its key and its text,
//! in the mode given, and fills it as it is made. The hybrid index `i` is
//! of that table's CHUNK_TEXT and EMBEDDING, and keeps the options given,
//! so INFORMATION_SCHEMA shows its model. DROP INDEX `i` drops the
//! pipeline and the table with it.
//!
//! The table and the pipeline are made, and the table filled, in a
//! transaction of their own, of the session's user, committed before the
//! index is made over them, as an index statement runs outside a
//! transaction. If the index is refused, they are dropped again.
//! HYBRID_SEARCH of the index gives each document, by `t`'s key, once,
//! at its best chunk (`binder/hybrid_search.rs`).

use super::super::indexes::IndexSpec;
use super::super::{Session, State};
use crate::binder::qualify;
use crate::catalog::{IndexDesc, QualifiedName};
use crate::db::embedding::{delimited, model_named, written};
use crate::error::{Code, Result, SqlError};
use crate::index::vector::hybrid::{CHUNKER_OPTION, HYBRID_METHOD, MODE_OPTION, MODEL_OPTION};
use crate::value::Value;
use crate::vector::chunk::{Spec, quote};

/// The suffix of the name of the table of chunks.
const CHUNKS: &str = "_CHUNKS";
/// The suffix of the name of the pipeline.
const PIPELINE: &str = "_PIPELINE";
/// The columns of the table of chunks the index is of.
const INDEXED: [&str; 2] = ["CHUNK_TEXT", "EMBEDDING"];

fn refused(message: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {message}"))
}

/// The text option `name` of `options`, if set and not empty.
fn text_of<'o>(options: &'o [(String, Value)], name: &str) -> Option<&'o str> {
    options
        .iter()
        .find(|(n, _)| n == name)
        .and_then(|(_, v)| if let Value::Char(s) = v { Some(s.trim()) } else { None })
        .filter(|s| !s.is_empty())
}

/// Whether the index `method` of `options` describes embeds its text.
fn embeds(method: &str, options: &[(String, Value)]) -> bool {
    method == HYBRID_METHOD && text_of(options, MODEL_OPTION).is_some()
}

/// The table of chunks and the pipeline of the index `name` of a table of
/// schema `schema`.
fn kept(name: &str, schema: &str) -> (QualifiedName, QualifiedName) {
    let of = |suffix: &str| QualifiedName { auth: schema.to_owned(), ident: format!("{name}{suffix}") };
    (of(CHUNKS), of(PIPELINE))
}

/// The statements that make the table of chunks and the pipeline of the
/// index `spec` describes, the table `t`'s key column `key`, of `model`.
fn statements(spec: &IndexSpec, (key, text): (&str, &str), model: &QualifiedName) -> [String; 2] {
    let (chunks, pipeline) = kept(&spec.name, &spec.table.auth);
    let chunk = text_of(&spec.options, CHUNKER_OPTION).map_or_else(|| Spec::default().written(), str::to_owned);
    let mode = text_of(&spec.options, MODE_OPTION).unwrap_or("SYNC");
    let (table, chunks, key, text) = (written(&spec.table), written(&chunks), delimited(key), delimited(text));
    [
        format!(
            "CREATE TABLE {chunks} (DOC_ID, CHUNK_ID, CHUNK_TEXT, EMBEDDING) AS (SELECT S.{key}, U.CHUNK_ID, U.CHUNK_TEXT, \
             EMBED({}, U.CHUNK_TEXT) FROM {table} AS S, UNNEST(CHUNK_TEXT(S.{text}, {})) AS U (CHUNK_ID, CHUNK_OFFSET, \
             CHUNK_LENGTH, CHUNK_TEXT)) WITH NO DATA",
            quote(&written(model)),
            quote(&chunk),
        ),
        format!(
            "CREATE VECTOR PIPELINE {} SOURCE {table} ({key}, {text}) CHUNK {chunk} EMBED USING {} TARGET {chunks} \
             (DOC_ID, CHUNK_ID, CHUNK_TEXT, EMBEDDING) MODE {mode}",
            written(&pipeline),
            written(model),
        ),
    ]
}

impl Session {
    /// The index `spec` describes made, on `st`: of a hybrid index that
    /// embeds its text, its table of chunks and its pipeline first.
    ///
    /// # Errors
    ///
    /// Those of `Database::make_index`; SQLCODE -103 if an index that
    /// embeds its text names more than its text column, or its table's
    /// primary key is not one column, or its MODEL is no name, or another
    /// hybrid index has a CHUNKER or a MODE; those of the statements that
    /// make its table and pipeline.
    pub(super) fn index_made(&self, st: &State, spec: &IndexSpec) -> Result<()> {
        if !embeds(&spec.method, &spec.options) {
            if spec.method == HYBRID_METHOD && [CHUNKER_OPTION, MODE_OPTION].iter().any(|o| text_of(&spec.options, o).is_some()) {
                return Err(refused("a hybrid index's CHUNKER and MODE are of one that embeds its text by a MODEL"));
            }
            return self.db.noted(self.db.make_index(spec));
        }
        let [text] = spec.columns.as_slice() else {
            return Err(refused("a hybrid index of a MODEL indexes its text column alone, and embeds it"));
        };
        let desc = st.catalog.table(&spec.table).ok_or_else(|| refused(&format!("{} is not a base table", spec.table)))?;
        let key = match desc.primary_key() {
            Some([key]) => desc.columns.get(*key).map(|c| c.name.as_str()),
            Some(_) | None => None,
        };
        let key = key
            .ok_or_else(|| refused(&format!("a hybrid index of a MODEL keys its chunks by {}'s primary key of one column", spec.table)))?;
        let model = text_of(&spec.options, MODEL_OPTION).and_then(model_named).ok_or_else(|| refused("HYBRID.MODEL names no model"))?;
        let model = qualify(&model, &self.default_schema());
        self.kept_run(&statements(spec, (key, text), &model))?;
        let (chunks, _) = kept(&spec.name, &spec.table.auth);
        let columns = INDEXED.iter().map(|c| (*c).to_owned()).collect();
        let indexed = self.db.noted(self.db.make_index(&IndexSpec { table: chunks, columns, ..spec.clone() }));
        if indexed.is_err() {
            let _ = self.kept_dropped(&spec.name, &spec.table.auth);
        }
        indexed
    }

    /// The index `ix` dropped: of a hybrid index that embeds its text,
    /// its pipeline and its table of chunks then.
    ///
    /// # Errors
    ///
    /// Those of `Database::drop_index`, and of the statements that drop
    /// the pipeline and the table.
    pub(super) fn index_dropped(&self, ix: &IndexDesc) -> Result<()> {
        self.db.drop_index(&ix.name)?;
        if !embeds(&ix.method, &ix.options) {
            return Ok(());
        }
        self.kept_dropped(&ix.name, &ix.table.auth)
    }

    /// The pipeline and the table of chunks of the index `name` of schema
    /// `schema` dropped, in a transaction of their own.
    fn kept_dropped(&self, name: &str, schema: &str) -> Result<()> {
        let (chunks, pipeline) = kept(name, schema);
        self.kept_run(&[format!("DROP VECTOR PIPELINE {}", written(&pipeline)), format!("DROP TABLE {} CASCADE", written(&chunks))])
    }

    /// `statements` run, then committed, in a session of this session's
    /// user, as this one runs none while its index statement runs; rolled
    /// back if one fails. An error is placed at none of its text.
    fn kept_run(&self, statements: &[String]) -> Result<()> {
        let other = self.db.session(&self.auth());
        let ran = statements.iter().try_for_each(|sql| other.run(sql).map(|_| ())).and_then(|()| other.run("COMMIT").map(|_| ()));
        if ran.is_err() {
            let _ = other.run("ROLLBACK");
        }
        ran.map_err(|e| SqlError { pos: None, ..e })
    }
}
