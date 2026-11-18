//! A database of VECTOR columns for the vector index methods: the VECTOR
//! type and functions registered, each distance function of one metric
//! declaring it (`vector_metric`), so FETCH APPROX of one is served by an
//! index of its metric. A table S.T (ID INTEGER, V VECTOR(16), a FLOAT32,
//! C INTEGER) of clustered random vectors, with the exact answers to
//! compare an index's with.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Result;
use turborel::functions::{ScalarFunction, ScalarRef};
use turborel::memory::MemoryBudget;
use turborel::params::ParameterType;
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{DataType, Value};
use turborel::vector::{VectorType, functions};

use crate::common::{Scratch, after_crash, rows};

/// The dimensions of T's vectors.
pub const DIMS: usize = 16;
/// The rows asked of a nearest query.
pub const K: usize = 10;
/// The query vectors a recall is measured over.
pub const QUERIES: usize = 20;
/// The methods tested, each of its own metric option group.
pub const METHODS: [&str; 3] = ["diskann", "hnsw", "ivf"];

/// A built-in VECTOR function, declaring the metric of a distance
/// shorthand as the vector design document's §13 R13.1 asks.
struct Declared {
    inner: Arc<dyn ScalarFunction>,
    metric: Option<&'static str>,
}

impl ScalarFunction for Declared {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        self.inner.return_type(args)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        self.inner.call(args)
    }
    fn deterministic(&self) -> bool {
        self.inner.deterministic()
    }
    fn specialize(&self, args: &[Option<&Value>], budget: &Arc<MemoryBudget>) -> Option<ScalarRef> {
        self.inner.specialize(args, budget)
    }
    fn argument_type(&self, position: usize, args: &[Option<DataType>]) -> Option<DataType> {
        self.inner.argument_type(position, args)
    }
    fn parameter_types(&self) -> Option<Vec<DataType>> {
        self.inner.parameter_types()
    }
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        self.inner.parameter_type(position, args)
    }
    fn vector_metric(&self, constants: &[Option<&Value>]) -> Option<(String, usize)> {
        let metric = self.metric?;
        (constants.len() == 2).then(|| (metric.to_owned(), 0))
    }
}

/// The metric each distance shorthand measures by.
fn metric_of(name: &str) -> Option<&'static str> {
    match name {
        "L2_DISTANCE" => Some("EUCLIDEAN"),
        "COSINE_DISTANCE" => Some("COSINE"),
        "L1_DISTANCE" => Some("MANHATTAN"),
        _ => None,
    }
}

/// Registers the VECTOR type and functions of `db`.
pub fn register(db: &Database) {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.register_type(Box::new(VectorType)).unwrap();
    for f in functions() {
        let metric = metric_of(f.name());
        db.register_scalar_function(Box::new(Declared { inner: f, metric })).unwrap();
    }
}

/// A random-looking number of `seed` in [0, 1), the same each run.
pub fn unit(seed: u64) -> f64 {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0x2545_F491_4F6C_DD1D);
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    f64::from(u32::try_from(x >> 32).unwrap()) / 4_294_967_296.0
}

/// The vector of row `id`: near one of 16 centres, as embeddings cluster.
pub fn vector_of(id: u64) -> Vec<f64> {
    let centre = id % 16;
    (0..DIMS as u64).map(|d| (unit(centre * 131 + d) * 10.0 + unit(id * 977 + d) * 2.0 * 100.0).round() / 100.0).collect()
}

/// Query vector `n`, off the rows' own.
pub fn query_of(n: u64) -> Vec<f64> {
    vector_of(n * 7919 + 1_000_003)
}

/// `v` as a SQL literal of T's column.
pub fn literal(v: &[f64]) -> String {
    let parts: Vec<String> = v.iter().map(|x| format!("{x}")).collect();
    format!("TO_VECTOR('[{}]', {DIMS}, 'FLOAT32')", parts.join(","))
}

/// Inserts rows `from..to` into T, committed.
pub fn insert(s: &Session, (from, to): (u64, u64)) {
    for start in (from..to).step_by(250) {
        let values: Vec<String> = (start..to.min(start + 250)).map(|i| format!("({i}, {}, {})", literal(&vector_of(i)), i % 10)).collect();
        s.run(&format!("INSERT INTO T VALUES {}", values.join(","))).unwrap();
    }
    s.commit().unwrap();
}

/// Makes S.T in `db`, of `n` rows and one null vector, row -1.
pub fn populate(db: &Database, n: u64) {
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (ID INTEGER, V VECTOR({DIMS}), C INTEGER)")).unwrap();
    let s = db.session("S");
    insert(&s, (0, n));
    s.run("INSERT INTO T VALUES (-1, NULL, 0)").unwrap();
    s.commit().unwrap();
}

/// A database in memory of `n` rows.
pub fn database(n: u64) -> Database {
    let db = Database::new();
    register(&db);
    populate(&db, n);
    db
}

/// A database of files under `scratch`, of no tables.
pub fn on_files(scratch: &Scratch) -> Database {
    let db = Database::new();
    register(&db);
    db.open_files(&after_crash(), &scratch.path("db"), &OpenOptions::default()).unwrap();
    db
}

/// The `K` rows nearest query `n` by `function`, as `fetch` asks (APPROX,
/// EXACT), of the rows `filter` keeps.
pub fn nearest_sql(function: &str, n: u64, (fetch, filter): (&str, &str)) -> String {
    format!("SELECT ID FROM T {filter} ORDER BY {function}(V, {}) FETCH {fetch} FIRST {K} ROWS ONLY", literal(&query_of(n)))
}

/// The share of the exact `K` nearest rows the approximate search finds,
/// over `QUERIES` queries, of `function`'s metric.
pub fn recall(s: &Session, function: &str, filter: &str) -> f64 {
    let mut found = 0_usize;
    let mut asked = 0_usize;
    for n in 0..QUERIES as u64 {
        let exact = rows(s, &nearest_sql(function, n, ("EXACT", filter))).unwrap();
        let approx = rows(s, &nearest_sql(function, n, ("APPROX", filter))).unwrap();
        asked += exact.len();
        found += approx.iter().filter(|r| exact.contains(r)).count();
    }
    f64::from(u32::try_from(found).unwrap()) / f64::from(u32::try_from(asked.max(1)).unwrap())
}

pub fn plan(s: &Session, sql: &str) -> String {
    s.physical_plan(sql, &Params::new()).unwrap()
}
