//! Integer predicates and an index of their candidates, for D34's tests.
//! The method scans its held keys so the tests judge the contract without
//! adding another production index structure.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use turborel::db::{Database, Session};
use turborel::error::{Code, Result, SqlError};
use turborel::functions::ScalarFunction;
use turborel::index::{BTreeMethod, Entries, Index, IndexMethod, Key, KeyOrder, Predicate, Probe};
use turborel::params::{ParameterType, Params};
use turborel::storage::RowId;
use turborel::threads::JobPolicy;
use turborel::value::{CompOp, DataType, Spelled, Value};

pub const FUNCTION: &str = "NEXT_TO";
pub const METHOD: &str = "candidates";
pub const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

#[derive(Clone)]
pub struct Function {
    pub safe: bool,
    pub deterministic: bool,
    pub error_at: Option<i128>,
    pub null_true: bool,
}

impl Default for Function {
    fn default() -> Function {
        Function { safe: true, deterministic: true, error_at: None, null_true: false }
    }
}

impl ScalarFunction for Function {
    fn name(&self) -> &str {
        FUNCTION
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let integer = |t: &DataType| matches!(t, DataType::Exact { scale: 0, .. });
        if args.len() != 2 || !args.iter().all(integer) {
            return Err(SqlError::new(Code::SyntaxRule, "[EXTENSION] NEXT_TO takes two integers"));
        }
        Ok(DataType::Boolean)
    }
    fn parameter_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        Some(ParameterType::Of(INTEGER))
    }
    fn index_predicate(&self) -> bool {
        self.safe
    }
    fn deterministic(&self) -> bool {
        self.deterministic
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        if args.iter().any(Value::is_null) {
            return Ok(if self.null_true { Value::Bool(true) } else { Value::Null });
        }
        let [Value::Exact(a), Value::Exact(b)] = args else {
            return Err(SqlError::new(Code::Api, "[DESIGN] NEXT_TO was given no pair of integers"));
        };
        if self.error_at == Some(a.units) {
            return Err(SqlError::new(Code::Data, "[EXTENSION] NEXT_TO refuses this integer"));
        }
        Ok(Value::Bool(a.units.checked_sub(b.units) == Some(1)))
    }
}

#[derive(Clone)]
pub struct Method {
    pub lossy: bool,
    pub estimate: Option<usize>,
    pub pages: Option<f64>,
    pub argument: Option<usize>,
    pub seen: Arc<Mutex<Vec<(usize, usize, Value)>>>,
    pub priced: Arc<AtomicUsize>,
}

impl Default for Method {
    fn default() -> Method {
        Method { lossy: false, estimate: Some(2), pages: Some(1.0), argument: None, seen: Arc::default(), priced: Arc::default() }
    }
}

impl IndexMethod for Method {
    fn name(&self) -> &'static str {
        METHOD
    }
    fn supports(&self, _op: CompOp) -> bool {
        false
    }
    fn ordered(&self) -> bool {
        true
    }
    fn restores_keys(&self) -> bool {
        true
    }
    fn supports_predicate(&self, predicate: &Predicate, columns: &[DataType]) -> bool {
        let accepted = self.argument.is_none_or(|arg| arg == predicate.argument);
        let integer = columns.get(predicate.column).is_some_and(|t| matches!(t, DataType::Exact { scale: 0, .. }));
        predicate.function.0.name() == FUNCTION && accepted && integer
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        Ok(Box::new(Candidates { inner: Arc::from(BTreeMethod::in_memory().create(columns)?), method: self.clone() }))
    }
}

struct Candidates {
    inner: Arc<dyn Index>,
    method: Method,
}

impl Index for Candidates {
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
        self.method.priced.fetch_add(1, Ordering::Relaxed);
        self.method.estimate
    }
    fn probe_pages(&self, _probe: &Probe) -> Option<f64> {
        self.method.pages
    }
    fn search(self: Arc<Self>, probe: &Probe, order: KeyOrder) -> Entries {
        let Probe::Predicate { predicate, value } = probe else {
            return Arc::clone(&self.inner).search(probe, order);
        };
        self.method.seen.lock().unwrap().push((predicate.column, predicate.argument, value.clone()));
        let (predicate, value, lossy) = (predicate.clone(), value.clone(), self.method.lossy);
        let entries = Arc::clone(&self.inner).search(&Probe::All, order);
        Box::new(entries.filter(move |entry| match entry {
            Ok((key, _)) => candidate(&predicate, &value, key, lossy),
            Err(_) => true,
        }))
    }
}

fn candidate(predicate: &Predicate, value: &Value, key: &Key, lossy: bool) -> bool {
    let (Some(Value::Exact(column)), Value::Exact(constant)) = (key.0.get(predicate.column), value) else { return false };
    let difference =
        if predicate.argument == 0 { column.units.checked_sub(constant.units) } else { constant.units.checked_sub(column.units) };
    difference == Some(1) || lossy && difference == Some(2)
}

pub fn database(method: Method, function: Function) -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.register_scalar_function(Box::new(function)).unwrap();
    db.register_index_method(Box::new(method)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER, N INTEGER NOT NULL, V INTEGER)").unwrap();
    let s = session(&db);
    for start in (0..200).step_by(50) {
        let values: Vec<_> = (start..start + 50).map(|k| format!("({k}, {k}, {k})")).collect();
        s.run(&format!("INSERT INTO T VALUES {}", values.join(","))).unwrap();
    }
    s.run("INSERT INTO T VALUES (NULL, 200, NULL)").unwrap();
    s.commit().unwrap();
    db
}

pub fn session(db: &Database) -> Session {
    let s = db.session("S");
    s.run("SET SQL.FUNCTION_CALLS = 'ON'").unwrap();
    s
}

pub fn indexed(db: &Database, columns: &[&str]) {
    db.create_index("T_P", "S.T", columns, METHOD).unwrap();
}

pub fn plan(s: &Session, sql: &str) -> String {
    s.physical_plan(sql, &Params::new()).unwrap()
}
