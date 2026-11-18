//! Node and edge tables (RULES.md D109), Mode B of the property
//! graph implementation standard (§4.2): CREATE TABLE ... AS NODE and AS
//! EDGE, their hidden pseudo-columns and the identities in them, edges
//! inserted between nodes, CONNECTION constraints, a node's deletion and
//! its edges (R12.1), and the implicit graph SYS_GRAPH of a schema. The
//! modules below hold what other statements do to them, the Information
//! Schema's views, and node and edge tables kept across reopening.

use turborel::db::{Database, Session};

mod catalog;
mod recovery;
mod statements;

/// Schema A of people and cities, who knows whom and who lives where,
/// LIVES_IN joining people to cities alone, and deleted with its nodes.
const SOCIAL: &str = "CREATE SCHEMA AUTHORIZATION A \
     CREATE TABLE PERSON (NAME VARCHAR(20), AGE INTEGER) AS NODE \
     CREATE TABLE CITY (CITY_NAME VARCHAR(20)) AS NODE \
     CREATE TABLE KNOWS (SINCE INTEGER) AS EDGE \
     CREATE TABLE LIVES_IN (CONSTRAINT LIVES CONNECTION (PERSON TO CITY) ON DELETE CASCADE) AS EDGE";

/// A database of `SOCIAL`.
fn social() -> Database {
    let db = Database::new();
    db.schema(SOCIAL).unwrap();
    db
}

/// The values of `query` in `s`, each row its values joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// That `sql` is refused in `s` with SQLCODE `code` and a message that
/// holds `holds`, the transaction rolled back after.
fn refused(s: &Session, sql: &str, (code, holds): (i32, &str)) {
    let e = s.run(sql).err().unwrap_or_else(|| panic!("{sql} succeeded"));
    s.rollback().unwrap();
    assert_eq!(e.sqlcode(), code, "{sql}: {}", e.msg);
    assert!(e.msg.contains(holds), "{sql}: {} lacks {holds}", e.msg);
}

/// The identity of row `id` of node table `table` of schema A.
fn node(table: &str, id: u32) -> String {
    format!(r#"{{"type":"node","schema":"A","table":"{table}","id":{id}}}"#)
}

/// Verifies: [EXTENSION] §4.2 R5.1: rows inserted into a node table are
/// given identities of its table numbered from 0, which `*` does not
/// show and `$NODE_ID` names in any case; an edge is inserted giving its
/// endpoints' identities, written or found by a subquery, and is given
/// its own.
#[test]
fn nodes_and_edges_are_inserted_4_2() {
    let db = social();
    let a = db.session("A");
    a.run("INSERT INTO PERSON VALUES ('ANN', 30), ('BOB', 40)").unwrap();
    a.run("INSERT INTO CITY (CITY_NAME) VALUES ('ROME')").unwrap();
    assert_eq!(rows(&a, "SELECT * FROM PERSON ORDER BY NAME"), ["ANN,30", "BOB,40"]);
    assert_eq!(rows(&a, "SELECT P.* FROM PERSON P ORDER BY NAME"), ["ANN,30", "BOB,40"]);
    assert_eq!(
        rows(&a, "SELECT $node_id, NAME FROM PERSON ORDER BY NAME"),
        [format!("{},ANN", node("PERSON", 0)), format!("{},BOB", node("PERSON", 1)),]
    );
    a.run(&format!("INSERT INTO KNOWS ($FROM_ID, $TO_ID, SINCE) VALUES ('{}', '{}', 2020)", node("PERSON", 0), node("PERSON", 1))).unwrap();
    a.run(
        "INSERT INTO LIVES_IN ($FROM_ID, $TO_ID) \
         SELECT P.$NODE_ID, C.$NODE_ID FROM PERSON P, CITY C WHERE P.NAME = 'BOB'",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT $EDGE_ID, SINCE FROM KNOWS"), [r#"{"type":"edge","schema":"A","table":"KNOWS","id":0},2020"#]);
    assert_eq!(rows(&a, "SELECT * FROM KNOWS"), ["2020"]);
    let lives = "SELECT P.NAME, C.CITY_NAME FROM PERSON P, LIVES_IN L, CITY C WHERE L.$FROM_ID = P.$NODE_ID AND L.$TO_ID = C.$NODE_ID";
    assert_eq!(rows(&a, lives), ["BOB,ROME"]);
    a.commit().unwrap();
}
