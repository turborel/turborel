//! A fake approximate index method and a distance function of one metric,
//! so the planner and executor of approximate search (RULES.md D94)
//! are tested apart from the vector engines. TEST_DIST(x, q [, metric])
//! is |x - q| of DOUBLE PRECISION values, of the metric TESTL1 unless a
//! third argument names another. The index "fakeann" keeps its keys in a
//! B-link tree and answers a search with the `L` keys nearest the query,
//! L the search list asked or 4, at least the rows asked, times the
//! widening; of an in-filter, only keys whose row the filter admits. It
//! records every request, and can be told to decline.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use turborel::catalog::IndexDesc;
use turborel::db::{Database, Session};
use turborel::error::{Code, Result, SqlError};
use turborel::functions::ScalarFunction;
use turborel::index::{
    Approximation, BTreeMethod, Entries, FilterHint, Index, IndexMethod, Key, KeyOrder, NearestQuery, NearestSupport, NeighbourEntries,
    Probe, RowFilter,
};
use turborel::memory::MemoryBudget;
use turborel::params::{ParameterType, Params};
use turborel::settings::Kind;
use turborel::storage::{Rebuild, RowId, TableOption};
use turborel::threads::JobPolicy;
use turborel::value::{CompOp, DOUBLE_PRECISION, DataType, Spelled, Value};

pub const FUNCTION: &str = "TEST_DIST";
pub const METRIC: &str = "TESTL1";
pub const METHOD: &str = "fakeann";
/// The search list of a request that names none.
pub const DEFAULT_LIST: u32 = 4;

pub fn double() -> DataType {
    DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision }
}

/// TEST_DIST(x, q [, metric]), under the name it holds.
pub struct Distance(pub &'static str);

impl ScalarFunction for Distance {
    fn name(&self) -> &str {
        self.0
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let numbers = args.iter().take(2).all(DataType::is_numeric);
        if !(2..=3).contains(&args.len()) || !numbers {
            return Err(SqlError::new(Code::SyntaxRule, "[EXTENSION] TEST_DIST takes two numbers and a metric"));
        }
        Ok(double())
    }
    fn rechecked_errors(&self) -> bool {
        true
    }
    fn parameter_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        Some(ParameterType::Of(double()))
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let (Some(a), Some(b)) = (args.first(), args.get(1)) else { return Ok(Value::Null) };
        match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => Ok(Value::Approx((a - b).abs())),
            _ => Ok(Value::Null),
        }
    }
    fn vector_metric(&self, constants: &[Option<&Value>]) -> Option<(String, usize)> {
        match constants {
            [_, _] => Some((METRIC.to_owned(), 0)),
            [_, _, Some(Value::Char(metric))] => Some((metric.to_ascii_uppercase(), 0)),
            _ => None,
        }
    }
}

/// What the fake was asked, and how it answers.
#[derive(Clone, Default)]
pub struct Log {
    pub asked: Arc<Mutex<Vec<Approximation>>>,
    pub filtered: Arc<Mutex<usize>>,
    pub decline: Arc<AtomicBool>,
    pub calls: Arc<Mutex<Vec<&'static str>>>,
}

