//! \[EXTENSION\] A hybrid index that embeds its text itself (the vector
//! design document §11.1): CREATE HYBRID VECTOR INDEX `i` ON `t` (`text`)
//! with a MODEL, and a CHUNKER and a MODE if any. "The index stores: a
//! full-text index on body, the chunks, their embeddings and an ANN index
//! over the embeddings", kept as the text changes, synchronously or after
//! commit (R12.6).
//!
//! It is made of what the embedding models' design gives (`db/embedding.rs`).
//! The hybrid index `i` is of `t`'s text alone, its full-text part, so
//! CONTAINS and RANK of the text read it, a document at a time, and it
//! keeps the options given, so INFORMATION_SCHEMA shows its model. A table
//! `i_CHUNKS` (DOC_ID, CHUNK_ID, CHUNK_TEXT, EMBEDDING) holds the chunks,
//! of the types CHUNK_TEXT and EMBED of the model give. An index
//! `i_VECTORS` of the vector part's method and options orders their
//! embeddings. A vector pipeline `i_PIPELINE` keeps the table in step
//! with `t`, its key and its text, in the mode given, filling it as it is
//! made. HYBRID_SEARCH of `i` reads `t` through `i` and the chunks through
//! `i_VECTORS` (`binder/hybrid_search.rs`).
//!
//! An index statement runs outside a transaction, and each of these is
//! made, and committed, in turn. The index `i` is made first, its
//! HYBRID.STATE BUILDING, and set READY once the others are, the
//! pipeline last. It is set DROPPING before they are dropped, the
//! pipeline first, and dropped last. A statement that fails drops what it
//! made. A database that stopped between them drops, as it opens, each
//! such index not READY and what it keeps (`hybrid_settled.rs`), so none
//! is left half made.

use super::super::indexes::IndexSpec;
use super::super::{Session, State};
use super::refused;
use crate::binder::qualify;
use crate::catalog::{IndexDesc, QualifiedName};
use crate::db::embedding::{delimited, model_named, written};
use crate::error::{Code, Result, SqlError};
use crate::index::vector::hybrid::{
    CHUNKER_OPTION, CHUNKS_SUFFIX, HYBRID_METHOD, MODE_OPTION, MODEL_OPTION, PIPELINE_SUFFIX, STATE_OPTION, STATES, VECTORS_SUFFIX, embeds,
    vector_part,
};
use crate::value::Value;
use crate::vector::chunk::{Spec, quote};

/// The column of the table of chunks its vector index is of.
const EMBEDDING: &str = "EMBEDDING";

/// The text option `name` of `options`, if set and not empty.
pub(super) fn text_of<'o>(options: &'o [(String, Value)], name: &str) -> Option<&'o str> {
    options
        .iter()
        .find(|(n, _)| n == name)
        .and_then(|(_, v)| if let Value::Char(s) = v { Some(s.trim()) } else { None })
        .filter(|s| !s.is_empty())
}

/// What the index `name` of a table of schema `schema` keeps beside it:
/// its table of chunks, its pipeline, and its vector index's name.
pub(super) fn kept(name: &str, schema: &str) -> (QualifiedName, QualifiedName, String) {
    let of = |suffix: &str| QualifiedName { auth: schema.to_owned(), ident: format!("{name}{suffix}") };
    (of(CHUNKS_SUFFIX), of(PIPELINE_SUFFIX), format!("{name}{VECTORS_SUFFIX}"))
}

/// The options of `options` with HYBRID.STATE `state`.
fn in_state(options: &[(String, Value)], state: &str) -> Vec<(String, Value)> {
    let mut set: Vec<(String, Value)> = options.iter().filter(|(n, _)| n != STATE_OPTION).cloned().collect();
    set.push((STATE_OPTION.to_owned(), Value::Char(state.to_owned())));
    set
}

/// What an index that embeds its text is made of: its table's key and
/// text columns, and its model.
pub(super) struct Embedding {
    pub(super) key: String,
    pub(super) text: String,
    pub(super) model: QualifiedName,
}

