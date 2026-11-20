//! \[DESIGN\] The defaults of a column that are value functions
//! [ISO/IEC 9075:1992 §11.5, §6.8], each kept in the catalog as written
//! and given again after the database is reopened.

use turborel::db::{Database, OpenOptions, Opened};

use crate::common::{Scratch, after_crash, rows};

/// Verifies: [ISO/IEC 9075:1992 §11.5 GR2, §6.8] [DESIGN] a column's
/// default of SESSION_USER, SYSTEM_USER, CURRENT_ROLE, CURRENT_DATE,
/// CURRENT_TIME or CURRENT_TIMESTAMP is kept across a reopen and given by
/// DEFAULT VALUES after it.
#[test]
fn value_function_defaults_survive_a_reopen_11_5() {
    let scratch = Scratch::new("defaults-functions-kept");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        let opened = db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        (db, opened)
    };
    let (db, _) = open();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER DEFAULT 1, U VARCHAR(128) DEFAULT SESSION_USER, \
         Y VARCHAR(128) DEFAULT SYSTEM_USER, R VARCHAR(128) DEFAULT CURRENT_ROLE, D DATE DEFAULT CURRENT_DATE, \
         M TIME DEFAULT CURRENT_TIME, P TIMESTAMP DEFAULT CURRENT_TIMESTAMP)",
    )
    .unwrap();
    db.close().unwrap();
    drop(db);
    let (db, opened) = open();
    assert_eq!(opened, Opened::Reopened);
    let s = db.session("S");
    s.run("INSERT INTO T DEFAULT VALUES").unwrap();
    let got = rows(&s, "SELECT K, U, Y IS NOT NULL, R IS NULL, D IS NOT NULL, M IS NOT NULL, P IS NOT NULL FROM T").unwrap();
    assert_eq!(got, ["1,S,TRUE,TRUE,TRUE,TRUE,TRUE"]);
}