impl Log {
    pub fn asked(&self) -> Vec<Approximation> {
        std::mem::take(&mut *self.asked.lock().unwrap())
    }
    pub fn calls(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

/// The fake method, of a name and the options of its group.
#[derive(Clone)]
pub struct Method {
    pub name: &'static str,
    pub log: Log,
}

/// The numeric options every fake declares, under its group.
const NUMBERS: [&str; 15] = [
    "M",
    "EF_CONSTRUCTION",
    "EF_SEARCH",
    "TARGET_ACCURACY",
    "PQ_BYTES",
    "LISTS",
    "SAMPLE_PER_LIST",
    "PROBES",
    "MAX_DEGREE",
    "BUILD_LIST",
    "ALPHA",
    "BLOCK_SIZE",
    "SEARCH_LIST",
    "BEAM_WIDTH",
    "QUANTIZATION_BITS",
];
/// The word options every fake declares, under its group.
const WORDS: [&str; 4] = ["METRIC", "QUANTIZER", "RERANK", "QUANTIZATION"];

impl IndexMethod for Method {
    fn name(&self) -> &'static str {
        self.name
    }
    fn supports(&self, _op: CompOp) -> bool {
        false
    }
    fn supports_nearest(&self, query: &NearestQuery, columns: &[DataType]) -> NearestSupport {
        let one = matches!(columns, [t] if t.is_numeric());
        if one && query.metric.is_some() { NearestSupport::Approximate } else { NearestSupport::No }
    }
    fn metric(&self, index: &IndexDesc) -> Option<String> {
        let own = format!("{}.METRIC", self.name.to_ascii_uppercase());
        let set =
            index.options.iter().find(|(n, _)| *n == own).and_then(|(_, v)| if let Value::Char(m) = v { Some(m.clone()) } else { None });
        Some(set.unwrap_or_else(|| METRIC.into()).to_string())
    }
    fn index_options(&self) -> Vec<TableOption> {
        let group = self.name.to_ascii_uppercase();
        let number = |n: &str| {
            let name = format!("{group}.{n}");
            let (kind, default) = if n == "ALPHA" {
                (Kind::Number { min: 0.0, max: 1.0e9 }, Value::Approx(0.0))
            } else {
                (Kind::Integer { min: 0, max: 1_000_000_000 }, crate::common::number(0))
            };
            TableOption::new((&name, kind, default), Rebuild::No, "A number the fake keeps.")
        };
        let word = |n: &str| {
            let name = format!("{group}.{n}");
            let default = if n == "METRIC" { METRIC } else { "NONE" };
            TableOption::new((&name, Kind::Identifier, Value::Char(default.into())), Rebuild::No, "A word the fake keeps.")
        };
        NUMBERS.iter().map(|n| number(n)).chain(WORDS.iter().map(|n| word(n))).collect()
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        Ok(Box::new(Fake { inner: Arc::from(BTreeMethod::in_memory().create(columns)?), log: self.log.clone() }))
    }
}

struct Fake {
    inner: Arc<dyn Index>,
    log: Log,
}

impl Fake {
    /// Its keys by distance from `q`, then row.
    fn by_distance(&self, q: f64) -> Result<Vec<(f64, Key, RowId)>> {
        let mut all = Vec::new();
        for entry in Arc::clone(&self.inner).search(&Probe::All, KeyOrder::Ascending) {
            let (key, row) = entry?;
            let Some(x) = key.0.first().and_then(Value::as_f64) else { continue };
            all.push(((x - q).abs(), key, row));
        }
        all.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
        Ok(all)
    }
}

impl Index for Fake {
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.inner.insert(key, row)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.inner.remove(key, row)
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        self.inner.rows(key)
    }
    fn estimate(&self, _probe: &Probe) -> Option<usize> {
        None
    }
    fn search(self: Arc<Self>, probe: &Probe, order: KeyOrder) -> Entries {
        Arc::clone(&self.inner).search(probe, order)
    }
    fn maintain(&self) -> Result<bool> {
        self.log.calls.lock().unwrap().push("maintain");
        Ok(false)
    }
    fn reside(&self, resident: bool) -> Result<()> {
        self.log.calls.lock().unwrap().push(if resident { "load" } else { "unload" });
        Ok(())
    }
    fn approximate(
        self: Arc<Self>,
        query: &NearestQuery,
        _budget: &Arc<MemoryBudget>,
        mut filter: Option<&mut dyn RowFilter>,
    ) -> Result<Option<NeighbourEntries>> {
        let Some(asked) = query.approximate else { return Ok(None) };
        self.log.asked.lock().unwrap().push(asked);
        if self.log.decline.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let Some(q) = query.value.as_f64() else { return Ok(None) };
        let list = usize::try_from(asked.search_list.unwrap_or(DEFAULT_LIST)).unwrap().max(asked.first);
        let list = list * usize::try_from(asked.widen).unwrap();
        let mut found = Vec::new();
        for (d, key, row) in self.by_distance(q)? {
            if found.len() == list || asked.radius.is_some_and(|r| d > r) {
                break;
            }
            if let (Some(f), FilterHint::InFilter) = (filter.as_deref_mut(), asked.filter) {
                *self.log.filtered.lock().unwrap() += 1;
                if !f.admits(&key, row)? {
                    continue;
                }
            }
            found.push(Ok((d, key, row)));
        }
        Ok(Some(Box::new(found.into_iter())))
    }
}

/// A database with the fake method `fakeann` and TEST_DIST, and schema
/// S's table T (ID INTEGER, X DOUBLE PRECISION, C INTEGER) of `n` rows:
/// X is the row's number, C its number modulo 1000.
pub fn database(n: usize) -> (Database, Log) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.register_scalar_function(Box::new(Distance(FUNCTION))).unwrap();
    let log = Log::default();
    db.register_index_method(Box::new(Method { name: METHOD, log: log.clone() })).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (ID INTEGER, X DOUBLE PRECISION, C INTEGER)").unwrap();
    let s = session(&db);
    for start in (0..n).step_by(500) {
        let values: Vec<_> = (start..n.min(start + 500)).map(|i| format!("({i}, {i}.0E0, {})", i % 1000)).collect();
        s.run(&format!("INSERT INTO T VALUES {}", values.join(","))).unwrap();
    }
    s.commit().unwrap();
    (db, log)
}

/// `database` with the index T_X of `fakeann` on X.
pub fn indexed(n: usize) -> (Database, Log) {
    let (db, log) = database(n);
    db.create_index("T_X", "S.T", &["X"], METHOD).unwrap();
    log.asked();
    (db, log)
}

pub fn session(db: &Database) -> Session {
    let s = db.session("S");
    s.run("SET SQL.FUNCTION_CALLS = 'ON'").unwrap();
    s
}

pub fn plan(s: &Session, sql: &str) -> String {
    s.physical_plan(sql, &Params::new()).unwrap()
}
