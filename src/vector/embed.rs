//! \[EXTENSION\] Embedding generation, the vector design's §12: models
//! as plug-ins (`Embedder`), the registry of them (`Embedders`), the
//! routines EMBED, CHUNK_TEXT and EMBED_VERSION (`routines.rs`), and the
//! models of the catalog bound to their plug-ins (`bind.rs`, `model.rs`),
//! which pipelines and auto-embedded columns embed by (`db/embedding.rs`).
//!
//! A model is a plug-in, as a ranking of full text is. A local ONNX
//! model or a remote one over HTTP is an `Embedder` its host registers,
//! the runtime or client its own (R12.2), and turborel holds none. The
//! built-in models hash features (`hashed.rs`), deterministic and needing
//! no file, for tests, for lexical sparse retrieval, and as a fallback.
//!
//! An embedding is a `Vector`, whose value is the VECTOR type's canonical
//! image (§4.1, §4.2).

mod bind;
mod hashed;
mod model;
mod routines;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub use bind::{Configured, DATABASE_OPTIONS, DIMENSIONS, FILE, FORMAT, LAYOUT, SHAPE, VERSION, configure_shape, number_option};
pub use hashed::{HASH_DENSE, HASH_DENSE_DIMS, HASH_SPARSE, HASH_SPARSE_DIMS, HashedEmbedder};
pub use model::{BoundEmbed, BoundModel, BoundVersion, MOST_PRIMED, tag_type};
pub use routines::text_of;
pub use routines::{CHUNK_NAMES, ChunkFunction, EMBED_NAMES, EMBED_VERSION, EmbedFunction, MOST_TEXT_BYTES, routines};
use routines::{PURPOSES, data};

use super::chunk::{Span, words_of};
use super::{Element, Layout, Vector};
use crate::error::{Code, Result, SqlError};
use crate::sync::{lock, read, write};
use crate::value::DataType;

/// \[EXTENSION\] R12.3: the texts a model is given at once by default.
pub const DEFAULT_BATCH: usize = 64;

/// \[EXTENSION\] What a model's vectors are: R12.1's VECTOR(DIMENSIONS,
/// FORMAT), dense or sparse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelShape {
    /// The dimension count: of a sparse vector, its vocabulary.
    pub dims: u32,
    /// The element format.
    pub element: Element,
    /// The layout.
    pub layout: Layout,
}

impl ModelShape {
    /// The VECTOR type of its vectors (R12.1).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the VECTOR type refuses the shape (R3.4, R3.8).
    pub fn data_type(self) -> Result<DataType> {
        super::typed(super::Shape { dims: Some(self.dims), element: Some(self.element), layout: Some(self.layout) })
    }

    /// Whether `v` has the shape.
    pub fn fits(self, v: &Vector) -> bool {
        v.dims == self.dims && v.element == self.element && v.layout() == self.layout
    }
}

/// The type of any vector: of EMBED's result while its model is not
/// known as a statement is bound.
///
/// # Errors
///
/// None in practice; as `typed`.
pub fn any_vector() -> Result<DataType> {
    super::typed(super::Shape { dims: None, element: None, layout: None })
}

/// \[EXTENSION\] What a text is embedded for (§12.2's third argument of
/// EMBED). A model may prefix each kind of text as it was trained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// A query.
    Query,
    /// A document.
    Document,
}

/// \[EXTENSION\] R12.4: what EMBED does when its model fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnError {
    /// The statement fails, the default.
    Fail,
    /// The text's vector is null.
    Null,
    /// The model is asked again, this many times more, then it fails.
    Retry(u32),
}

/// \[EXTENSION\] A model: what makes vectors of texts (§12.1).
///
/// Contract (RULES.md R-SAFE-3, checked for the built-in models by
/// `embed/tests`):
/// * `name` is fixed, an upper-case SQL identifier, possibly qualified
///   (`APP.MINILM_L12`); EMBED and pipelines name the model by it.
/// * `shape` is fixed for the model's life. `embed` gives one vector a
///   text, in order, each of that shape (`ModelShape::fits`). A model's
///   own output format is its to convert to `shape`'s.
/// * `version` names the weights; a changed model gives another (R12.5).
/// * If `deterministic`, equal texts give equal embeddings, bit for bit,
///   whatever the batch they are in; a model whose answers may vary, as a
///   remote one's may, returns false, and no generated column uses it.
/// * `tokens` gives the spans, in characters from 0, of the tokens of a
///   text as the model counts them, for CHUNK_TEXT BY TOKENS; it never
///   fails, as a tokenizer is part of the model.
/// * Methods never panic, may be called from several threads at once
///   (`Send + Sync`), and never call back into the database (R-ERR-4).
///   They are called holding no row lock (R12.4), so a remote call may
///   wait; its time is the model's own to bound (its TIMEOUT_MS).
pub trait Embedder: Send + Sync {
    /// Its name.
    fn name(&self) -> &str;
    /// Its vectors' shape.
    fn shape(&self) -> ModelShape;
    /// The version of its weights. Default: 1.
    fn version(&self) -> u32 {
        1
    }
    /// Whether equal texts give equal vectors. Default: true.
    fn deterministic(&self) -> bool {
        true
    }
    /// The most texts it is given at once (R12.3). Default: 64.
    fn batch(&self) -> usize {
        DEFAULT_BATCH
    }
    /// The most tokens of a text it reads, if it cuts texts (its
    /// MAX_TOKENS); a longer text is read only in part. Default: none.
    fn most_tokens(&self) -> Option<usize> {
        None
    }
    /// The text put before each text embedded for `purpose`. Default:
    /// none.
    fn prefix(&self, _purpose: Purpose) -> &'static str {
        ""
    }
    /// What EMBED does if it fails. Default: fail.
    fn on_error(&self) -> OnError {
        OnError::Fail
    }
    /// The model this one is as a model of the catalog configures it
    /// with `options` (§12.1's WITH, FROM FILE given as FILE): none if it
    /// is this one as it is. Default: this one, if each option is
    /// DIMENSIONS, FORMAT or LAYOUT agreeing with its shape
    /// (`configure_shape`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for an option it does not take, or a value it refuses.
    fn configured(&self, options: &[crate::catalog::ModelOption]) -> Result<Option<Arc<dyn Embedder>>> {
        configure_shape(self.shape(), options).map(|_| None)
    }
    /// The spans of the tokens of `text`. Default: its words.
    fn tokens(&self, text: &str) -> Vec<Span> {
        words_of(&text.chars().collect::<Vec<_>>())
    }
    /// The embeddings of `texts`, one each, in order.
    ///
    /// # Errors
    ///
    /// The model could not embed them. The statement fails, or the texts'
    /// vectors are null, as `on_error` says.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vector>>;
}

