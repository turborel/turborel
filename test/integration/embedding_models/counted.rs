//! A model plug-in that counts what it is asked: COUNTED, of 4 dense
//! FLOAT32 dimensions, each vector the text's length then 1, 0, 0. It
//! fails for a text holding "bad", and for the first `failing` batches
//! asked; it records each batch's size and the options it was configured
//! with, a model's octets FROM BLOB among them, and gives the version
//! `version` holds. Registered under another name, it is that plug-in.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use turborel::catalog::ModelOption;
use turborel::db::Database;
use turborel::error::{Code, Result, SqlError};
use turborel::threads::JobPolicy;
use turborel::vector::embed::{Embedder, ModelShape};
use turborel::vector::{Element, Layout, Vector, VectorType};

/// What the plug-in was asked, shared by its configured copies.
#[derive(Default)]
pub struct Asked {
    /// The size of each batch asked for.
    pub batches: Mutex<Vec<usize>>,
    /// The options of each configuration, as `NAME=value`.
    pub options: Mutex<Vec<String>>,
    /// The batches left to fail.
    pub failing: AtomicUsize,
    /// The version it gives.
    pub version: AtomicU32,
}

impl Asked {
    /// The sizes of the batches asked for since the last call.
    pub fn taken(&self) -> Vec<usize> {
        std::mem::take(&mut *self.batches.lock().unwrap())
    }
}

/// The plug-in, of its name.
pub struct Counted {
    pub name: &'static str,
    pub asked: Arc<Asked>,
}

impl Embedder for Counted {
    fn name(&self) -> &'static str {
        self.name
    }
    fn shape(&self) -> ModelShape {
        ModelShape { dims: 4, element: Element::Float32, layout: Layout::Dense }
    }
    fn version(&self) -> u32 {
        self.asked.version.load(Ordering::SeqCst)
    }
    fn batch(&self) -> usize {
        8
    }
    fn configured(&self, options: &[ModelOption]) -> Result<Option<Arc<dyn Embedder>>> {
        let shown = options.iter().map(|o| format!("{}={}", o.name, o.value));
        self.asked.options.lock().unwrap().extend(shown);
        Ok(None)
    }
    fn loaded(&self, blob: &[u8], options: &[ModelOption]) -> Result<Option<Arc<dyn Embedder>>> {
        self.asked.options.lock().unwrap().push(format!("BLOB={blob:?}"));
        self.configured(options)
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vector>> {
        self.asked.batches.lock().unwrap().push(texts.len());
        let failing = self.asked.failing.load(Ordering::SeqCst);
        if failing > 0 {
            self.asked.failing.store(failing - 1, Ordering::SeqCst);
            return Err(SqlError::new(Code::Data, "the model is unreachable"));
        }
        if texts.iter().any(|t| t.contains("bad")) {
            return Err(SqlError::new(Code::Data, "the model refuses a text"));
        }
        texts
            .iter()
            .map(|t| Vector::dense(Element::Float32, vec![f64::from(u32::try_from(t.chars().count()).unwrap_or(0)), 1.0, 0.0, 0.0]))
            .collect()
    }
}

/// A database with VECTOR, schemas U and V, COUNTED registered, and the
/// background work run only when asked; and what COUNTED is asked.
pub fn database() -> (Database, Arc<Asked>) {
    let db = Database::new();
    prepared(&db);
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION V").unwrap();
    let asked = registered(&db);
    (db, asked)
}

/// `db` given VECTOR, and its background work run only when asked.
pub fn prepared(db: &Database) {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.register_type(Box::new(VectorType)).unwrap();
}

/// COUNTED registered in `db`, version 1: what it is asked.
pub fn registered(db: &Database) -> Arc<Asked> {
    registered_as(db, "COUNTED")
}

/// The plug-in of name `name` registered in `db`, version 1: what it is
/// asked.
pub fn registered_as(db: &Database, name: &'static str) -> Arc<Asked> {
    let asked = Arc::new(Asked { version: AtomicU32::new(1), ..Asked::default() });
    db.register_embedder(Arc::new(Counted { name, asked: Arc::clone(&asked) })).unwrap();
    asked
}