/// The statement that makes the table of chunks of the index `spec`
/// describes, and the one that makes its pipeline.
fn statements(spec: &IndexSpec, of: &Embedding) -> [String; 2] {
    let (chunks, pipeline, _) = kept(&spec.name, &spec.table.auth);
    let chunk = text_of(&spec.options, CHUNKER_OPTION).map_or_else(|| Spec::default().written(), str::to_owned);
    let mode = text_of(&spec.options, MODE_OPTION).unwrap_or("SYNC");
    let (table, chunks, key, text) = (written(&spec.table), written(&chunks), delimited(&of.key), delimited(&of.text));
    [
        format!(
            "CREATE TABLE {chunks} (DOC_ID, CHUNK_ID, CHUNK_TEXT, {EMBEDDING}) AS (SELECT S.{key}, U.CHUNK_ID, U.CHUNK_TEXT, \
             EMBED({}, U.CHUNK_TEXT) FROM {table} AS S, UNNEST(CHUNK_TEXT(S.{text}, {})) AS U (CHUNK_ID, CHUNK_OFFSET, \
             CHUNK_LENGTH, CHUNK_TEXT)) WITH NO DATA",
            quote(&written(&of.model)),
            quote(&chunk),
        ),
        format!(
            "CREATE VECTOR PIPELINE {} SOURCE {table} ({key}, {text}) CHUNK {chunk} EMBED USING {} TARGET {chunks} \
             (DOC_ID, CHUNK_ID, CHUNK_TEXT, {EMBEDDING}) MODE {mode}",
            written(&pipeline),
            written(&of.model),
        ),
    ]
}

/// The index of the chunks' embeddings of the index `spec` describes: of
/// its vector part's method and options.
fn vectors_of(spec: &IndexSpec) -> IndexSpec {
    let (chunks, _, vectors) = kept(&spec.name, &spec.table.auth);
    let desc = IndexDesc {
        name: spec.name.clone(),
        table: spec.table.clone(),
        columns: vec![0, 1],
        method: spec.method.clone(),
        unique: false,
        options: spec.options.clone(),
        collations: Vec::new(),
        nulls: false,
        expressions: Vec::new(),
    };
    let (method, options) = vector_part(&desc);
    IndexSpec { name: vectors, table: chunks, columns: vec![EMBEDDING.to_owned()], method, options, nulls: false, expressions: Vec::new() }
}

impl Session {
    /// The index `spec` describes made, on `st`: of a hybrid index that
    /// embeds its text, with what it keeps beside it.
    ///
    /// # Errors
    ///
    /// Those of `Database::make_index`; SQLCODE -103 if HYBRID.STATE is
    /// given, or a hybrid index of no MODEL has a CHUNKER or a MODE, and
    /// those of `embedding`; -900 if one of no MODEL names one column.
    pub(super) fn index_made(&self, st: &State, spec: &IndexSpec) -> Result<()> {
        if spec.method != HYBRID_METHOD {
            return self.db.noted(self.db.make_index(spec));
        }
        if text_of(&spec.options, STATE_OPTION).is_some() {
            return Err(refused("a hybrid index sets its HYBRID.STATE itself"));
        }
        if embeds(&spec.method, &spec.options) {
            let of = self.embedding(st, spec)?;
            return self.embedded_made(spec, &of);
        }
        if [CHUNKER_OPTION, MODE_OPTION].iter().any(|o| text_of(&spec.options, o).is_some()) {
            return Err(refused("a hybrid index's CHUNKER and MODE are of one that embeds its text by a MODEL"));
        }
        if spec.columns.len() < 2 {
            return Err(SqlError::new(
                Code::Unsupported,
                "[EXTENSION] a hybrid index of one column embeds it by a MODEL; of none, it is of a text and a vector column (§11.1)",
            ));
        }
        self.db.noted(self.db.make_index(spec))
    }

    /// What the index `spec` describes, of a MODEL, is made of.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it names more than its text column, its table's
    /// primary key is not one column, its MODEL is no name, or a name of
    /// what it keeps beside it is taken.
    pub(super) fn embedding(&self, st: &State, spec: &IndexSpec) -> Result<Embedding> {
        let [text] = spec.columns.as_slice() else {
            return Err(refused("a hybrid index of a MODEL indexes its text column alone, and embeds it"));
        };
        let desc = st.catalog.table(&spec.table).ok_or_else(|| refused(&format!("{} is not a base table", spec.table)))?;
        let key = match desc.primary_key() {
            Some([key]) => desc.columns.get(*key).map(|c| c.name.clone()),
            Some(_) | None => None,
        };
        let key = key
            .ok_or_else(|| refused(&format!("a hybrid index of a MODEL keys its chunks by {}'s primary key of one column", spec.table)))?;
        let model = text_of(&spec.options, MODEL_OPTION).and_then(model_named).ok_or_else(|| refused("HYBRID.MODEL names no model"))?;
        Ok(Embedding { key, text: text.clone(), model: qualify(&model, &self.default_schema()) })
    }