fn refused(detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] {detail}"))
}

/// \[EXTENSION\] The models registered, by name.
#[derive(Default)]
pub struct Embedders {
    models: RwLock<Vec<Arc<dyn Embedder>>>,
    /// The models of the catalog bound, each by what it was bound of
    /// (`bind.rs`).
    bound: Mutex<Vec<(String, Arc<BoundModel>)>>,
    /// Whether the database has had, since it opened, a column embedded
    /// after commit or of a model's version (`db/embedding/queue.rs`).
    watching: AtomicBool,
    /// What the queue of embeddings after commit did of each column.
    queue: Mutex<Vec<QueueState>>,
    /// Held by a round of the queue, so one runs at a time.
    rounds: Mutex<()>,
}

/// \[EXTENSION\] What the queue of embeddings after commit did of one
/// column, as INFORMATION_SCHEMA.EMBEDDING_QUEUE shows it (R12.6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueState {
    /// The schema of the table.
    pub schema: String,
    /// The table.
    pub table: String,
    /// The column of vectors.
    pub column: String,
    /// The model, as its version tag names it.
    pub model: String,
    /// The rows waiting for a vector as the last round began.
    pub pending: u64,
    /// The rows given one since the database opened.
    pub embedded: u64,
    /// The rounds that failed since then.
    pub failed: u64,
    /// The last failure's message, if the last round failed.
    pub error: Option<String>,
}

impl Embedders {
    /// No models.
    pub fn new() -> Embedders {
        Embedders::default()
    }

    /// The built-in models: HASH_DENSE and HASH_SPARSE.
    pub fn built_in() -> Embedders {
        let models: Vec<Arc<dyn Embedder>> = vec![Arc::new(HashedEmbedder::dense()), Arc::new(HashedEmbedder::sparse())];
        Embedders { models: RwLock::new(models), ..Embedders::default() }
    }

    /// Registers `model`.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if one of its name is registered, or its name is not
    /// upper case; SQLCODE -910 if the registry was poisoned.
    pub fn register(&self, model: Arc<dyn Embedder>) -> Result<()> {
        let name = model.name();
        if name.is_empty() || name != name.to_uppercase() {
            return Err(refused(&format!("a model's name is upper case, not {name}")));
        }
        let mut models = write(&self.models)?;
        if models.iter().any(|m| m.name() == name) {
            return Err(refused(&format!("a model {name} is registered")));
        }
        models.push(model);
        Ok(())
    }

    /// The model `name` names, in any case.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the registry was poisoned.
    pub fn named(&self, name: &str) -> Result<Option<Arc<dyn Embedder>>> {
        let wanted = name.trim().to_uppercase();
        Ok(read(&self.models)?.iter().find(|m| m.name() == wanted).cloned())
    }

    /// The names of the models, in the order registered.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the registry was poisoned.
    pub fn names(&self) -> Result<Vec<String>> {
        Ok(read(&self.models)?.iter().map(|m| m.name().to_owned()).collect())
    }

    /// Notes that a column is embedded after commit, or of a model's
    /// version, so each commit that writes asks for the queue.
    pub fn watch(&self) {
        self.watching.store(true, Ordering::Release);
    }

    /// Whether a column has been embedded after commit, or of a model's
    /// version, since the database opened.
    pub fn watching(&self) -> bool {
        self.watching.load(Ordering::Acquire)
    }

    /// A round of the queue's turn: held while it runs.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it was poisoned.
    pub fn round(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        lock(&self.rounds)
    }

    /// What the queue did of each column.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it was poisoned.
    pub fn queue(&self) -> Result<Vec<QueueState>> {
        Ok(lock(&self.queue)?.clone())
    }

    /// `state` recorded, in place of the one of its column.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the queue's states were poisoned.
    pub fn record(&self, state: QueueState) -> Result<()> {
        let mut all = lock(&self.queue)?;
        let key = |s: &QueueState| (s.schema.clone(), s.table.clone(), s.column.clone());
        all.retain(|s| key(s) != key(&state));
        all.push(state);
        all.sort_by_key(key);
        Ok(())
    }

    /// Whether every model registered is deterministic.
    pub fn deterministic(&self) -> bool {
        read(&self.models).is_ok_and(|models| models.iter().all(|m| m.deterministic()))
    }
}

#[cfg(test)]
mod tests;
