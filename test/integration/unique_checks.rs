//! UNIQUE checked after a statement [X3.135-1986 §4.5, §6.4 GR2, §8.12]:
//! the checks an UPDATE makes, counted through an index method.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use turborel::db::Database;
use turborel::error::{self, Code};
use turborel::index::{BTreeMethod, Index, IndexMethod, Key, Probe};
use turborel::storage::RowId;
use turborel::value::{CompOp, DataType};

use crate::common::{exec, rows};

/// The built-in ordered index method, counting the UNIQUE checks made
/// through its indexes (`Index::rows`).
struct Counting {
    inner: BTreeMethod,
    checks: Arc<AtomicUsize>,
}

/// An index of `Counting`.
struct CountingIndex {
    inner: Box<dyn Index>,
    checks: Arc<AtomicUsize>,
}

impl IndexMethod for Counting {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn supports(&self, op: CompOp) -> bool {
        self.inner.supports(op)
    }
    fn ordered(&self) -> bool {
        self.inner.ordered()
    }
    fn create(&self, columns: &[DataType]) -> error::Result<Box<dyn Index>> {
        Ok(Box::new(CountingIndex { inner: self.inner.create(columns)?, checks: Arc::clone(&self.checks) }))
    }
}

impl Index for CountingIndex {
    fn insert(&self, key: Key, row: RowId) -> error::Result<()> {
        self.inner.insert(key, row)
    }
    fn insert_alone(&self, key: Key, row: RowId) -> error::Result<bool> {
        self.inner.insert_alone(key, row)
    }
    fn remove(&self, key: &Key, row: RowId) -> error::Result<()> {
        self.inner.remove(key, row)
    }
    fn rows(&self, key: &Key) -> error::Result<Vec<RowId>> {
        self.checks.fetch_add(1, Ordering::Relaxed);
        self.inner.rows(key)
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        self.inner.estimate(probe)
    }
    fn search(&self, probe: &Probe) -> error::Result<Vec<(Key, RowId)>> {
        self.inner.search(probe)
    }
}

/// Verifies: [X3.135-1986 §4.5, §6.4 GR2, §8.12] UPDATE enforces
/// UNIQUE after the statement: keys may pass through duplicates (SET K =
/// K + 1) but not end with them, a constraint of two columns checked when
/// either is assigned; [DESIGN] an UPDATE that assigns no column of a
/// constraint does not check it, as it leaves every key as it was, nor a
/// key whose entry was its only one as it was inserted
/// (`Index::insert_alone`).
#[test]
fn update_checks_the_unique_constraints_it_could_break_8_12() {
    let checks = Arc::new(AtomicUsize::new(0));
    let db = Database::new();
    db.register_index_method(Box::new(Counting { inner: BTreeMethod::in_memory(), checks: Arc::clone(&checks) })).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION APP
           CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER)
           CREATE TABLE U (A INTEGER NOT NULL, B INTEGER NOT NULL, V INTEGER, UNIQUE (A, B))",
    )
    .unwrap();
    let s = db.session("APP");
    for k in 1..=3 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 0)")).unwrap();
        s.run(&format!("INSERT INTO U VALUES ({k}, {k}, 0)")).unwrap();
    }
    let made = |sql: &str| {
        let before = checks.load(Ordering::Relaxed);
        let outcome = exec(&s, sql).map(|_| ());
        (outcome, checks.load(Ordering::Relaxed) - before)
    };
    assert_eq!(made("UPDATE T SET V = V + 1"), (Ok(()), 0), "no key assigned");
    assert_eq!(made("UPDATE U SET V = 7 WHERE A = 2"), (Ok(()), 0));
    assert_eq!(made("UPDATE T SET K = K + 1").0, Ok(()), "through duplicates, ending with none");
    // 2, 3 and 4 to 3, 4 and 5: the entries of 3 and 4 not their keys'
    // only ones, the old versions' there; 5's is, and needs no check.
    assert!(made("UPDATE T SET K = K + 1").1 >= 2, "each key of another row's entry checked");
    assert_eq!(made("UPDATE T SET K = 9").0, Err(Code::Constraint));
    let (outcome, made_checks) = made("UPDATE U SET A = 1 WHERE B = 2");
    assert_eq!((outcome, made_checks), (Ok(()), 0), "(2, 2) to (1, 2): one column assigned, its entry its key's only one");
    assert_eq!(made("UPDATE U SET B = 1 WHERE A = 1 AND B = 2").0, Err(Code::Constraint), "(1, 2) to (1, 1), a duplicate");
    assert_eq!(rows(&s, "SELECT K FROM T ORDER BY K"), Ok(vec!["3".into(), "4".into(), "5".into()]));
}
