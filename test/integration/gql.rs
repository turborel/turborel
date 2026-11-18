//! \[EXTENSION\] GQL (graph standard §9, RULES.md D-GQL): EXECUTE GQL,
//! GQL_TABLE and a GQL session over a graph over tables, Mode A (§4.1),
//! and over the GQL graphs of Mode C (§4.3), typed and open. The modules
//! below hold the queries' clauses, their translations checked against
//! SQL written by hand, Mode C's statements, and the data-modifying
//! statements.

use turborel::db::{Database, Direct, Session};

mod dml;
mod modes;
mod queries;
mod translation;

/// The bank of the standard's example (§4.1) with its rows: accounts of
/// Ann, Bob, Cy and Dee, transfers Ann to Bob to Cy to Ann and Cy to Dee,
/// two cities, and where three accounts are.
const BANK: &[&str] = &[
    "CREATE SCHEMA AUTHORIZATION A \
     CREATE TABLE CITY (CITY_ID INTEGER PRIMARY KEY, CITY_NAME VARCHAR(20)) \
     CREATE TABLE ACCOUNT (ACCT_ID INTEGER PRIMARY KEY, OWNER VARCHAR(20), IS_BLOCKED BOOLEAN) \
     CREATE TABLE TRANSFER (TXN_ID INTEGER PRIMARY KEY, FROM_ACCT INTEGER REFERENCES ACCOUNT, \
         TO_ACCT INTEGER REFERENCES ACCOUNT, AMOUNT DECIMAL(10, 2)) \
     CREATE TABLE LOCATED_IN (ACCT_ID INTEGER PRIMARY KEY REFERENCES ACCOUNT, CITY_ID INTEGER REFERENCES CITY)",
    "INSERT INTO A.CITY VALUES (1, 'Oslo'), (2, 'Rome')",
    "INSERT INTO A.ACCOUNT VALUES (1, 'Ann', FALSE), (2, 'Bob', FALSE), (3, 'Cy', TRUE), (4, 'Dee', FALSE)",
    "INSERT INTO A.TRANSFER VALUES (10, 1, 2, 100), (11, 2, 3, 50), (12, 3, 1, 25), (13, 3, 4, 75)",
    "INSERT INTO A.LOCATED_IN VALUES (1, 1), (2, 2), (3, 1)",
];

/// The graph BANK over the bank's tables.
const BANK_GRAPH: &str = "CREATE PROPERTY GRAPH BANK \
     VERTEX TABLES ( \
       ACCOUNT LABEL ACCOUNT PROPERTIES (ACCT_ID, OWNER, IS_BLOCKED), \
       CITY KEY (CITY_ID) LABEL CITY LABEL PLACE PROPERTIES (CITY_NAME)) \
     EDGE TABLES ( \
       TRANSFER KEY (TXN_ID) \
         SOURCE KEY (FROM_ACCT) REFERENCES ACCOUNT (ACCT_ID) \
         DESTINATION KEY (TO_ACCT) REFERENCES ACCOUNT (ACCT_ID) \
         LABEL TRANSFER PROPERTIES (TXN_ID, AMOUNT), \
       LOCATED_IN SOURCE ACCOUNT DESTINATION CITY LABEL IS_LOCATED_IN NO PROPERTIES)";

/// A session of A over the bank and its graph.
fn bank() -> (Database, Session) {
    let db = Database::new();
    let s = db.session("A");
    db.schema(BANK[0]).unwrap();
    for sql in &BANK[1..] {
        s.run(sql).unwrap();
    }
    s.run(BANK_GRAPH).unwrap();
    s.commit().unwrap();
    (db, s)
}

/// The rows a direct statement gives, each its values joined by commas.
fn direct(s: &Session, sql: &str) -> Vec<String> {
    match s.direct(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}")) {
        Direct::Rows(mut c) => {
            let mut out = Vec::new();
            while let Some(row) = c.fetch().unwrap() {
                out.push(row.iter().map(|v| v.to_string().trim().to_owned()).collect::<Vec<_>>().join(","));
            }
            out
        }
        Direct::Done(_) => Vec::new(),
    }
}

/// The rows of GQL `gql` on graph BANK.
fn gql(s: &Session, text: &str) -> Vec<String> {
    direct(s, &format!("EXECUTE GQL ON GRAPH BANK '{}'", text.replace('\'', "''")))
}

/// The SQLCODE and message `sql` fails with, run directly.
fn failed(s: &Session, sql: &str) -> (i32, String) {
    let e = s.direct(sql).map(|_| ()).err().unwrap_or_else(|| panic!("{sql} succeeded"));
    (e.sqlcode(), e.msg)
}
