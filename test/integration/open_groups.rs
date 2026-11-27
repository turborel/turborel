//! \[DESIGN\] A key of a row of another transaction's change still open
//! is pending to a UNIQUE check (RULES.md D1236): of a version in the tree
//! as of a row a load wrote straight into a frozen group, which may yet
//! be rolled back.

use turborel::db::Database;
use turborel::error::Code;
use turborel::params::Load;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::count;

/// T of the column store, a load into it written straight as groups.
const GROUPED: &str = "CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER PRIMARY KEY, A INTEGER) \
                       USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 4096, COLUMNSTORE.LOAD = 'GROUPS')";

/// A database of `schema`, its background work done only when asked.
fn database(schema: &str) -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(schema).unwrap();
    db
}

/// `n` as an exact numeric value.
fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// Verifies: [DESIGN] RULES.md D1236, D533: a row inserted by one
/// transaction under the key of a row another's load wrote into a frozen
/// group, still open, fails as a key another transaction is changing
/// fails it, SQLCODE -911, not as a duplicate of a row that stands; once
/// the load is rolled back, it goes in.
#[test]
fn a_key_of_an_open_loads_group_is_pending_to_another_transaction() {
    let db = database(GROUPED);
    let a = db.session("S");
    let p = a.prepare("INSERT INTO T (K, A) VALUES (?, ?)").unwrap();
    let rows: Vec<Vec<Value>> = (0..4096).map(|k| vec![int(k), int(k % 7)]).collect();
    a.execute_prepared_rows(&p, &rows, Load::Alone).unwrap();
    let b = db.session("S");
    assert_eq!(b.run("INSERT INTO T VALUES (5, 1)").map_err(|e| e.code).unwrap_err(), Code::Conflict);
    b.rollback().unwrap();
    a.rollback().unwrap();
    b.run("INSERT INTO T VALUES (5, 1)").unwrap();
    b.commit().unwrap();
    assert_eq!(count(&b, "SELECT COUNT(*) FROM T"), 1);
}
