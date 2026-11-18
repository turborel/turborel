//! What each list of HYBRID_SEARCH reads, of the hybrid index it names.
//! Of an index of a text and a vector column, both lists read its table,
//! each through its part. Of one that embeds its text
//! (`db/index_statements/hybrid_embedded.rs`), the text list reads the
//! table of documents through the index, a document a row, and the vector
//! list the table of chunks through the index of their embeddings.

use super::{quoted, refused, string};
use crate::ast::HybridSearch;
use crate::catalog::{IndexDesc, QualifiedName, TableDesc};
use crate::db::State;
use crate::error::Result;
use crate::index::vector::hybrid::{CHUNKS_SUFFIX, HYBRID_METHOD, PIPELINE_SUFFIX, STATE_OPTION, STATES, VECTORS_SUFFIX, embeds};
use crate::value::Value;

/// The columns of the table of chunks of an index that embeds its text.
const DOC_ID: &str = "DOC_ID";
const CHUNK_ID: &str = "CHUNK_ID";
const CHUNK_TEXT: &str = "CHUNK_TEXT";
const EMBEDDING: &str = "EMBEDDING";

/// The text list's read: its table, key and text column, quoted.
pub(super) struct Ranked {
    pub(super) table: String,
    pub(super) key: String,
    pub(super) text: String,
}

/// The vector list's read: its table, as a hint names it, its key, vector
/// and text columns, quoted, and the index's name and metric.
pub(super) struct Nearest {
    pub(super) table: String,
    pub(super) ident: String,
    pub(super) key: String,
    pub(super) vector: String,
    pub(super) text: String,
    pub(super) index: String,
    pub(super) metric: Option<String>,
}

/// What `search` reads: each list's read; of an index that embeds its
/// text, its model, as a string, and its table of chunks, from which the
/// text list takes a document's best chunk.
pub(super) struct Read {
    pub(super) ranked: Ranked,
    pub(super) nearest: Nearest,
    pub(super) model: Option<String>,
    pub(super) chunks: Option<String>,
}

/// `qn` as written: two delimited identifiers.
fn written(qn: &QualifiedName) -> String {
    format!("{}.{}", quoted(&qn.auth), quoted(&qn.ident))
}

/// The name of the column of `ix`'s key at `at`, quoted.
fn column_name(desc: &TableDesc, ix: &IndexDesc, at: usize) -> Result<String> {
    let column = ix.columns.get(at).and_then(|c| desc.columns.get(*c));
    column.map(|c| quoted(&c.name)).ok_or_else(|| refused(format!("the index {} has no column {at}", ix.name)))
}

/// The column of DOC_ID, quoted: the table's primary key of one column.
fn document_key(desc: &TableDesc, ix: &IndexDesc) -> Result<String> {
    let key = match desc.primary_key() {
        Some([key]) => desc.columns.get(*key),
        Some(_) | None => None,
    };
    key.map(|c| quoted(&c.name))
        .ok_or_else(|| refused(format!("HYBRID_SEARCH's DOC_ID is a primary key of one column, which {} has not", ix.table)))
}

impl Read {
    /// What `search` reads, of the hybrid index it names.
    pub(super) fn of(db: &State, search: &HybridSearch) -> Result<Read> {
        let ix = db.catalog.index(&search.index).filter(|ix| ix.method == HYBRID_METHOD);
        let ix = ix.ok_or_else(|| refused(format!("HYBRID_SEARCH's INDEX {} is no hybrid index", search.index)))?;
        let desc = db.catalog.table(&ix.table).ok_or_else(|| refused(format!("there is no table {}", ix.table)))?;
        let ranked = Ranked { table: written(&ix.table), key: document_key(desc, ix)?, text: column_name(desc, ix, 0)? };
        if embeds(&ix.method, &ix.options) {
            return Read::embedded(db, ix, ranked);
        }
        let nearest = Nearest {
            table: written(&ix.table),
            ident: quoted(&ix.table.ident),
            key: ranked.key.clone(),
            vector: column_name(desc, ix, 1)?,
            text: ranked.text.clone(),
            index: quoted(&ix.name),
            metric: db.methods.get(HYBRID_METHOD).and_then(|m| m.metric(ix)),
        };
        Ok(Read { ranked, nearest, model: None, chunks: None })
    }

    /// What a search of `ix`, which embeds its text, reads.
    fn embedded(db: &State, ix: &IndexDesc, ranked: Ranked) -> Result<Read> {
        let unready = |(n, v): &(String, Value)| if let Value::Char(state) = v { n == STATE_OPTION && state != STATES[0] } else { false };
        let ready = !ix.options.iter().any(unready);
        let of = |suffix: &str| QualifiedName { auth: ix.table.auth.clone(), ident: format!("{}{suffix}", ix.name) };
        let (chunks, piped) = (of(CHUNKS_SUFFIX), of(PIPELINE_SUFFIX));
        let indexed = db.catalog.index(&format!("{}{VECTORS_SUFFIX}", ix.name)).filter(|_| ready);
        let kept = db.catalog.embeddings().pipeline(&piped).zip(indexed);
        let (pipeline, vectors) = kept.ok_or_else(|| refused(format!("the index {} is being made or dropped", ix.name)))?;
        let nearest = Nearest {
            table: written(&chunks),
            ident: quoted(&chunks.ident),
            key: quoted(DOC_ID),
            vector: quoted(EMBEDDING),
            text: quoted(CHUNK_TEXT),
            index: quoted(&vectors.name),
            metric: db.methods.get(&vectors.method).and_then(|m| m.metric(vectors)),
        };
        Ok(Read { ranked, nearest, model: Some(string(&written(&pipeline.model))), chunks: Some(written(&chunks)) })
    }
}

/// The text of the column CHUNK_ID of a table of chunks.
pub(super) fn chunk_id() -> String {
    quoted(CHUNK_ID)
}
