//! \[EXTENSION\] The hybrid index method, "hybrid", of the vector design
//! document's §10.1 ("HYBRID: full-text index + vector index over the
//! same documents") and §11.1 (CREATE HYBRID VECTOR INDEX): an index of
//! two columns, a document's text and its embedding, holding a full-text
//! index of the text (RULES.md D86) and a vector index of the embedding,
//! HNSW by default (R10.2), or IVF, DiskANN, or the inverted index of a
//! SPARSE column (`index/sparse.rs`), as HYBRID.VECTOR_METHOD names.
//!
//! Each part is made by its own method, from the options of that method's
//! group (`options.rs`), so a hybrid index reads its documents and orders
//! its vectors as an index of either method would. Its parts are kept in
//! the method's file and its memory as their methods keep their own; a
//! checkpoint checkpoints them. The query that reads it is HYBRID_SEARCH
//! (§11.2, `binder/hybrid_search.rs`), which reads a top-k of each part
//! and fuses them; an ordinary query reads either part as it would an
//! index of the part's method (`index.rs`).
//!
//! An index that embeds its text itself, by a MODEL and a CHUNKER (§11.1,
//! §12), is one of these over a table of the text's chunks and their
//! embeddings, which a vector pipeline keeps
//! (`db/index_statements/hybrid_embedded.rs`). Its options HYBRID.MODEL,
//! HYBRID.CHUNKER and HYBRID.MODE say how, and its parts read none of
//! them.

mod index;
mod options;

use std::sync::Arc;

pub use options::{CHUNKER_OPTION, MODE_OPTION, MODEL_OPTION, VECTOR_METHOD_OPTION};

use self::index::{HybridIndex, TEXT, VECTOR};
use crate::catalog::{IndexDesc, TableDesc};
use crate::disk::buffer::{BufferPool, FileId};
use crate::engine::MemoryPages;
use crate::error::{Code, Result, SqlError};
use crate::fulltext::rank::Rankings;
use crate::index::{
    DiskAnnMethod, FullTextMethod, HnswMethod, Index, IndexMethod, IvfMethod, NearestQuery, NearestSupport, Predicate, SparseMethod,
};
use crate::memory::Host;
use crate::storage::TableOption;
use crate::value::{CompOp, DataType};
use crate::vector::metric::Metrics;
use crate::vector::quantizer::Quantizers;

/// The name of `HybridMethod`.
pub const HYBRID_METHOD: &str = "hybrid";

fn unsupported(detail: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[EXTENSION] the hybrid index method {detail}"))
}

/// \[EXTENSION\] The hybrid index method: a full-text method and the
/// vector methods, each making its part.
pub struct HybridMethod {
    text: FullTextMethod,
    hnsw: HnswMethod,
    ivf: IvfMethod,
    diskann: DiskAnnMethod,
    sparse: SparseMethod,
}

impl HybridMethod {
    /// Stores its indexes in the registered file of this buffer pool.
    pub fn new(pool: &Arc<BufferPool>, file: FileId) -> HybridMethod {
        let pool = || Arc::clone(pool);
        HybridMethod {
            text: FullTextMethod::new(pool(), file),
            hnsw: HnswMethod::new(pool(), file),
            ivf: IvfMethod::new(pool(), file),
            diskann: DiskAnnMethod::new(pool(), file),
            sparse: SparseMethod::new(pool(), file),
        }
    }

    /// A method keeping its indexes in `memory`, which others may share.
    pub(crate) fn sharing(memory: &Arc<MemoryPages>) -> HybridMethod {
        let memory = || Arc::clone(memory);
        HybridMethod {
            text: FullTextMethod::sharing(memory()),
            hnsw: HnswMethod::sharing(memory()),
            ivf: IvfMethod::sharing(memory()),
            diskann: DiskAnnMethod::sharing(memory()),
            sparse: SparseMethod::sharing(memory()),
        }
    }

    /// \[EXTENSION\] The method, its dense vector parts ordering by the
    /// metrics and compressing by the quantizers `plugins` gives: the
    /// database's.
    #[must_use]
    pub fn with_plugins(self, (metrics, quantizers): &(Metrics, Quantizers)) -> HybridMethod {
        let (m, q) = (|| Arc::clone(metrics), || Arc::clone(quantizers));
        HybridMethod {
            hnsw: self.hnsw.with_metrics(m()).with_quantizers(q()),
            ivf: self.ivf.with_metrics(m()).with_quantizers(q()),
            diskann: self.diskann.with_metrics(m()).with_quantizers(q()),
            ..self
        }
    }

    /// Its vector methods, HNSW first, the default.
    fn vectors(&self) -> [&dyn IndexMethod; 4] {
        [&self.hnsw, &self.ivf, &self.diskann, &self.sparse]
    }

    /// \[EXTENSION\] The method, its text parts naming their rankings
    /// among `rankings` (FULLTEXT.RANKING), not the built-in alone.
    #[must_use]
    pub fn with_rankings(self, rankings: Rankings) -> HybridMethod {
        HybridMethod { text: self.text.with_rankings(rankings), ..self }
    }

    /// The method of the vector part `desc` describes.
    fn vector_method(&self, desc: &IndexDesc) -> Result<&dyn IndexMethod> {
        let name = options::vector_method(desc);
        let found = self.vectors().into_iter().find(|m| m.name() == name);
        found.ok_or_else(|| unsupported(&format!("has no vector method {name}")))
    }

