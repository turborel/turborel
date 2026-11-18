//! GQL's data-modifying statements (graph standard §12): INSERT, SET,
//! REMOVE, DELETE and DETACH DELETE over a graph over tables (Mode A,
//! R12.1), over node and edge tables (Mode B), and all or none.

use super::{bank, direct, failed, gql};
use turborel::db::{Database, Session};

/// Verifies: [EXTENSION] graph standard §12 R12.1: INSERT writes a
/// vertex and an edge to the one table their labels name, SET a
/// property's column, and the rows are SQL's.
#[test]
fn insert_and_set_over_tables_12_1() {
    let (_db, s) = bank();
    gql(&s, "INSERT (:ACCOUNT {acct_id: 5, owner: 'Eve', is_blocked: false})");
    gql(&s, "MATCH (a:ACCOUNT {owner: 'Eve'}), (b:ACCOUNT {owner: 'Ann'}) INSERT (a)-[:TRANSFER {txn_id: 14, amount: 5}]->(b)");
    assert_eq!(direct(&s, "SELECT FROM_ACCT, TO_ACCT, AMOUNT FROM A.TRANSFER WHERE TXN_ID = 14"), ["5,1,5.00"]);
    let set = gql(&s, "MATCH (a:ACCOUNT {owner: 'Eve'}) SET a.is_blocked = true RETURN a.is_blocked AS b");
    assert_eq!(set, ["TRUE"]);
    assert_eq!(direct(&s, "SELECT IS_BLOCKED FROM A.ACCOUNT WHERE ACCT_ID = 5"), ["TRUE"]);
    let removed = gql(&s, "MATCH (a:ACCOUNT {owner: 'Eve'}) REMOVE a.is_blocked RETURN a.owner AS o, a.is_blocked AS b");
    assert_eq!(removed, ["Eve,NULL"]);
}

/// Verifies: [EXTENSION] graph standard §12 R12.1, §22: a write no one
/// table can take fails (GRF-0051), and a vertex with edges is deleted
/// only by DETACH DELETE (GRF-0052), all or none.
#[test]
fn delete_and_refusals_12_1() {
    let (_db, s) = bank();
    let (code, msg) = failed(&s, "EXECUTE GQL ON GRAPH BANK 'INSERT (:NOWHERE {x: 1})'");
    assert_eq!(code, -103, "{msg}");
    assert!(msg.contains("GRF-0051"), "{msg}");
    let (code, msg) = failed(&s, "EXECUTE GQL ON GRAPH BANK 'INSERT (:CITY {city_name: ''Pisa''})'");
    assert_eq!(code, -103, "{msg}");
    assert!(msg.contains("GRF-0051"), "{msg}");
    assert_eq!(direct(&s, "SELECT COUNT(*) FROM A.CITY"), ["2"]);
    let (code, msg) = failed(&s, "EXECUTE GQL ON GRAPH BANK 'MATCH (a:ACCOUNT {owner: ''Dee''}) DELETE a'");
    assert_eq!(code, -201, "{msg}");
    assert!(msg.contains("GRF-0052"), "{msg}");
    assert_eq!(direct(&s, "SELECT COUNT(*) FROM A.ACCOUNT"), ["4"]);
    let (code, msg) = failed(&s, "EXECUTE GQL ON GRAPH BANK 'MATCH (a:ACCOUNT {owner: ''Bob''}) SET a:CITY'");
    assert_eq!(code, -103, "{msg}");
    gql(&s, "MATCH (a:ACCOUNT {owner: 'Dee'}) DETACH DELETE a");
    assert_eq!(direct(&s, "SELECT COUNT(*) FROM A.ACCOUNT"), ["3"]);
    assert_eq!(direct(&s, "SELECT COUNT(*) FROM A.TRANSFER"), ["3"]);
    gql(&s, "MATCH ()-[t:TRANSFER]->() WHERE t.amount < 60 DELETE t");
    assert_eq!(direct(&s, "SELECT TXN_ID FROM A.TRANSFER"), ["10"]);
}

