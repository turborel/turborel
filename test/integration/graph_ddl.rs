//! Property graphs over tables (RULES.md D-GRAPH-CATALOG), Mode A of the
//! property graph implementation standard (§4.1): CREATE PROPERTY GRAPH
//! with its keys, endpoints, labels and properties found or written, each
//! rule of §4.1 and its conditions of §22, and the Information Schema's
//! views of graphs (§20). The modules below hold ALTER PROPERTY GRAPH,
//! what other statements do to graphs, their privileges, and graphs kept
//! across reopening.

use turborel::db::{Database, Session};
use turborel::error::Code;

mod alter;
mod catalog;
mod dependents;
mod privileges;
mod recovery;

/// The bank of the standard's example (§4.1): schema A of accounts,
/// cities, phones, transfers between accounts, where each account is and
/// its phones, and a table of notes of no key.
const BANK_SCHEMA: &str = "CREATE SCHEMA AUTHORIZATION A \
     CREATE TABLE CITY (CITY_ID INTEGER PRIMARY KEY, CITY_NAME VARCHAR(20)) \
     CREATE TABLE ACCOUNT (ACCT_ID INTEGER PRIMARY KEY, OWNER VARCHAR(20), IS_BLOCKED BOOLEAN) \
     CREATE TABLE PHONE (PHONE_ID INTEGER PRIMARY KEY, DIGITS VARCHAR(20)) \
     CREATE TABLE TRANSFER (TXN_ID INTEGER PRIMARY KEY, FROM_ACCT INTEGER REFERENCES ACCOUNT, \
         TO_ACCT INTEGER REFERENCES ACCOUNT, AMOUNT DECIMAL(10, 2)) \
     CREATE TABLE LOCATED_IN (ACCT_ID INTEGER PRIMARY KEY REFERENCES ACCOUNT, CITY_ID INTEGER REFERENCES CITY) \
     CREATE TABLE HAS_PHONE (ACCT_ID INTEGER NOT NULL REFERENCES ACCOUNT, PHONE_ID INTEGER NOT NULL REFERENCES PHONE, \
         PRIMARY KEY (ACCT_ID, PHONE_ID)) \
     CREATE TABLE NOTES (NOTE VARCHAR(20))";

/// A database of `BANK_SCHEMA`.
fn bank() -> Database {
    let db = Database::new();
    db.schema(BANK_SCHEMA).unwrap();
    db
}

/// The graph BANK over `bank`'s tables, as the standard's example writes
/// it (§4.1).
const BANK: &str = "CREATE PROPERTY GRAPH BANK \
     VERTEX TABLES ( \
       ACCOUNT LABEL ACCOUNT PROPERTIES (OWNER, IS_BLOCKED), \
       CITY KEY (CITY_ID) LABEL CITY LABEL PLACE PROPERTIES (CITY_NAME), \
       PHONE DEFAULT LABEL PROPERTIES ARE ALL COLUMNS) \
     EDGE TABLES ( \
       TRANSFER KEY (TXN_ID) \
         SOURCE KEY (FROM_ACCT) REFERENCES ACCOUNT (ACCT_ID) \
         DESTINATION KEY (TO_ACCT) REFERENCES ACCOUNT (ACCT_ID) \
         LABEL TRANSFER PROPERTIES (AMOUNT, AMOUNT * 100 AS CENTS), \
       LOCATED_IN SOURCE ACCOUNT DESTINATION CITY LABEL IS_LOCATED_IN NO PROPERTIES, \
       HAS_PHONE UNDIRECTED SOURCE KEY (ACCT_ID) REFERENCES ACCOUNT \
         DESTINATION KEY (PHONE_ID) REFERENCES PHONE LABEL HAS_PHONE)";

/// The values of `query` in `s`, each row its values joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// The SQLCODE of the error `sql` raises in `s`, and its message, the
/// transaction rolled back after.
fn refusal(s: &Session, sql: &str) -> (i32, String) {
    let e = s.run(sql).err().unwrap_or_else(|| panic!("{sql} succeeded"));
    s.rollback().unwrap();
    (e.sqlcode(), e.msg)
}

/// That `sql` is refused in `s` with SQLCODE -103 and a message that
/// holds `holds`.
fn refused_with(s: &Session, sql: &str, holds: &str) {
    let (code, msg) = refusal(s, sql);
    assert_eq!(code, -103, "{sql}: {msg}");
    assert!(msg.contains(holds), "{sql}: {msg} lacks {holds}");
}

