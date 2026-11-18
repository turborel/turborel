//! A hybrid index that embeds its text, its statement stopped by a crash
//! between its steps: the database opened again keeps no half of it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use turborel::catalog::ModelOption;
use turborel::db::{Database, OpenOptions};
use turborel::error::{Code, Result, SqlError};
use turborel::threads::JobPolicy;
use turborel::vector::embed::{Embedder, ModelShape};
use turborel::vector::{Element, Layout, Vector};

use super::fixture::read;
use crate::common::{Scratch, after_crash};

/// A model, GATED, of 4 dense dimensions, that once shut stops each call
/// until opened, then fails it.
#[derive(Default)]
struct Gate {
    shut: AtomicBool,
    reached: AtomicBool,
    opened: AtomicBool,
}

/// The plug-in.
struct Gated(Arc<Gate>);

impl Embedder for Gated {
    fn name(&self) -> &'static str {
        "GATED"
    }
    fn shape(&self) -> ModelShape {
        ModelShape { dims: 4, element: Element::Float32, layout: Layout::Dense }
    }
    fn version(&self) -> u32 {
        1
    }
    fn batch(&self) -> usize {
        8
    }
    fn configured(&self, _options: &[ModelOption]) -> Result<Option<Arc<dyn Embedder>>> {
        Ok(None)
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vector>> {
        if self.0.shut.load(Ordering::SeqCst) {
            self.0.reached.store(true, Ordering::SeqCst);
            while !self.0.opened.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
            return Err(SqlError::new(Code::Data, "the model was stopped"));
        }
        texts.iter().map(|t| Vector::dense(Element::Float32, vec![f64::from(u32::try_from(t.len()).unwrap_or(0)), 1.0, 0.0, 0.0])).collect()
    }
}

/// The database in `dir`, opened as after a crash, GATED registered by
/// `gate`, its background work run only when asked.
fn opened(dir: &std::path::Path, gate: &Arc<Gate>) -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.register_embedder(Arc::new(Gated(Arc::clone(gate)))).unwrap();
    db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    db
}

/// What a half made index would leave: tables, indexes and pipelines
/// named for DOCS_H.
const LEFT: &str = "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME LIKE 'DOCS_H%' \
                    UNION SELECT INDEX_NAME FROM INFORMATION_SCHEMA.INDEXES WHERE INDEX_NAME LIKE 'DOCS_H%' \
                    UNION SELECT PIPELINE_NAME FROM INFORMATION_SCHEMA.VECTOR_PIPELINES";

/// Verifies: [EXTENSION] vector_standard §11.1, RULES.md D100: a crash
/// while CREATE HYBRID VECTOR INDEX of a MODEL embeds the rows there,
/// after it made the index, the table of chunks and their vector index,
/// leaves none of them once the database opens again; the index is then
/// made whole, and DROP INDEX leaves none either.
#[test]
fn a_crash_between_the_steps_leaves_no_half_index_11_1() {
    let scratch = Scratch::new("hybrid-crash");
    let dir = scratch.path("db");
    let gate = Arc::new(Gate::default());
    let db = opened(&dir, &gate);
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE DOCS (ID INTEGER PRIMARY KEY, BODY VARCHAR(100))").unwrap();
    let s = db.session("S");
    s.run("CREATE EMBEDDING MODEL G WITH (TYPE = 'GATED')").unwrap();
    s.run("INSERT INTO DOCS VALUES (1, 'refund policy. shipping'), (2, 'opening hours')").unwrap();
    s.commit().unwrap();
    gate.shut.store(true, Ordering::SeqCst);
    let making = {
        let db = db.clone();
        thread::spawn(move || db.session("S").run("CREATE HYBRID VECTOR INDEX DOCS_H ON DOCS (BODY) PARAMETERS (MODEL G)").map(drop))
    };
    let began = Instant::now();
    while !gate.reached.load(Ordering::SeqCst) {
        assert!(began.elapsed() < Duration::from_secs(120), "the statement reached the model");
        thread::sleep(Duration::from_millis(5));
    }
    let shown = read(&s, LEFT);
    assert!(shown.iter().any(|n| n == "DOCS_H") && shown.iter().any(|n| n == "DOCS_H_CHUNKS"), "half made: {shown:?}");
    let after = opened(&dir, &Arc::new(Gate::default()));
    let t = after.session("S");
    assert_eq!(read(&t, LEFT), Vec::<String>::new(), "nothing half made");
    t.commit().unwrap();
    t.run("CREATE HYBRID VECTOR INDEX DOCS_H ON DOCS (BODY) PARAMETERS (MODEL G)").unwrap();
    assert_eq!(read(&t, "SELECT COUNT(*) FROM DOCS_H_CHUNKS"), ["2"]);
    t.commit().unwrap();
    t.run("DROP INDEX DOCS_H").unwrap();
    assert_eq!(read(&t, LEFT), Vec::<String>::new());
    gate.opened.store(true, Ordering::SeqCst);
    assert!(making.join().unwrap().is_err(), "the stopped statement fails");
    drop((t, s, after, db));
}
