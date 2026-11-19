//! \[EXTENSION\] [GS §18] INTO PROPERTY of a property graph named in place
//! of the edges: the result written to its vertex tables, each row the
//! value of the vertex its identity names, with the privileges, the
//! transaction and the durability of any MERGE.

use turborel::db::{Database, Session};

use crate::common::{Scratch, failed};

use super::named::{banked, identity};
use super::{graph, query};

/// The SUM of column `c` of the bank's three vertex tables, in thousandths,
/// and how many of their rows it is null in.
fn summed(s: &Session, c: &str) -> Vec<String> {
    let all = format!("SELECT {c} FROM ACCOUNT UNION ALL SELECT {c} FROM PLACE UNION ALL SELECT {c} FROM PHONE");
    query(s, &format!("SELECT CAST(SUM(V) * 1000 AS INTEGER), COUNT(*) - COUNT(V) FROM ({all}) AS A (V)"))
}

/// How many accounts hold in PR the rank PAGERANK(BANK) gives their
/// identity.
fn ranked_accounts(s: &Session) -> Vec<String> {
    let identity = "'{\"table\":\"ACCOUNT\",\"key\":[' || TRIM(CAST(A.ACCT_ID AS VARCHAR(5))) || ']}'";
    query(
        s,
        &format!("SELECT COUNT(*) FROM ACCOUNT AS A JOIN GRAPH_ALGO.PAGERANK(B.BANK) AS R ON R.VERTEX = {identity} WHERE A.PR = R.RANK"),
    )
}

/// Verifies: [EXTENSION] [GS §18] [GS §23.1] PAGERANK(BANK) INTO PROPERTY
/// PR runs PAGERANK once, one row of GRAPH_ALGO_RUNS, and writes each
/// vertex's rank to a column PR added to each of ACCOUNT, PLACE and PHONE, of DOUBLE PRECISION, its rows the vertices the
/// identities name, so the ranks sum to 1 and none is null; written
/// again, replaced; OF names one vertex table alone, a component of
/// VARCHAR identities; of the statement's transaction, so ROLLBACK undoes
/// it. A key column, a table of no vertex, or no OF of a table of edges
/// is refused (-103).
#[test]
fn a_graph_property_is_written_to_each_vertex_table() {
    let db = Database::new();
    banked(&db);
    let s = db.session("B");
    s.run("GRAPH_ALGO.PAGERANK(BANK) INTO PROPERTY PR").unwrap();
    let runs = "SELECT COUNT(*), MAX(ROWS_RETURNED) FROM INFORMATION_SCHEMA.GRAPH_ALGO_RUNS WHERE ALGORITHM = 'PAGERANK'";
    assert_eq!(query(&s, runs), ["1,10"], "one run for the three vertex tables");
    assert_eq!(summed(&s, "PR"), ["1000,0"]);
    assert_eq!(ranked_accounts(&s), ["6"]);
    assert_eq!(query(&s, runs), ["2,10"], "a query after runs again");
    let types = "SELECT TABLE_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE COLUMN_NAME = 'PR' ORDER BY 1";
    assert_eq!(query(&s, types), ["ACCOUNT,DOUBLE PRECISION", "PHONE,DOUBLE PRECISION", "PLACE,DOUBLE PRECISION"]);
    s.run(&format!("GRAPH_ALGO.BFS_LEVELS(BANK, {}) INTO PROPERTY HOPS", identity("ACCOUNT", "6"))).unwrap();
    s.run(&format!("GRAPH_ALGO.BFS_LEVELS(BANK, {}) INTO PROPERTY HOPS", identity("ACCOUNT", "4"))).unwrap();
    assert_eq!(query(&s, "SELECT ACCT_ID, HOPS FROM ACCOUNT ORDER BY 1"), ["1,3", "2,3", "3,2", "4,0", "5,2", "6,1"]);
    assert_eq!(query(&s, "SELECT PLACE_ID, HOPS FROM PLACE ORDER BY 1"), ["1,3", "2,1"]);
    s.run("GRAPH_ALGO.WCC(BANK) INTO PROPERTY PART OF PLACE").unwrap();
    assert_eq!(
        query(&s, "SELECT PLACE_ID, PART FROM PLACE ORDER BY 1"),
        ["1,{\"table\":\"ACCOUNT\",\"key\":[1]}", "2,{\"table\":\"ACCOUNT\",\"key\":[1]}"]
    );
    assert_eq!(failed(&s, "SELECT PART FROM ACCOUNT").sqlcode(), -103, "OF names PLACE alone");
    s.commit().unwrap();
    s.run("GRAPH_ALGO.K_CORE(BANK) INTO PROPERTY CORE").unwrap();
    s.rollback().unwrap();
    assert_eq!(failed(&s, "SELECT CORE FROM PHONE").sqlcode(), -103, "ROLLBACK undoes it");
    assert_eq!(s.run("GRAPH_ALGO.WCC(BANK) INTO PROPERTY P OF ACCOUNT (ACCT_ID)").unwrap_err().sqlcode(), -103);
    assert_eq!(s.run("GRAPH_ALGO.WCC(BANK) INTO PROPERTY P OF TRANSFER").unwrap_err().sqlcode(), -103);
    s.run("CREATE TABLE E (SRC INTEGER, DST INTEGER)").unwrap();
    assert_eq!(s.run("GRAPH_ALGO.WCC(TABLE (E)) INTO PROPERTY P").unwrap_err().sqlcode(), -103);
}