/// Verifies: [EXTENSION] §4.1 R4.2, R4.5, R4.6, §20: the standard's bank
/// graph is created; each element table, label, property, key column and
/// endpoint column is in its view of the Information Schema, a key and
/// an endpoint not written found from the PRIMARY KEY and the one FOREIGN
/// KEY, DEFAULT LABEL the table's name, ALL COLUMNS each column, and a
/// label without properties all columns.
#[test]
fn the_bank_graph_is_described_4_1() {
    let db = bank();
    let a = db.session("A");
    a.run(BANK).unwrap();
    a.commit().unwrap();
    let from = |view: &str, columns: &str| {
        rows(&a, &format!("SELECT {columns} FROM INFORMATION_SCHEMA.{view} WHERE PROPERTY_GRAPH_NAME = 'BANK' ORDER BY {columns}"))
    };
    assert_eq!(
        from("PG_ELEMENT_TABLES", "ELEMENT_TABLE_KIND, ORDINAL_POSITION, ELEMENT_TABLE_ALIAS, TABLE_NAME, IS_UNDIRECTED"),
        [
            "EDGE,1,TRANSFER,TRANSFER,NO",
            "EDGE,2,LOCATED_IN,LOCATED_IN,NO",
            "EDGE,3,HAS_PHONE,HAS_PHONE,YES",
            "VERTEX,1,ACCOUNT,ACCOUNT,NO",
            "VERTEX,2,CITY,CITY,NO",
            "VERTEX,3,PHONE,PHONE,NO",
        ]
    );
    assert_eq!(
        from("PG_LABELS", "ELEMENT_TABLE_ALIAS, ORDINAL_POSITION, LABEL_NAME"),
        [
            "ACCOUNT,1,ACCOUNT",
            "CITY,1,CITY",
            "CITY,2,PLACE",
            "HAS_PHONE,1,HAS_PHONE",
            "LOCATED_IN,1,IS_LOCATED_IN",
            "PHONE,1,PHONE",
            "TRANSFER,1,TRANSFER"
        ]
    );
    assert_eq!(
        from("PG_PROPERTIES", "LABEL_NAME, ORDINAL_POSITION, PROPERTY_NAME, PROPERTY_EXPRESSION, DATA_TYPE"),
        [
            "ACCOUNT,1,OWNER,OWNER,CHARACTER VARYING(20)",
            "ACCOUNT,2,IS_BLOCKED,IS_BLOCKED,BOOLEAN",
            "CITY,1,CITY_ID,CITY_ID,INTEGER",
            "CITY,2,CITY_NAME,CITY_NAME,CHARACTER VARYING(20)",
            "HAS_PHONE,1,ACCT_ID,ACCT_ID,INTEGER",
            "HAS_PHONE,2,PHONE_ID,PHONE_ID,INTEGER",
            "PHONE,1,PHONE_ID,PHONE_ID,INTEGER",
            "PHONE,2,DIGITS,DIGITS,CHARACTER VARYING(20)",
            "PLACE,1,CITY_NAME,CITY_NAME,CHARACTER VARYING(20)",
            "TRANSFER,1,AMOUNT,AMOUNT,DECIMAL(10,2)",
            "TRANSFER,2,CENTS,AMOUNT * 100,NUMERIC(18,2)",
        ]
    );
    assert_eq!(from("PG_KEY_COLUMNS", "ELEMENT_TABLE_ALIAS, ORDINAL_POSITION, COLUMN_NAME").len(), 7);
    assert_eq!(rows(&a, "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.PG_KEY_COLUMNS WHERE ELEMENT_TABLE_ALIAS = 'LOCATED_IN'"), ["ACCT_ID"]);
    assert_eq!(
        from("PG_EDGE_ENDPOINTS", "EDGE_TABLE_ALIAS, EDGE_END, ORDINAL_POSITION, VERTEX_TABLE_ALIAS, EDGE_COLUMN_NAME, VERTEX_COLUMN_NAME"),
        [
            "HAS_PHONE,DESTINATION,1,PHONE,PHONE_ID,PHONE_ID",
            "HAS_PHONE,SOURCE,1,ACCOUNT,ACCT_ID,ACCT_ID",
            "LOCATED_IN,DESTINATION,1,CITY,CITY_ID,CITY_ID",
            "LOCATED_IN,SOURCE,1,ACCOUNT,ACCT_ID,ACCT_ID",
            "TRANSFER,DESTINATION,1,ACCOUNT,TO_ACCT,ACCT_ID",
            "TRANSFER,SOURCE,1,ACCOUNT,FROM_ACCT,ACCT_ID",
        ]
    );
    assert_eq!(
        rows(
            &a,
            "SELECT PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, PROPERTY_GRAPH_MODE, IS_INVOKER_RIGHTS, IS_VALID FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS"
        ),
        ["A,BANK,ENFORCED,NO,YES"]
    );
}

