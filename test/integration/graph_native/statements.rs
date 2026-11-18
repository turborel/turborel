//! What statements do to node and edge tables (§4.2): CONNECTION
//! constraints checked on INSERT and UPDATE, a node's deletion with its
//! edges (R12.1), the identities no statement writes, and the ALTER,
//! TRUNCATE and DROP of node and edge tables.

use super::{node, refused, rows, social};

/// Verifies: [EXTENSION] §4.2 R12.2: an edge of LIVES_IN joins a person
/// to a city alone, else GRF-0053; an endpoint that is no node's
/// identity, or of a node no row has, is refused; an UPDATE of an
/// endpoint is checked as an INSERT is; an edge table of no CONNECTION
/// constraint joins any nodes of its schema.
#[test]
fn connections_are_enforced_4_2() {
    let db = social();
    let a = db.session("A");
    a.run("INSERT INTO PERSON VALUES ('ANN', 30), ('BOB', 40)").unwrap();
    a.run("INSERT INTO CITY VALUES ('ROME')").unwrap();
    a.commit().unwrap();
    let (ann, bob, rome) = (node("PERSON", 0), node("PERSON", 1), node("CITY", 0));
    let lives = |from: &str, to: &str| format!("INSERT INTO LIVES_IN ($FROM_ID, $TO_ID) VALUES ('{from}', '{to}')");
    refused(&a, &lives(&rome, &ann), (-201, "GRF-0053"));
    refused(&a, &lives(&ann, &bob), (-201, "CONNECTION A.LIVES"));
    refused(&a, &lives(&ann, &node("CITY", 7)), (-201, "no row of A.CITY"));
    refused(&a, &lives(&ann, "ROME"), (-201, "GRF-0053"));
    refused(&a, &lives(&ann, r#"{"type":"edge","schema":"A","table":"KNOWS","id":0}"#), (-201, "GRF-0053"));
    refused(&a, &format!("INSERT INTO KNOWS ($FROM_ID, $TO_ID) VALUES ('{ann}', '{}')", node("KNOWS", 0)), (-201, "GRF-0053"));
    refused(&a, "INSERT INTO LIVES_IN ($FROM_ID) VALUES (NULL)", (-201, "NOT NULL"));
    // The same identity written otherwise is one node.
    a.run(&lives(r#"{ "id" : 0, "table" : "PERSON", "schema" : "A", "type" : "node" }"#, &rome)).unwrap();
    assert_eq!(rows(&a, "SELECT $FROM_ID FROM LIVES_IN"), std::slice::from_ref(&ann));
    refused(&a, &format!("UPDATE LIVES_IN SET $TO_ID = '{bob}'"), (-201, "GRF-0053"));
    a.run(&format!("INSERT INTO KNOWS ($FROM_ID, $TO_ID) VALUES ('{ann}', '{rome}')")).unwrap();
    a.run(&format!("UPDATE KNOWS SET $TO_ID = '{bob}', SINCE = 1999")).unwrap();
    assert_eq!(rows(&a, "SELECT $TO_ID, SINCE FROM KNOWS"), [format!("{bob},1999")]);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] §4.2 R12.1: a node of edges in KNOWS, of no
/// CONNECTION constraint, or one of NO ACTION, is not deleted, GRF-0052; one
/// of edges in LIVES_IN alone is deleted with them, as is a city; a
/// node of no edges is deleted.
#[test]
fn a_node_deleted_takes_or_keeps_its_edges_r12_1() {
    let db = social();
    let a = db.session("A");
    a.run("INSERT INTO PERSON VALUES ('ANN', 30), ('BOB', 40), ('CY', 50)").unwrap();
    a.run("INSERT INTO CITY VALUES ('ROME'), ('OSLO')").unwrap();
    let (ann, bob, cy) = (node("PERSON", 0), node("PERSON", 1), node("PERSON", 2));
    let (rome, oslo) = (node("CITY", 0), node("CITY", 1));
    a.run(&format!("INSERT INTO KNOWS ($FROM_ID, $TO_ID) VALUES ('{ann}', '{bob}'), ('{cy}', '{cy}')")).unwrap();
    a.run(&format!("INSERT INTO LIVES_IN ($FROM_ID, $TO_ID) VALUES ('{bob}', '{rome}'), ('{cy}', '{rome}'), ('{ann}', '{oslo}')")).unwrap();
    a.commit().unwrap();
    refused(&a, "DELETE FROM PERSON WHERE NAME = 'ANN'", (-201, "GRF-0052"));
    refused(&a, "DELETE FROM PERSON WHERE NAME = 'BOB'", (-201, "has edges in A.KNOWS"));
    // A CONNECTION constraint of NO ACTION deletes no edge.
    a.run("ALTER TABLE KNOWS ADD CONSTRAINT K0 CONNECTION (PERSON TO PERSON) ON DELETE NO ACTION").unwrap();
    refused(&a, "DELETE FROM PERSON WHERE NAME = 'ANN'", (-201, "GRF-0052"));
    a.run("DELETE FROM KNOWS WHERE $FROM_ID <> $TO_ID").unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM KNOWS"), ["1"]);
    a.run("DELETE FROM PERSON WHERE NAME = 'BOB'").unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM LIVES_IN"), ["2"]);
    a.commit().unwrap();
    // CY's self-loop in KNOWS forbids it still.
    refused(&a, "DELETE FROM PERSON WHERE NAME = 'CY'", (-201, "GRF-0052"));
    a.run("DELETE FROM KNOWS").unwrap();
    a.run("DELETE FROM CITY WHERE CITY_NAME = 'ROME'").unwrap();
    assert_eq!(rows(&a, "SELECT $FROM_ID FROM LIVES_IN"), std::slice::from_ref(&ann));
    a.run("DELETE FROM PERSON").unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM LIVES_IN"), ["0"]);
    a.run("INSERT INTO PERSON VALUES ('DI', 20)").unwrap();
    assert_eq!(rows(&a, "SELECT $NODE_ID FROM PERSON"), [node("PERSON", 3)]);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] §4.2: no statement gives a node or edge its
/// identity, by INSERT or UPDATE, though DEFAULT is given, and MERGE's
/// INSERT draws one as INSERT does; an edge's
/// endpoints are given by name alone; LIKE copies no pseudo-column.
#[test]
fn identities_are_the_tables_own_4_2() {
    let db = social();
    let a = db.session("A");
    refused(&a, "INSERT INTO PERSON ($NODE_ID, NAME) VALUES ('X', 'ANN')", (-103, "given only DEFAULT"));
    a.run("INSERT INTO PERSON ($NODE_ID, NAME) VALUES (DEFAULT, 'ANN')").unwrap();
    a.run("MERGE INTO PERSON USING (VALUES ('BOB')) AS S (N) ON PERSON.NAME = S.N WHEN NOT MATCHED THEN INSERT (NAME) VALUES (S.N)")
        .unwrap();
    assert_eq!(rows(&a, "SELECT $NODE_ID FROM PERSON WHERE NAME = 'BOB'"), [node("PERSON", 1)]);
    refused(&a, "UPDATE PERSON SET $NODE_ID = 'X'", (-103, "no <set clause> sets it"));
    refused(&a, "UPDATE PERSON SET $NODE_ID = DEFAULT", (-103, "no <set clause> sets it"));
    refused(&a, "INSERT INTO KNOWS VALUES (1, 'X')", (-103, ""));
    a.run("CREATE TABLE COPY (LIKE PERSON)").unwrap();
    assert_eq!(
        rows(&a, "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'COPY' ORDER BY ORDINAL_POSITION"),
        ["NAME", "AGE"]
    );
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] §4.2: AS NODE and AS EDGE make no temporary,
/// typed or system-versioned table; CONNECTION names node tables of the
/// table's schema, GRF-0004, and is of an edge table alone; a table
/// named SYS_GRAPH is not made beside node tables, nor is a graph so
/// named made, altered or dropped.
#[test]
fn node_and_edge_tables_are_defined_by_their_rules_4_2() {
    let db = social();
    let a = db.session("A");
    refused(&a, "CREATE GLOBAL TEMPORARY TABLE T (X INTEGER) AS NODE", (-103, "node or edge table"));
    refused(&a, "CREATE TABLE T (X INTEGER, PERIOD FOR SYSTEM_TIME (S, E)) AS NODE WITH SYSTEM VERSIONING", (-103, ""));
    refused(&a, "CREATE TABLE E2 (CONSTRAINT C CONNECTION (PERSON TO KNOWS)) AS EDGE", (-103, "GRF-0004"));
    refused(&a, "CREATE TABLE E2 (CONSTRAINT C CONNECTION (PERSON TO NOPE)) AS EDGE", (-103, "GRF-0004"));
    refused(&a, "CREATE TABLE N2 (X INTEGER, CONSTRAINT C CONNECTION (PERSON TO CITY)) AS NODE", (-103, "is an edge table"));
    refused(&a, "CREATE TABLE PLAIN (X INTEGER, CONSTRAINT C CONNECTION (PERSON TO CITY))", (-103, "is an edge table"));
    refused(&a, "CREATE TABLE SYS_GRAPH (X INTEGER)", (-103, "SYS_GRAPH"));
    refused(&a, "CREATE PROPERTY GRAPH SYS_GRAPH VERTEX TABLES (PERSON)", (-103, "implicit graph"));
    refused(&a, "ALTER PROPERTY GRAPH SYS_GRAPH COMPILE", (-103, "implicit graph"));
    refused(&a, "DROP PROPERTY GRAPH SYS_GRAPH", (-103, "implicit graph"));
    a.run("CREATE TABLE EMPTY AS NODE").unwrap();
    assert_eq!(rows(&a, "SELECT * FROM EMPTY"), Vec::<String>::new());
    a.run("INSERT INTO EMPTY DEFAULT VALUES").unwrap();
    assert_eq!(rows(&a, "SELECT $NODE_ID FROM EMPTY"), [node("EMPTY", 0)]);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] §4.2: ALTER TABLE adds and drops a node table's
/// own columns, its rows keeping their identities, and adds a CONNECTION
/// constraint its edges satisfy, else not, whose cascade then deletes a
/// node's edges, a self-loop once; no pseudo-column is dropped or
/// altered, nor versioning added; TRUNCATE of a node table of a schema
/// with an edge table is refused, GRF-0052, of an edge table done; DROP
/// TABLE drops a node table's edges' CONNECTION constraint only with
/// CASCADE.
#[test]
fn node_and_edge_tables_are_altered_and_dropped_4_2() {
    let db = social();
    let a = db.session("A");
    a.run("INSERT INTO PERSON VALUES ('ANN', 30), ('BOB', 40)").unwrap();
    a.run("INSERT INTO CITY VALUES ('ROME')").unwrap();
    let (ann, bob) = (node("PERSON", 0), node("PERSON", 1));
    a.run(&format!("INSERT INTO KNOWS ($FROM_ID, $TO_ID) VALUES ('{ann}', '{bob}'), ('{ann}', '{ann}')")).unwrap();
    a.commit().unwrap();
    a.run("ALTER TABLE PERSON ADD COLUMN NICK VARCHAR(10)").unwrap();
    a.run("ALTER TABLE PERSON DROP COLUMN AGE RESTRICT").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT * FROM PERSON ORDER BY NAME"), ["ANN,NULL", "BOB,NULL"]);
    assert_eq!(rows(&a, "SELECT $NODE_ID FROM PERSON WHERE NAME = 'BOB'"), std::slice::from_ref(&bob));
    a.run("INSERT INTO PERSON (NAME) VALUES ('CY')").unwrap();
    assert_eq!(rows(&a, "SELECT $NODE_ID FROM PERSON WHERE NAME = 'CY'"), [node("PERSON", 2)]);
    refused(&a, "DELETE FROM PERSON WHERE NAME = 'ANN'", (-201, "GRF-0052"));
    refused(&a, "ALTER TABLE PERSON DROP COLUMN $NODE_ID RESTRICT", (-103, "pseudo-column"));
    refused(&a, "ALTER TABLE KNOWS ALTER COLUMN $FROM_ID SET DEFAULT 'X'", (-103, "pseudo-column"));
    refused(&a, "ALTER TABLE KNOWS ADD SYSTEM VERSIONING", (-103, ""));
    refused(&a, "ALTER TABLE KNOWS ADD CONSTRAINT K CONNECTION (CITY TO CITY)", (-201, "CONNECTION A.K"));
    a.run("ALTER TABLE KNOWS ADD CONSTRAINT K CONNECTION (PERSON TO PERSON) ON DELETE CASCADE").unwrap();
    a.run("DELETE FROM PERSON WHERE NAME = 'ANN'").unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM KNOWS"), ["0"]);
    a.commit().unwrap();
    refused(&a, "TRUNCATE TABLE PERSON", (-103, "GRF-0052"));
    a.run("TRUNCATE TABLE KNOWS").unwrap();
    refused(&a, "DROP TABLE CITY RESTRICT", (-103, "CITY"));
    a.run("DROP TABLE CITY CASCADE").unwrap();
    assert_eq!(
        rows(
            &a,
            "SELECT CONSTRAINT_NAME FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE TABLE_NAME = 'LIVES_IN' AND CONSTRAINT_TYPE = 'CONNECTION'"
        ),
        Vec::<String>::new()
    );
    a.commit().unwrap();
}
