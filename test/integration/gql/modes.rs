//! Mode C (graph standard §4.3): graph types, GQL graphs typed by them
//! and open, the tables each is held in, and what the Information Schema
//! shows of them.

use super::{direct, failed};
use turborel::db::{Database, Session};

/// A graph type of people who know each other and the cities they live
/// in.
const SOCIAL: &str = "CREATE GRAPH TYPE SOCIAL { \
     NODE PERSON {NAME STRING NOT NULL, AGE INT32}, \
     NODE CITY {NAME STRING}, \
     EDGE KNOWS {SINCE INT32} CONNECTING (PERSON -> PERSON), \
     EDGE LIVES_IN CONNECTING (PERSON -> CITY) }";

/// A session of schema A.
fn session() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    (db, s)
}

/// The rows of GQL `text` on graph `g`.
fn on(s: &Session, g: &str, text: &str) -> Vec<String> {
    direct(s, &format!("EXECUTE GQL ON GRAPH {g} '{}'", text.replace('\'', "''")))
}

/// Verifies: [EXTENSION] graph standard §4.3: CREATE GRAPH TYPE and a
/// graph typed by it, its tables made, written and read by GQL.
#[test]
fn typed_graph_4_3() {
    let (_db, s) = session();
    s.run(SOCIAL).unwrap();
    s.run("CREATE GRAPH NET TYPED SOCIAL").unwrap();
    on(&s, "NET", "INSERT (:PERSON {name: 'Ann', age: 30})-[:KNOWS {since: 2020}]->(:PERSON {name: 'Bob', age: 40})");
    on(&s, "NET", "MATCH (p:PERSON {name: 'Ann'}) INSERT (p)-[:LIVES_IN]->(:CITY {name: 'Oslo'})");
    let got = on(&s, "NET", "MATCH (a:PERSON)-[k:KNOWS]->(b:PERSON) RETURN a.name AS a, b.name AS b, k.since AS s");
    assert_eq!(got, ["Ann,Bob,2020"]);
    let lives = on(&s, "NET", "MATCH (p)-[:LIVES_IN]->(c) RETURN p.name AS p, c.name AS c");
    assert_eq!(lives, ["Ann,Oslo"]);
    assert_eq!(direct(&s, "SELECT COUNT(*) FROM \"NET$PERSON\""), ["2"]);
}

/// Verifies: [EXTENSION] graph standard §4.3, §22: a typed graph refuses
/// an element of a type it lacks, and a property of no type (GRF-0053).
#[test]
fn typed_graph_refuses_what_its_type_lacks_4_3() {
    let (_db, s) = session();
    s.run(SOCIAL).unwrap();
    s.run("CREATE GRAPH NET TYPED SOCIAL").unwrap();
    let (code, msg) = failed(&s, "EXECUTE GQL ON GRAPH NET 'INSERT (:ROBOT {name: ''R2''})'");
    assert_eq!(code, -201, "{msg}");
    assert!(msg.contains("GRF-0053"), "{msg}");
    let (code, msg) = failed(&s, "EXECUTE GQL ON GRAPH NET 'INSERT (:PERSON {name: ''Ann'', shoe: 42})'");
    assert_eq!(code, -201, "{msg}");
    assert!(msg.contains("GRF-0053"), "{msg}");
    assert_eq!(direct(&s, "SELECT COUNT(*) FROM \"NET$PERSON\""), ["0"]);
}

/// Verifies: [EXTENSION] graph standard §4.3: an open graph takes any
/// labels and properties, read as JSON.
#[test]
fn open_graph_4_3() {
    let (_db, s) = session();
    s.run("CREATE GRAPH FREE ANY").unwrap();
    on(&s, "FREE", "INSERT (:A {x: 1})-[:R {w: 'heavy'}]->(:B&C {y: true})");
    let got = on(&s, "FREE", "MATCH (a:A)-[r:R]->(b) WHERE a.x = 1 RETURN r.w AS w, LABELS(b) AS l");
    assert_eq!(got, ["heavy,|B|C|"]);
    on(&s, "FREE", "MATCH (b:B) SET b.z = 'set', b:D REMOVE b:C");
    assert_eq!(on(&s, "FREE", "MATCH (b:D) RETURN b.z AS z, LABELS(b) AS l"), ["set,|B|D|"]);
}

/// Verifies: [EXTENSION] graph standard §4.3, [GS §7]: GQL reads an open
/// graph's elements as GRAPH_TABLE does its NODE and EDGE tables, of the
/// same identities, labels and properties.
#[test]
fn open_graph_is_its_graph_table_4_3() {
    let (_db, s) = session();
    s.run("CREATE GRAPH FREE ANY").unwrap();
    on(&s, "FREE", "INSERT (:A {x: 1})-[:R {w: 'heavy'}]->(:B {y: 2}), (:A {x: 3})");
    let mut ours =
        on(&s, "FREE", "MATCH (a)-[r]->(b) RETURN ELEMENT_ID(a) AS a, ELEMENT_ID(r) AS r, ELEMENT_ID(b) AS b, LABELS(b) AS l, b.y AS y");
    let theirs = direct(
        &s,
        "SELECT * FROM GRAPH_TABLE (FREE MATCH (a IS NODE)-[r IS EDGE]->(b IS NODE) COLUMNS (ELEMENT_ID(a) AS a, ELEMENT_ID(r) AS r, \
         ELEMENT_ID(b) AS b, b.LABELS AS l, JSON_VALUE(b.PROPS, 'lax $.\"Y\"' RETURNING INTEGER) AS y)) AS G",
    );
    ours.sort();
    assert_eq!(ours, theirs);
    assert_eq!(ours.len(), 1);
    assert_eq!(on(&s, "FREE", "MATCH (a:A) RETURN COUNT(*) AS n"), ["2"]);
}

/// Verifies: [EXTENSION] graph standard §20: GRAPH_TYPES and GQL_GRAPHS
/// show each graph type and GQL graph, and DROP takes them away.
#[test]
fn information_schema_and_drop_20() {
    let (_db, s) = session();
    s.run(SOCIAL).unwrap();
    s.run("CREATE GRAPH NET TYPED SOCIAL").unwrap();
    s.run("CREATE GRAPH FREE ANY").unwrap();
    assert_eq!(direct(&s, "SELECT GRAPH_TYPE_NAME FROM INFORMATION_SCHEMA.GRAPH_TYPES"), ["SOCIAL"]);
    let graphs = direct(&s, "SELECT GRAPH_NAME, GRAPH_TYPE_NAME FROM INFORMATION_SCHEMA.GQL_GRAPHS ORDER BY 1");
    assert_eq!(graphs, ["FREE,NULL", "NET,SOCIAL"]);
    let (code, msg) = failed(&s, "DROP GRAPH TYPE SOCIAL");
    assert!(code < 0, "{msg}");
    s.run("DROP GRAPH NET").unwrap();
    s.run("DROP GRAPH TYPE SOCIAL").unwrap();
    s.run("DROP GRAPH FREE").unwrap();
    s.run("DROP GRAPH IF EXISTS FREE").unwrap();
    assert_eq!(direct(&s, "SELECT GRAPH_NAME FROM INFORMATION_SCHEMA.GQL_GRAPHS"), Vec::<String>::new());
    let (code, _) = failed(&s, "SELECT * FROM \"NET$PERSON\"");
    assert!(code < 0);
}
