//! GRAPH_TABLE over property graphs [GS §8] (RULES.md D108):
//! the conformance tests T1 to T12 of the standard (§23.2) on its bank
//! fixture (§23.1), with the rows each gives, and the modules below of
//! what else a GRAPH_TABLE does: its functions and predicates, rows
//! clauses, limits, lateral and time-travel reads, adjacency indexes and
//! the plans EXPLAIN shows.

use turborel::db::{Database, Session};

mod adjacency;
mod conformance;
mod functions;
mod graph_indexes;
mod limits;
mod native;
mod reads;
mod refusals;
mod search_choice;
mod statistics;

/// The bank of the standard's fixture (§23.1): accounts a1 to a6, the
/// country Zembla c1 and the city Ankh-Morpork c2, which is a country too,
/// phones p1 and p2, transfers t1 to t8, where each account is and which
/// phones they share. Amounts are in millions.
const FIXTURE: &[&str] = &[
    "CREATE TABLE ACCOUNT (ID VARCHAR(4) PRIMARY KEY, OWNER VARCHAR(20), IS_BLOCKED VARCHAR(3))",
    "CREATE TABLE COUNTRY (ID VARCHAR(4) PRIMARY KEY, NAME VARCHAR(20))",
    "CREATE TABLE CITY (ID VARCHAR(4) PRIMARY KEY, NAME VARCHAR(20))",
    "CREATE TABLE PHONE (ID VARCHAR(4) PRIMARY KEY)",
    "CREATE TABLE TRANSFER (ID VARCHAR(4) PRIMARY KEY, SRC VARCHAR(4) REFERENCES ACCOUNT, DST VARCHAR(4) REFERENCES ACCOUNT, \
     WHEN_AT DATE, AMOUNT INTEGER NOT NULL)",
    "CREATE TABLE IN_COUNTRY (ID VARCHAR(4) PRIMARY KEY, ACCT VARCHAR(4) REFERENCES ACCOUNT, PLACE VARCHAR(4) REFERENCES COUNTRY)",
    "CREATE TABLE IN_CITY (ID VARCHAR(4) PRIMARY KEY, ACCT VARCHAR(4) REFERENCES ACCOUNT, PLACE VARCHAR(4) REFERENCES CITY)",
    "CREATE TABLE HAS_PHONE (ID VARCHAR(4) PRIMARY KEY, ACCT VARCHAR(4) REFERENCES ACCOUNT, PHONE VARCHAR(4) REFERENCES PHONE)",
    "INSERT INTO ACCOUNT VALUES ('a1', 'Scott', 'no'), ('a2', 'Aretha', 'no'), ('a3', 'Mike', 'no'), ('a4', 'Jay', 'yes'), \
     ('a5', 'Charles', 'no'), ('a6', 'Dave', 'no')",
    "INSERT INTO COUNTRY VALUES ('c1', 'Zembla')",
    "INSERT INTO CITY VALUES ('c2', 'Ankh-Morpork')",
    "INSERT INTO PHONE VALUES ('p1'), ('p2')",
    "INSERT INTO TRANSFER VALUES ('t1', 'a1', 'a3', DATE '2020-01-01', 8), ('t2', 'a3', 'a2', DATE '2020-02-01', 10), \
     ('t3', 'a2', 'a4', DATE '2020-03-01', 10), ('t4', 'a4', 'a6', DATE '2020-04-01', 10), \
     ('t5', 'a6', 'a3', DATE '2020-06-01', 10), ('t6', 'a6', 'a5', DATE '2020-07-01', 4), \
     ('t7', 'a3', 'a5', DATE '2020-08-01', 6), ('t8', 'a5', 'a1', DATE '2020-09-01', 9)",
    "INSERT INTO IN_COUNTRY VALUES ('li1', 'a1', 'c1'), ('li3', 'a3', 'c1'), ('li5', 'a5', 'c1')",
    "INSERT INTO IN_CITY VALUES ('li2', 'a2', 'c2'), ('li4', 'a4', 'c2'), ('li6', 'a6', 'c2')",
    "INSERT INTO HAS_PHONE VALUES ('hp1', 'a1', 'p1'), ('hp2', 'a5', 'p1'), ('hp3', 'a2', 'p2'), ('hp4', 'a3', 'p2')",
];

/// The graph BANK of the fixture, its labels as [P1] Fig. 1 writes them.
const BANK: &str = "CREATE PROPERTY GRAPH BANK \
     VERTEX TABLES ( \
       ACCOUNT LABEL ACCOUNT PROPERTIES (ID, OWNER, IS_BLOCKED), \
       COUNTRY LABEL COUNTRY PROPERTIES (ID, NAME), \
       CITY LABEL CITY LABEL COUNTRY PROPERTIES (ID, NAME), \
       PHONE LABEL PHONE PROPERTIES (ID)) \
     EDGE TABLES ( \
       TRANSFER SOURCE KEY (SRC) REFERENCES ACCOUNT (ID) DESTINATION KEY (DST) REFERENCES ACCOUNT (ID) \
         LABEL TRANSFER PROPERTIES (ID, WHEN_AT, AMOUNT), \
       IN_COUNTRY SOURCE KEY (ACCT) REFERENCES ACCOUNT (ID) DESTINATION KEY (PLACE) REFERENCES COUNTRY (ID) \
         LABEL ISLOCATEDIN PROPERTIES (ID), \
       IN_CITY SOURCE KEY (ACCT) REFERENCES ACCOUNT (ID) DESTINATION KEY (PLACE) REFERENCES CITY (ID) \
         LABEL ISLOCATEDIN PROPERTIES (ID), \
       HAS_PHONE UNDIRECTED SOURCE KEY (ACCT) REFERENCES ACCOUNT (ID) DESTINATION KEY (PHONE) REFERENCES PHONE (ID) \
         LABEL HASPHONE PROPERTIES (ID))";

/// A database of the fixture and its graph BANK, and a session of A.
fn bank() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    for sql in FIXTURE {
        s.run(sql).unwrap();
    }
    s.run(BANK).unwrap();
    (db, s)
}

/// The rows of `query` in `s`, each its values joined by commas, sorted.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap_or_else(|e| panic!("{query}: {e:?}"));
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap_or_else(|e| panic!("{query}: {e:?}")) {
        out.push(row.iter().map(|v| v.to_string().trim().to_owned()).collect::<Vec<_>>().join(","));
    }
    out.sort();
    out
}

/// The SQLCODE and message `query` fails with in `s`.
fn refused(s: &Session, query: &str) -> (i32, String) {
    let e = s.open_cursor(query).and_then(|mut c| c.fetch().map(|_| ())).map(|()| panic!("{query}: accepted")).unwrap_err();
    (e.sqlcode(), e.msg)
}
mod joins;