/// Verifies: [EXTENSION] graph standard §12 R12.3: a procedure's writes
/// are its transaction's, undone by ROLLBACK, and a failing statement
/// undoes the procedure's every write.
#[test]
fn writes_are_the_transactions_12_3() {
    let (_db, s) = bank();
    let (code, _) = failed(
        &s,
        "EXECUTE GQL ON GRAPH BANK 'INSERT (:ACCOUNT {acct_id: 6, owner: ''Fay''}) INSERT (:ACCOUNT {acct_id: 6, owner: ''Gus''})'",
    );
    assert!(code < 0);
    assert_eq!(direct(&s, "SELECT COUNT(*) FROM A.ACCOUNT"), ["4"]);
    gql(&s, "INSERT (:ACCOUNT {acct_id: 7, owner: 'Hal'})");
    s.rollback().unwrap();
    assert_eq!(direct(&s, "SELECT COUNT(*) FROM A.ACCOUNT"), ["4"]);
}

/// Node and edge tables of schema A, and a session of A.
pub(super) fn native() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A \
         CREATE TABLE PERSON (NAME VARCHAR(20), AGE INTEGER) AS NODE \
         CREATE TABLE CITY (CITY_NAME VARCHAR(20)) AS NODE \
         CREATE TABLE KNOWS (SINCE INTEGER) AS EDGE \
         CREATE TABLE LIVES_IN (CONSTRAINT LIVES CONNECTION (PERSON TO CITY)) AS EDGE",
    )
    .unwrap();
    let s = db.session("A");
    (db, s)
}

/// The rows of GQL `text` on SYS_GRAPH.
fn sys(s: &Session, text: &str) -> Vec<String> {
    direct(s, &format!("EXECUTE GQL ON GRAPH SYS_GRAPH '{}'", text.replace('\'', "''")))
}

/// Verifies: [EXTENSION] graph standard §4.2, §12: GQL writes node and
/// edge tables through SYS_GRAPH, each node's identity drawn by its
/// table, and reads them back.
#[test]
fn native_tables_12() {
    let (_db, s) = native();
    sys(&s, "INSERT (:PERSON {name: 'Ann', age: 30})-[:KNOWS {since: 2020}]->(:PERSON {name: 'Bob', age: 40})");
    sys(&s, "MATCH (p:PERSON {name: 'Bob'}) INSERT (p)-[:LIVES_IN]->(:CITY {city_name: 'Rome'})");
    let got = sys(&s, "MATCH (a:PERSON)-[k:KNOWS]->(b:PERSON)-[:LIVES_IN]->(c:CITY) RETURN a.name AS a, k.since AS k, c.city_name AS c");
    assert_eq!(got, ["Ann,2020,Rome"]);
    assert_eq!(direct(&s, "SELECT COUNT(*) FROM KNOWS"), ["1"]);
    let (code, msg) = failed(&s, "EXECUTE GQL ON GRAPH SYS_GRAPH 'MATCH (c:CITY), (p:PERSON {name: ''Ann''}) INSERT (c)-[:LIVES_IN]->(p)'");
    assert_eq!(code, -201, "{msg}");
    sys(&s, "MATCH (p:PERSON {name: 'Bob'}) DETACH DELETE p");
    assert_eq!(direct(&s, "SELECT COUNT(*) FROM KNOWS"), ["0"]);
    assert_eq!(sys(&s, "MATCH (p:PERSON) RETURN p.name AS n"), ["Ann"]);
}

/// Verifies: [EXTENSION] graph standard §12, [P2 §5.1]: an inline CALL
/// whose procedure modifies data writes once for each binding of its
/// scope, and NEXT goes on from what the statement before returned.
#[test]
fn modifying_call_and_next_12() {
    let (_db, s) = bank();
    let got = gql(&s, "MATCH (a:ACCOUNT) WHERE a.is_blocked CALL (a) { SET a.owner = a.owner || '?' RETURN a.owner AS n } RETURN n");
    assert_eq!(got, ["Cy?"]);
    let next = gql(&s, "MATCH (a:ACCOUNT {owner: 'Cy?'}) RETURN a NEXT SET a.is_blocked = false RETURN a.is_blocked AS b");
    assert_eq!(next, ["FALSE"]);
    assert_eq!(direct(&s, "SELECT OWNER, IS_BLOCKED FROM A.ACCOUNT WHERE ACCT_ID = 3"), ["Cy?,FALSE"]);
}