/// Verifies: [EXTENSION] §20 (DDL text retrieval): PROPERTY_GRAPH_DEFINITION
/// is a CREATE PROPERTY GRAPH statement of the graph as it is, every key,
/// endpoint, label and property written, which defines the same graph
/// when run again.
#[test]
fn the_definition_text_defines_the_graph_again_20() {
    let db = bank();
    let a = db.session("A");
    a.run(BANK).unwrap();
    let definition = "SELECT PROPERTY_GRAPH_DEFINITION FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS";
    let text = rows(&a, definition).remove(0);
    assert_eq!(
        text,
        "CREATE PROPERTY GRAPH A.BANK VERTEX TABLES (A.ACCOUNT KEY (ACCT_ID) LABEL ACCOUNT PROPERTIES (OWNER, IS_BLOCKED), \
         A.CITY KEY (CITY_ID) LABEL CITY PROPERTIES (CITY_ID, CITY_NAME) LABEL PLACE PROPERTIES (CITY_NAME), \
         A.PHONE KEY (PHONE_ID) LABEL PHONE PROPERTIES (PHONE_ID, DIGITS)) EDGE TABLES (A.TRANSFER KEY (TXN_ID) \
         SOURCE KEY (FROM_ACCT) REFERENCES ACCOUNT (ACCT_ID) DESTINATION KEY (TO_ACCT) REFERENCES ACCOUNT (ACCT_ID) \
         LABEL TRANSFER PROPERTIES (AMOUNT, AMOUNT * 100 AS CENTS), A.LOCATED_IN KEY (ACCT_ID) SOURCE KEY (ACCT_ID) \
         REFERENCES ACCOUNT (ACCT_ID) DESTINATION KEY (CITY_ID) REFERENCES CITY (CITY_ID) LABEL IS_LOCATED_IN NO PROPERTIES, \
         A.HAS_PHONE KEY (ACCT_ID, PHONE_ID) UNDIRECTED SOURCE KEY (ACCT_ID) REFERENCES ACCOUNT (ACCT_ID) \
         DESTINATION KEY (PHONE_ID) REFERENCES PHONE (PHONE_ID) LABEL HAS_PHONE PROPERTIES (ACCT_ID, PHONE_ID)) \
         OPTIONS (ENFORCED MODE)"
    );
    a.run("DROP PROPERTY GRAPH BANK").unwrap();
    a.run(&text).unwrap();
    assert_eq!(rows(&a, definition), [text]);
}

/// Verifies: [EXTENSION] §4.1 R4.1, R4.2, §22 GRF-0002, GRF-0003, GRF-0004:
/// an element table is a base table or view there is; a key not written
/// is found from a PRIMARY KEY or the one UNIQUE key of NOT NULL columns;
/// an endpoint not written from the one FOREIGN KEY to its vertex table;
/// and it names a vertex table of the graph.
#[test]
fn keys_and_endpoints_are_found_or_refused_4_1() {
    let db = bank();
    let a = db.session("A");
    refused_with(&a, "CREATE PROPERTY GRAPH G VERTEX TABLES (NOWHERE)", "GRF-0002");
    refused_with(&a, "CREATE PROPERTY GRAPH G VERTEX TABLES (INFORMATION_SCHEMA.TABLES)", "GRF-0002");
    refused_with(&a, "CREATE PROPERTY GRAPH G VERTEX TABLES (NOTES)", "GRF-0003");
    a.run("CREATE TABLE PAIRED (X INTEGER NOT NULL UNIQUE, Y INTEGER UNIQUE)").unwrap();
    a.run("CREATE PROPERTY GRAPH G VERTEX TABLES (PAIRED)").unwrap();
    assert_eq!(rows(&a, "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.PG_KEY_COLUMNS"), ["X"]);
    a.rollback().unwrap();
    a.run("CREATE TABLE TWICE (X INTEGER NOT NULL UNIQUE, Y INTEGER NOT NULL UNIQUE)").unwrap();
    refused_with(&a, "CREATE PROPERTY GRAPH G VERTEX TABLES (TWICE)", "GRF-0003");
    let transfer = |source: &str| {
        format!(
            "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT) EDGE TABLES (TRANSFER SOURCE {source} DESTINATION KEY (TO_ACCT) REFERENCES ACCOUNT)"
        )
    };
    refused_with(&a, &transfer("ACCOUNT"), "GRF-0003");
    refused_with(&a, &transfer("KEY (FROM_ACCT) REFERENCES CITY"), "GRF-0004");
    refused_with(&a, &transfer("KEY (FROM_ACCT, TXN_ID) REFERENCES ACCOUNT"), "REFERENCES 1");
    refused_with(&a, &transfer("KEY (NOPE) REFERENCES ACCOUNT"), "NOPE");
    a.run(&transfer("KEY (FROM_ACCT) REFERENCES ACCOUNT")).unwrap();
    a.rollback().unwrap();
    refused_with(
        &a,
        "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT) EDGE TABLES (LOCATED_IN SOURCE ACCOUNT DESTINATION LOCATED_IN)",
        "GRF-0004",
    );
    refused_with(&a, "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT KEY (ACCT_ID, ACCT_ID))", "twice");
    refused_with(&a, "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT, ACCOUNT)", "alias ACCOUNT");
    a.run("CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT, ACCOUNT AS OTHER)").unwrap();
}