    /// Whether a name of what the index `spec` describes keeps beside it
    /// is taken on `st`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one is.
    pub(super) fn kept_free(st: &State, spec: &IndexSpec) -> Result<()> {
        let (chunks, pipeline, vectors) = kept(&spec.name, &spec.table.auth);
        let taken = st.catalog.table(&chunks).is_some()
            || st.catalog.index(&vectors).is_some()
            || st.catalog.embeddings().pipelines.iter().any(|p| p.name == pipeline);
        if taken {
            return Err(refused(&format!("{chunks}, {pipeline} or {vectors}, which the index {} keeps, is taken", spec.name)));
        }
        Ok(())
    }

    /// The index `spec` describes, of a MODEL, made BUILDING, then what it
    /// keeps, then set READY; if one fails, what was made dropped.
    fn embedded_made(&self, spec: &IndexSpec, of: &Embedding) -> Result<()> {
        Session::kept_free(&*self.db.snapshot()?, spec)?;
        let building = IndexSpec { options: in_state(&spec.options, STATES[1]), ..spec.clone() };
        self.db.noted(self.db.make_index(&building))?;
        let made = self.kept_made(spec, of).and_then(|()| self.state_set(&spec.name, None));
        if made.is_err() {
            let _ = self.kept_undone(&spec.name, &spec.table.auth);
        }
        made
    }

    /// HYBRID.STATE of the index `name` set to `state`, READY if none.
    pub(super) fn state_set(&self, name: &str, state: Option<&str>) -> Result<()> {
        let value = state.map(|s| Value::Char(s.to_owned()));
        self.db.noted(self.db.alter_index(name, &[(STATE_OPTION.to_owned(), value)]))
    }

    /// What the index `spec` describes keeps beside it, made: its table of
    /// chunks, the vector index of their embeddings, then the pipeline,
    /// which fills the table of the rows there.
    pub(super) fn kept_made(&self, spec: &IndexSpec, of: &Embedding) -> Result<()> {
        let [table, pipeline] = statements(spec, of);
        self.kept_run(&[table])?;
        self.db.noted(self.db.make_index(&vectors_of(spec)))?;
        self.kept_run(&[pipeline])
    }

    /// The index `ix` dropped: of one that embeds its text, set DROPPING,
    /// then what it keeps dropped, then it.
    ///
    /// # Errors
    ///
    /// Those of `Database::drop_index`, and of the statements that drop
    /// the pipeline and the table.
    pub(super) fn index_dropped(&self, ix: &IndexDesc) -> Result<()> {
        if !embeds(&ix.method, &ix.options) {
            return self.db.drop_index(&ix.name);
        }
        self.state_set(&ix.name, Some(STATES[2]))?;
        self.kept_undone(&ix.name, &ix.table.auth)
    }

    /// What the index `name` of schema `schema` keeps beside it dropped,
    /// each if it is there, the pipeline first, then the index.
    pub(super) fn kept_undone(&self, name: &str, schema: &str) -> Result<()> {
        self.kept_dropped(name, schema)?;
        if self.db.snapshot()?.catalog.index(name).is_some() {
            self.db.drop_index(name)?;
        }
        Ok(())
    }

    /// What the index `name` of schema `schema` keeps beside it dropped,
    /// each if it is there, the pipeline first.
    pub(super) fn kept_dropped(&self, name: &str, schema: &str) -> Result<()> {
        let (chunks, pipeline, vectors) = kept(name, schema);
        let st = self.db.snapshot()?;
        if st.catalog.embeddings().pipelines.iter().any(|p| p.name == pipeline) {
            self.kept_run(&[format!("DROP VECTOR PIPELINE {}", written(&pipeline))])?;
        }
        if st.catalog.index(&vectors).is_some() {
            self.db.drop_index(&vectors)?;
        }
        if st.catalog.table(&chunks).is_some() {
            self.kept_run(&[format!("DROP TABLE {} CASCADE", written(&chunks))])?;
        }
        Ok(())
    }

    /// `statements` run, then committed, in a session of this session's
    /// user, as this one runs none while its index statement runs; rolled
    /// back if one fails. An error is placed at none of its text.
    pub(super) fn kept_run(&self, statements: &[String]) -> Result<()> {
        let other = self.db.session(&self.auth());
        let ran = statements.iter().try_for_each(|sql| other.run(sql).map(|_| ())).and_then(|()| other.run("COMMIT").map(|_| ()));
        if ran.is_err() {
            let _ = other.run("ROLLBACK");
        }
        ran.map_err(|e| SqlError { pos: None, ..e })
    }
}
