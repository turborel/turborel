//! Property graphs kept with the definitions (§4.1 R4.7, §20): their
//! CREATE and ALTER made again from the log, their DROP too, and the
//! descriptors taken whole in a checkpoint's snapshot, valid or INVALID,
//! with the privileges granted on them.

use turborel::db::Database;

use crate::common::Scratch;

use super::{BANK, BANK_SCHEMA, rows};

/// What the Information Schema shows of every graph to its owner and to
/// B: their names, validity and definitions, labels, and B's privileges.
fn shown(db: &Database) -> Vec<Vec<String>> {
    let a = db.session("A");
    vec![
        rows(&a, "SELECT PROPERTY_GRAPH_NAME, IS_VALID, PROPERTY_GRAPH_DEFINITION FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS ORDER BY 1"),
        rows(&a, "SELECT PROPERTY_GRAPH_NAME, ELEMENT_TABLE_ALIAS, LABEL_NAME FROM INFORMATION_SCHEMA.PG_LABELS ORDER BY 1, 2, 3"),
        rows(&db.session("B"), "SELECT GRANTOR, GRANTEE, PROPERTY_GRAPH_NAME FROM INFORMATION_SCHEMA.PROPERTY_GRAPH_PRIVILEGES ORDER BY 1"),
    ]
}

/// Verifies: [EXTENSION] §4.1 R4.7, §20: graphs, altered, INVALID or
/// valid, and their grants, are as they were after reopening without a
/// checkpoint, made again from the log, and after a checkpoint, from its
/// snapshot; a graph dropped stays dropped.
#[test]
fn graphs_are_kept_across_reopening_4_1() {
    let scratch = Scratch::new("graph-ddl-reopen");
    let dir = scratch.path("db");
    let before = {
        let db = Database::open(&dir).unwrap();
        db.schema(BANK_SCHEMA).unwrap();
        let a = db.session("A");
        a.run("CREATE TABLE PLAIN (K INTEGER NOT NULL, CONSTRAINT PK PRIMARY KEY (K))").unwrap();
        a.run(BANK).unwrap();
        a.run("ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE ACCOUNT ADD LABEL PERSON PROPERTIES (UPPER(OWNER) AS LOUD)").unwrap();
        a.run("CREATE PROPERTY GRAPH BROKEN VERTEX TABLES (PLAIN)").unwrap();
        a.run("CREATE PROPERTY GRAPH GONE VERTEX TABLES (CITY) OPTIONS (TRUSTED MODE) WITH INVOKER RIGHTS").unwrap();
        a.run("GRANT SELECT ON PROPERTY GRAPH BANK TO B").unwrap();
        a.commit().unwrap();
        a.run("ALTER TABLE PLAIN DROP CONSTRAINT PK CASCADE").unwrap();
        a.run("DROP PROPERTY GRAPH GONE").unwrap();
        a.commit().unwrap();
        let before = shown(&db);
        assert_eq!(before[0].len(), 2, "{before:?}");
        assert!(before[0][1].starts_with("BROKEN,NO,"), "{before:?}");
        assert_eq!(before[2], ["A,B,BANK"]);
        before
    };
    {
        let db = Database::open(&dir).unwrap();
        assert_eq!(shown(&db), before);
        db.checkpoint().unwrap();
        db.close().unwrap();
    }
    let db = Database::open(&dir).unwrap();
    assert_eq!(shown(&db), before);
    let a = db.session("A");
    a.run("ALTER TABLE PLAIN ADD CONSTRAINT PK PRIMARY KEY (K)").unwrap();
    a.run("ALTER PROPERTY GRAPH BROKEN COMPILE").unwrap();
    a.commit().unwrap();
    db.close().unwrap();
}
