//! The examples of CREATE VECTOR INDEX, run as printed, of stand-ins for
//! the vector methods: each keeps its keys in a B-link tree and declares
//! the options its engine does, so the statement's forms are shown as
//! they are desugared, apart from the engines.

use turborel::db::Database;
use turborel::error::Result;
use turborel::index::{BTreeMethod, Index, IndexMethod};
use turborel::settings::Kind;
use turborel::storage::{Rebuild, TableOption};
use turborel::value::{CompOp, DataType, Decimal, Value};

use super::Client;

/// A stand-in for a vector method.
struct Standing(&'static str, &'static [&'static str]);

/// Each stand-in, and the options of its group it declares.
const METHODS: [Standing; 3] = [
    Standing("hnsw", &["M", "EF_CONSTRUCTION", "TARGET_ACCURACY", "METRIC", "QUANTIZER"]),
    Standing("diskann", &["MAX_DEGREE", "BUILD_LIST", "TARGET_ACCURACY", "METRIC", "QUANTIZER", "PQ_BYTES"]),
    Standing("ivf", &["LISTS", "SAMPLE_PER_LIST", "METRIC", "QUANTIZER"]),
];

/// The options whose values are words.
const WORDS: [&str; 2] = ["METRIC", "QUANTIZER"];

impl IndexMethod for Standing {
    fn name(&self) -> &'static str {
        self.0
    }
    fn supports(&self, _op: CompOp) -> bool {
        false
    }
    fn index_options(&self) -> Vec<TableOption> {
        let group = self.0.to_ascii_uppercase();
        let option = |n: &&str| {
            let name = format!("{group}.{n}");
            let (kind, default) = if WORDS.contains(n) {
                (Kind::Identifier, Value::Char("NONE".into()))
            } else {
                (Kind::Integer { min: 0, max: 1_000_000 }, Value::Exact(Decimal::new(0, 0)))
            };
            TableOption::new((&name, kind, default), Rebuild::Yes, "An option the engine declares.")
        };
        self.1.iter().map(option).collect()
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        BTreeMethod::in_memory().create(columns)
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D-APPROX: the examples of CREATE
/// VECTOR INDEX run as printed, as ANN, of a table DOCS and stand-ins for
/// the methods `hnsw`, `diskann` and `ivf`.
#[test]
fn the_vector_index_examples_run_as_printed() {
    let db = Database::new();
    for method in METHODS {
        db.register_index_method(Box::new(method)).unwrap();
    }
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE DOCS (ID INTEGER, EMBEDDING DOUBLE PRECISION)").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/statements/create-vector-index.md") > 0, "it shows its examples' output");
}