    /// The index of `columns` `on` describes, of its options, or with
    /// every option its default if none, as `create` and `open` are
    /// given; made anew or, of a locator, opened; none if a part keeps
    /// nothing to open.
    fn index(&self, columns: &[DataType], on: Option<(&TableDesc, &IndexDesc)>, locator: Option<&[u8]>) -> Result<Option<Box<dyn Index>>> {
        let [text_type, vector_type] = columns else {
            return Err(unsupported("indexes two columns, a document's text and its embedding (§11.1)"));
        };
        let (text_type, vector_type) = (std::slice::from_ref(text_type), std::slice::from_ref(vector_type));
        if on.is_some_and(|(_, desc)| desc.unique) {
            return Err(unsupported("makes no UNIQUE index"));
        }
        let parts = on.map(|(table, desc)| (table, options::parts(desc)));
        let vector_method = match on {
            Some((_, desc)) => self.vector_method(desc)?,
            None => &self.hnsw,
        };
        let (text, vector) = match (locator.map(options::located).transpose()?, parts) {
            (Some((t, v)), Some((table, (td, vd)))) => {
                (self.text.open_on(text_type, t, (table, &td))?, vector_method.open_on(vector_type, v, (table, &vd)))
            }
            (Some((t, v)), None) => (self.text.open(text_type, t)?, vector_method.open(vector_type, v)),
            (None, Some((table, (td, vd)))) => {
                (Some(self.text.create_on(text_type, (table, &td))?), vector_method.create_on(vector_type, (table, &vd)).map(Some))
            }
            (None, None) => (Some(self.text.create(text_type)?), vector_method.create(vector_type).map(Some)),
        };
        both(text, vector)
    }
}

/// The index of parts `text` and `vector`, if both are; else, or if the
/// vector part failed, the text part discarded, so its pages are freed.
fn both(text: Option<Box<dyn Index>>, vector: Result<Option<Box<dyn Index>>>) -> Result<Option<Box<dyn Index>>> {
    match (text, vector) {
        (Some(text), Ok(Some(vector))) => Ok(Some(Box::new(HybridIndex::new(text, vector)))),
        (Some(text), Ok(None)) => {
            text.discard();
            Ok(None)
        }
        (Some(text), Err(error)) => {
            text.discard();
            Err(error)
        }
        (None, vector) => {
            if let Ok(Some(vector)) = &vector {
                vector.discard();
            }
            vector.map(|_| None)
        }
    }
}

impl IndexMethod for HybridMethod {
    /// Of its text column, as the full-text method serves a score of it;
    /// of its vector column, as a vector method serves a distance of it.
    fn supports_nearest(&self, query: &NearestQuery, columns: &[DataType]) -> NearestSupport {
        let ([text, vector], part) = (columns, NearestQuery { column: 0, ..query.clone() }) else { return NearestSupport::No };
        match query.column {
            TEXT => self.text.supports_nearest(&part, std::slice::from_ref(text)),
            VECTOR => {
                let served = self
                    .vectors()
                    .into_iter()
                    .any(|m| m.supports_nearest(&part, std::slice::from_ref(vector)) == NearestSupport::Approximate);
                if served { NearestSupport::Approximate } else { NearestSupport::No }
            }
            _ => NearestSupport::No,
        }
    }
    /// CONTAINS or TEXT_MATCHES of its text column, as the full-text
    /// method serves them of its one column.
    fn supports_predicate(&self, predicate: &Predicate, columns: &[DataType]) -> bool {
        let ([text, _], TEXT) = (columns, predicate.column) else { return false };
        self.text.supports_predicate(predicate, std::slice::from_ref(text))
    }
    /// The metric of its vector part.
    fn metric(&self, index: &IndexDesc) -> Option<String> {
        let (_, vector) = options::parts(index);
        self.vector_method(index).ok()?.metric(&vector)
    }
    /// As its vector part's method serves it.
    fn serves_metric(&self, index: &IndexDesc, call: &str) -> bool {
        let (_, vector) = options::parts(index);
        self.vector_method(index).is_ok_and(|m| m.serves_metric(&vector, call))
    }
    fn name(&self) -> &'static str {
        HYBRID_METHOD
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.text.attach(host)?;
        self.vectors().into_iter().try_for_each(|m| m.attach(host))
    }
    /// None: its key is a text and a vector, which have no order together.
    fn supports(&self, _op: CompOp) -> bool {
        false
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        self.index(columns, None, None)?.ok_or_else(|| unsupported("made no index"))
    }
    fn open(&self, columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        self.index(columns, None, Some(locator))
    }
    /// Its own, then the full-text method's, then each vector method's.
    fn index_options(&self) -> Vec<TableOption> {
        let mut all = options::declared();
        all.extend(self.text.index_options());
        self.vectors().into_iter().for_each(|m| all.extend(m.index_options()));
        all
    }
    fn create_on(&self, columns: &[DataType], on: (&TableDesc, &IndexDesc)) -> Result<Box<dyn Index>> {
        self.index(columns, Some(on), None)?.ok_or_else(|| unsupported("made no index"))
    }
    fn open_on(&self, columns: &[DataType], locator: &[u8], on: (&TableDesc, &IndexDesc)) -> Result<Option<Box<dyn Index>>> {
        self.index(columns, Some(on), Some(locator))
    }
    fn checkpoint(&self) -> Result<()> {
        self.text.checkpoint()?;
        self.vectors().into_iter().try_for_each(IndexMethod::checkpoint)
    }
}

#[cfg(test)]
mod tests;