/// Verifies: [EXTENSION] §4.1 R4.3: of ENFORCED MODE, the default, a key
/// written is a PRIMARY KEY or UNIQUE key of NOT NULL columns and an
/// endpoint a FOREIGN KEY, so a view, which declares none, is no element
/// table of it; TRUSTED MODE takes them as written, a view too.
#[test]
fn enforced_mode_needs_declared_keys_4_1() {
    let db = bank();
    let a = db.session("A");
    a.run("CREATE VIEW RICH AS SELECT ACCT_ID, OWNER FROM ACCOUNT WHERE NOT IS_BLOCKED").unwrap();
    a.commit().unwrap();
    refused_with(&a, "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT KEY (OWNER))", "ENFORCED MODE");
    refused_with(&a, "CREATE PROPERTY GRAPH G VERTEX TABLES (RICH KEY (ACCT_ID))", "ENFORCED MODE");
    let edge = "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT) \
                EDGE TABLES (TRANSFER SOURCE KEY (TXN_ID) REFERENCES ACCOUNT DESTINATION KEY (TO_ACCT) REFERENCES ACCOUNT)";
    refused_with(&a, edge, "FOREIGN KEY");
    a.run(&format!("{edge} OPTIONS (TRUSTED MODE)")).unwrap();
    a.run("CREATE PROPERTY GRAPH H VERTEX TABLES (RICH KEY (ACCT_ID), ACCOUNT KEY (OWNER)) OPTIONS (TRUSTED MODE)").unwrap();
    a.commit().unwrap();
    refused_with(&a, "CREATE PROPERTY GRAPH J VERTEX TABLES (RICH) OPTIONS (TRUSTED MODE)", "GRF-0003");
    assert_eq!(
        rows(&a, "SELECT PROPERTY_GRAPH_NAME, PROPERTY_GRAPH_MODE FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS ORDER BY 1"),
        ["G,TRUSTED", "H,TRUSTED"]
    );
}

/// Verifies: [EXTENSION] §4.1 R4.4, R4.5, R5.6, §22 GRF-0011: a label on
/// several element tables has the same property names on each, of types
/// a union takes; a property of two labels of one table has one
/// expression; a label or property name is not repeated; an expression is
/// of one row, so no set function, and is named with AS unless a column.
#[test]
fn labels_and_properties_follow_the_rules_4_1() {
    let db = bank();
    let a = db.session("A");
    let shared = |account: &str, phone: &str| {
        format!(
            "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT LABEL THING PROPERTIES ({account}), PHONE LABEL THING PROPERTIES ({phone}))"
        )
    };
    refused_with(&a, &shared("OWNER", "DIGITS"), "GRF-0011");
    refused_with(&a, &shared("IS_BLOCKED AS X", "DIGITS AS X"), "GRF-0011");
    a.run(&shared("OWNER AS X, ACCT_ID AS K", "DIGITS AS X, PHONE_ID AS K")).unwrap();
    a.rollback().unwrap();
    refused_with(
        &a,
        "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT LABEL L PROPERTIES (OWNER AS X) LABEL M PROPERTIES (ACCT_ID AS X))",
        "two expressions",
    );
    refused_with(&a, "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT LABEL L LABEL L)", "label L twice");
    refused_with(&a, "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT LABEL L PROPERTIES (OWNER, ACCT_ID AS OWNER))", "OWNER twice");
    refused_with(&a, "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT PROPERTIES (ACCT_ID + 1))", "AS");
    refused_with(&a, "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT PROPERTIES (COUNT(*) AS N))", "set function");
    refused_with(&a, "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT PROPERTIES (NOPE))", "NOPE");
    refused_with(&a, "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT PROPERTIES ARE ALL COLUMNS EXCEPT (NOPE))", "EXCEPT names NOPE");
    a.run(
        "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT PROPERTIES ARE ALL COLUMNS EXCEPT (IS_BLOCKED), \
         CITY PROPERTIES ((SELECT COUNT(*) FROM LOCATED_IN AS L WHERE L.CITY_ID = CITY.CITY_ID) AS PEOPLE))",
    )
    .unwrap();
    let properties = "SELECT ELEMENT_TABLE_ALIAS, LABEL_NAME, PROPERTY_NAME FROM INFORMATION_SCHEMA.PG_PROPERTIES ORDER BY 1, 3";
    assert_eq!(rows(&a, properties), ["ACCOUNT,ACCOUNT,ACCT_ID", "ACCOUNT,ACCOUNT,OWNER", "CITY,CITY,PEOPLE"]);
}

