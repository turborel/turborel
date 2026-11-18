//! A database of documents for the hybrid index and the inverted index of
//! sparse vectors: the VECTOR type and functions registered, VECTOR_DISTANCE
//! of a constant metric declaring it (`vector_metric`), so FETCH APPROX of
//! it is served by an index of its metric.

use std::sync::Arc;

use turborel::db::{Database, Session};
use turborel::error::Result;
use turborel::functions::{ScalarFunction, ScalarRef};
use turborel::memory::MemoryBudget;
use turborel::params::{ParameterType, Params};
use turborel::threads::JobPolicy;
use turborel::value::{DataType, Value};
use turborel::vector::{VectorType, functions};

use crate::common::rows;

/// The least ID of the filler rows.
pub const FILLER: u32 = 1000;
/// The filler rows.
pub const FILLERS: u32 = 2000;

/// VECTOR_DISTANCE, declaring the metric its third argument names when it
/// is a constant, as the vector design document's R13.1 asks.
struct Declared(Arc<dyn ScalarFunction>);

impl ScalarFunction for Declared {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        self.0.return_type(args)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        self.0.call(args)
    }
    fn deterministic(&self) -> bool {
        self.0.deterministic()
    }
    fn specialize(&self, args: &[Option<&Value>], budget: &Arc<MemoryBudget>) -> Option<ScalarRef> {
        self.0.specialize(args, budget)
    }
    fn argument_type(&self, position: usize, args: &[Option<DataType>]) -> Option<DataType> {
        self.0.argument_type(position, args)
    }
    fn parameter_types(&self) -> Option<Vec<DataType>> {
        self.0.parameter_types()
    }
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        self.0.parameter_type(position, args)
    }
    fn vector_metric(&self, constants: &[Option<&Value>]) -> Option<(String, usize)> {
        let [_, _, Some(Value::Char(metric))] = constants else { return None };
        (self.0.name() == "VECTOR_DISTANCE").then(|| (metric.trim().to_ascii_uppercase(), 0))
    }
}

/// Registers the VECTOR type and functions of `db`.
pub fn register(db: &Database) {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.register_type(Box::new(VectorType)).unwrap();
    for f in functions() {
        db.register_scalar_function(Box::new(Declared(f))).unwrap();
    }
}

/// The documents of `documents`: a text and its embedding, ID from 1.
pub const DOCUMENTS: [(&str, [f64; 4]); 5] = [
    ("refund policy for damaged items", [1.0, 0.0, 0.0, 0.0]),
    ("shipping times and delivery", [0.9, 0.1, 0.0, 0.0]),
    ("how to return a damaged parcel", [0.0, 1.0, 0.0, 0.0]),
    ("refund of a gift card", [0.1, 0.0, 1.0, 0.0]),
    ("store opening hours", [0.7, 0.7, 0.0, 0.0]),
];

/// A number of `seed` in [0, 1), the same each run.
fn unit(seed: u32) -> f64 {
    let x = seed.wrapping_mul(0x9E37_79B9) ^ 0x2545_F491;
    let x = (x ^ (x >> 15)).wrapping_mul(0x2C1B_3C6D);
    f64::from(x ^ (x >> 12)) / 4_294_967_296.0
}

/// The embedding of filler row `k`: each its own, of a negative first
/// dimension, so farther by COSINE from the first document's than any
/// document's.
fn filler_vector(k: u32) -> [f64; 4] {
    let at = |d: u32| (unit(k * 4 + d) * 100.0).round() / 100.0;
    [-0.1 - at(0), at(1) * 2.0 - 1.0, at(2) * 2.0 - 1.0, at(3) * 2.0 - 1.0]
}

/// `v` as a VECTOR(4) literal.
pub fn literal(v: &[f64]) -> String {
    let parts: Vec<String> = v.iter().map(|x| format!("{x}")).collect();
    format!("TO_VECTOR('[{}]', 4, 'FLOAT32')", parts.join(","))
}

/// Schema S's table DOCS (ID INTEGER PRIMARY KEY, BODY VARCHAR(400), EMB
/// VECTOR(4)): `DOCUMENTS`, then `FILLERS` rows from ID `FILLER` of words
/// no test looks for and embeddings far from every document's, so an
/// index read costs less than a scan; and `index`, a statement making an
/// index of it, if any. Its session takes calls of registered functions.
pub fn documents(index: Option<&str>) -> (Database, Session) {
    let db = Database::new();
    register(&db);
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE DOCS (ID INTEGER PRIMARY KEY, BODY VARCHAR(400), EMB VECTOR(4))").unwrap();
    let s = db.session("S");
    s.run("SET SQL.FUNCTION_CALLS = 'ON'").unwrap();
    for (n, (text, v)) in DOCUMENTS.iter().enumerate() {
        s.run(&format!("INSERT INTO DOCS VALUES ({}, '{text}', {})", n + 1, literal(v))).unwrap();
    }
    let filler: Vec<String> =
        (FILLER..FILLER + FILLERS).map(|k| format!("({k}, 'filler row number {k}', {})", literal(&filler_vector(k)))).collect();
    for chunk in filler.chunks(200) {
        s.run(&format!("INSERT INTO DOCS VALUES {}", chunk.join(", "))).unwrap();
    }
    s.commit().unwrap();
    if let Some(index) = index {
        s.run(index).unwrap();
    }
    (db, s)
}

/// The plan of `sql`.
pub fn plan(s: &Session, sql: &str) -> String {
    s.physical_plan(sql, &Params::new()).unwrap()
}

/// The rows of `sql`, which must succeed.
pub fn read(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|code| panic!("{sql}: {code:?}"))
}