/// Verifies: [EXTENSION] [GS §18] [GS §4.1 R4.2] [GS §4.2] INTO PROPERTY
/// of a graph of keys of two columns, each read from the identity as of
/// its column's type; and of SYS_GRAPH, each node table's rows matched by
/// `$NODE_ID`.
#[test]
fn a_graph_property_of_composite_keys_and_of_sys_graph() {
    let db = graph();
    let s = db.session("G");
    s.run("CREATE TABLE R (REGION CHAR(2), NUM INTEGER, PRIMARY KEY (REGION, NUM))").unwrap();
    s.run("CREATE TABLE L (FR CHAR(2), FN INTEGER, TR CHAR(2), TN INTEGER, PRIMARY KEY (FR, FN, TR, TN))").unwrap();
    s.run("INSERT INTO R VALUES ('eu', 1), ('eu', 2), ('us', 1), ('us', 2)").unwrap();
    s.run("INSERT INTO L VALUES ('eu', 1, 'eu', 2), ('eu', 2, 'us', 1)").unwrap();
    s.run(
        "CREATE PROPERTY GRAPH CG VERTEX TABLES (R) EDGE TABLES (L SOURCE KEY (FR, FN) REFERENCES R (REGION, NUM) \
         DESTINATION KEY (TR, TN) REFERENCES R (REGION, NUM)) OPTIONS (TRUSTED MODE)",
    )
    .unwrap();
    s.run(&format!("GRAPH_ALGO.BFS_LEVELS(CG, {}) INTO PROPERTY HOPS", identity("R", "\"eu\",1"))).unwrap();
    assert_eq!(query(&s, "SELECT REGION, NUM, HOPS FROM R ORDER BY 1, 2"), ["eu,1,0", "eu,2,1", "us,1,2", "us,2,NULL"]);
    s.run("CREATE TABLE N (NAME VARCHAR(5)) AS NODE").unwrap();
    s.run("CREATE TABLE M (NAME VARCHAR(5)) AS NODE").unwrap();
    s.run("CREATE TABLE LINK (W INTEGER) AS EDGE").unwrap();
    s.run("INSERT INTO N VALUES ('a'), ('b')").unwrap();
    s.run("INSERT INTO M VALUES ('m')").unwrap();
    s.run(
        "INSERT INTO LINK ($FROM_ID, $TO_ID, W) SELECT X.$NODE_ID, Y.$NODE_ID, 1 FROM N AS X, M AS Y WHERE X.NAME = 'a' AND Y.NAME = 'm'",
    )
    .unwrap();
    s.run("GRAPH_ALGO.WCC(SYS_GRAPH) INTO PROPERTY PART").unwrap();
    let parts = "SELECT N.NAME, M.NAME FROM N JOIN M ON N.PART = M.PART";
    assert_eq!(query(&s, parts), ["a,m"]);
    assert_eq!(query(&s, "SELECT COUNT(PART) FROM N"), ["2"]);
}

/// Verifies: [EXTENSION] [GS §18 R18.2] INTO PROPERTY of a graph needs
/// what its MERGEs and ALTER TABLE need: a user who may read the bank's
/// tables but update none is refused, and one granted UPDATE of the column
/// on each vertex table writes it.
#[test]
fn a_graph_property_needs_update_privilege() {
    let db = Database::new();
    banked(&db);
    let owner = db.session("B");
    owner.run("GRAPH_ALGO.PAGERANK(BANK) INTO PROPERTY PR").unwrap();
    owner.run("UPDATE ACCOUNT SET PR = 0").unwrap();
    for t in ["ACCOUNT", "PLACE", "PHONE", "TRANSFER", "LOCATED_IN", "HAS_PHONE"] {
        owner.run(&format!("GRANT SELECT ON {t} TO X")).unwrap();
    }
    owner.commit().unwrap();
    let x = db.session("X");
    let refusal = x.run("GRAPH_ALGO.PAGERANK(B.BANK) INTO PROPERTY PR").unwrap_err();
    assert!(refusal.sqlcode() < 0, "{refusal:?}");
    x.rollback().unwrap();
    for t in ["ACCOUNT", "PLACE", "PHONE"] {
        owner.run(&format!("GRANT UPDATE (PR) ON {t} TO X")).unwrap();
    }
    owner.commit().unwrap();
    x.run("GRAPH_ALGO.PAGERANK(B.BANK) INTO PROPERTY PR").unwrap();
    x.commit().unwrap();
    let s = db.session("B");
    assert_eq!(summed(&s, "PR"), ["1000,0"]);
}

/// Verifies: [EXTENSION] [GS §18] A graph property written and committed
/// is kept across reopening: each vertex table's column and its values.
#[test]
fn a_graph_property_is_kept_across_reopening() {
    let scratch = Scratch::new("graph-algo-property-reopen");
    let dir = scratch.path("db");
    {
        let db = Database::open(&dir).unwrap();
        banked(&db);
        let s = db.session("B");
        s.run("GRAPH_ALGO.PAGERANK(BANK) INTO PROPERTY PR").unwrap();
        s.commit().unwrap();
    }
    let db = Database::open(&dir).unwrap();
    let s = db.session("B");
    assert_eq!(summed(&s, "PR"), ["1000,0"]);
    assert_eq!(ranked_accounts(&s), ["6"]);
}