/// Verifies: [EXTENSION] §4.1, R4.7: OR REPLACE replaces a graph, IF NOT
/// EXISTS leaves one there as it is, not both are written; a graph's name
/// is of the namespace of its schema's tables and views; DROP PROPERTY
/// GRAPH IF EXISTS of none does nothing, and of a graph there is not
/// raises GRF-0001; a graph is of a schema there is.
#[test]
fn a_graph_is_created_replaced_and_dropped_4_1() {
    let db = bank();
    let a = db.session("A");
    let one = "CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT)";
    a.run(one).unwrap();
    a.commit().unwrap();
    refused_with(&a, one, "property graph G");
    a.run("CREATE PROPERTY GRAPH IF NOT EXISTS G VERTEX TABLES (CITY)").unwrap();
    let tables = "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.PG_ELEMENT_TABLES";
    assert_eq!(rows(&a, tables), ["ACCOUNT"]);
    a.run("CREATE OR REPLACE PROPERTY GRAPH G VERTEX TABLES (CITY)").unwrap();
    assert_eq!(rows(&a, tables), ["CITY"]);
    let (code, _) = refusal(&a, "CREATE OR REPLACE PROPERTY GRAPH IF NOT EXISTS G VERTEX TABLES (CITY)");
    assert_eq!(code, -102);
    assert_eq!(rows(&a, tables), ["ACCOUNT"]);
    refused_with(&a, "CREATE PROPERTY GRAPH CITY VERTEX TABLES (ACCOUNT)", "one namespace");
    refused_with(&a, "CREATE TABLE G (X INTEGER)", "property graph G");
    refused_with(&a, "CREATE VIEW G AS SELECT * FROM CITY", "property graph G");
    refused_with(&a, "CREATE PROPERTY GRAPH NOBODY.G VERTEX TABLES (A.CITY)", "no schema NOBODY");
    a.run("DROP PROPERTY GRAPH G").unwrap();
    a.run("DROP PROPERTY GRAPH IF EXISTS G CASCADE").unwrap();
    assert_eq!(rows(&a, "SELECT * FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS"), Vec::<String>::new());
    a.commit().unwrap();
    refused_with(&a, "DROP PROPERTY GRAPH G", "GRF-0001");
    let e = a.run("CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT").unwrap_err();
    assert_eq!(e.code, Code::Syntax);
}

/// Verifies: [EXTENSION] §4.1, §20: a graph is an element of a `<schema
/// definition>`, made once its tables are, and SELECT on it granted there;
/// one named in another schema is refused.
#[test]
fn a_schema_definition_creates_graphs_4_1() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A \
         CREATE PROPERTY GRAPH G VERTEX TABLES (T) \
         CREATE TABLE T (K INTEGER PRIMARY KEY) \
         GRANT SELECT ON PROPERTY GRAPH G TO B",
    )
    .unwrap();
    let b = db.session("B");
    let graphs = "SELECT PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, PROPERTY_GRAPH_DEFINITION FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS";
    assert_eq!(rows(&b, graphs), ["A,G,NULL"]);
    let e = db
        .schema("CREATE SCHEMA AUTHORIZATION C CREATE TABLE T (K INTEGER PRIMARY KEY) CREATE PROPERTY GRAPH A.H VERTEX TABLES (T)")
        .unwrap_err();
    assert_eq!(e.sqlcode(), -103, "{e:?}");
}
