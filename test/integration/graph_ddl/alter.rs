//! ALTER PROPERTY GRAPH (§4.1): element tables, labels and properties
//! added and dropped, each change checked by every rule as CREATE checks
//! it, and COMPILE.

use super::{BANK, bank, refused_with, rows};

/// The labels of each element table of the one graph, in order.
const LABELS: &str = "SELECT ELEMENT_TABLE_ALIAS, LABEL_NAME FROM INFORMATION_SCHEMA.PG_LABELS ORDER BY 1, ORDINAL_POSITION";

/// The properties of each label of the one graph, in order.
const PROPERTIES: &str = "SELECT ELEMENT_TABLE_ALIAS, LABEL_NAME, PROPERTY_NAME FROM INFORMATION_SCHEMA.PG_PROPERTIES \
                          ORDER BY 1, 2, ORDINAL_POSITION";

/// Verifies: [EXTENSION] §4.1: ADD VERTEX TABLES and ADD EDGE TABLES add
/// element tables, by the rules of CREATE, an edge's vertex tables among
/// the graph's; DROP EDGE TABLES drops one; DROP VERTEX TABLES of one an
/// edge table references is refused, or with CASCADE drops the edge table
/// too; the last vertex table is not dropped; GRF-0012 of an alias the
/// graph has not.
#[test]
fn element_tables_are_added_and_dropped_4_1() {
    let db = bank();
    let a = db.session("A");
    a.run("CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT)").unwrap();
    a.run("ALTER PROPERTY GRAPH G ADD VERTEX TABLES (CITY, PHONE AS MOBILE)").unwrap();
    a.commit().unwrap();
    refused_with(&a, "ALTER PROPERTY GRAPH G ADD VERTEX TABLES (CITY)", "alias CITY");
    a.run("ALTER PROPERTY GRAPH G ADD EDGE TABLES (LOCATED_IN SOURCE ACCOUNT DESTINATION CITY)").unwrap();
    a.run("ALTER PROPERTY GRAPH G ADD EDGE TABLES (HAS_PHONE SOURCE ACCOUNT DESTINATION MOBILE)").unwrap();
    a.commit().unwrap();
    let tables = "SELECT ELEMENT_TABLE_KIND, ELEMENT_TABLE_ALIAS FROM INFORMATION_SCHEMA.PG_ELEMENT_TABLES ORDER BY 1, ORDINAL_POSITION";
    assert_eq!(rows(&a, tables), ["EDGE,LOCATED_IN", "EDGE,HAS_PHONE", "VERTEX,ACCOUNT", "VERTEX,CITY", "VERTEX,MOBILE"]);
    refused_with(&a, "ALTER PROPERTY GRAPH G DROP VERTEX TABLES (CITY)", "edge table LOCATED_IN");
    refused_with(&a, "ALTER PROPERTY GRAPH G DROP VERTEX TABLES (CITY) RESTRICT", "CASCADE");
    refused_with(&a, "ALTER PROPERTY GRAPH G DROP VERTEX TABLES (NOPE)", "GRF-0012");
    refused_with(&a, "ALTER PROPERTY GRAPH G DROP EDGE TABLES (CITY)", "GRF-0012");
    a.run("ALTER PROPERTY GRAPH G DROP EDGE TABLES (HAS_PHONE)").unwrap();
    a.run("ALTER PROPERTY GRAPH G DROP VERTEX TABLES (CITY) CASCADE").unwrap();
    assert_eq!(rows(&a, tables), ["VERTEX,ACCOUNT", "VERTEX,MOBILE"]);
    a.run("ALTER PROPERTY GRAPH G DROP VERTEX TABLES (MOBILE)").unwrap();
    refused_with(&a, "ALTER PROPERTY GRAPH G DROP VERTEX TABLES (ACCOUNT)", "one vertex table at least");
    refused_with(&a, "ALTER PROPERTY GRAPH NOPE COMPILE", "GRF-0001");
}

/// Verifies: [EXTENSION] §4.1 R4.4, R4.5: ALTER VERTEX TABLE and ALTER
/// EDGE TABLE add a label, with its properties or all columns; drop one,
/// but the last; add and drop properties of a label, a label of no
/// properties left with none; GRF-0012 of a label or property not there;
/// a label shared stays of one property set.
#[test]
fn labels_and_properties_are_altered_4_1() {
    let db = bank();
    let a = db.session("A");
    a.run(BANK).unwrap();
    a.commit().unwrap();
    a.run("ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE ACCOUNT ADD LABEL PERSON PROPERTIES (OWNER)").unwrap();
    a.run("ALTER PROPERTY GRAPH BANK ALTER EDGE TABLE LOCATED_IN ADD LABEL LIVES").unwrap();
    a.run("ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE CITY DROP LABEL CITY").unwrap();
    a.run("ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE ACCOUNT ALTER LABEL ACCOUNT ADD PROPERTIES (ACCT_ID AS ID, UPPER(OWNER) AS LOUD)")
        .unwrap();
    a.run("ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE ACCOUNT ALTER LABEL ACCOUNT DROP PROPERTIES (IS_BLOCKED, ID)").unwrap();
    a.run("ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE ACCOUNT ALTER LABEL PERSON DROP PROPERTIES (OWNER) CASCADE").unwrap();
    a.commit().unwrap();
    let labels = rows(&a, LABELS);
    assert_eq!(
        labels,
        [
            "ACCOUNT,ACCOUNT",
            "ACCOUNT,PERSON",
            "CITY,PLACE",
            "HAS_PHONE,HAS_PHONE",
            "LOCATED_IN,IS_LOCATED_IN",
            "LOCATED_IN,LIVES",
            "PHONE,PHONE",
            "TRANSFER,TRANSFER"
        ]
    );
    let properties = rows(&a, PROPERTIES);
    assert!(properties.contains(&"ACCOUNT,ACCOUNT,LOUD".to_owned()), "{properties:?}");
    assert!(properties.contains(&"LOCATED_IN,LIVES,CITY_ID".to_owned()), "{properties:?}");
    assert!(!properties.iter().any(|p| p.starts_with("ACCOUNT,PERSON")), "{properties:?}");
    refused_with(&a, "ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE CITY DROP LABEL PLACE", "one label at least");
    refused_with(&a, "ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE CITY DROP LABEL NOPE", "GRF-0012");
    refused_with(&a, "ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE NOPE DROP LABEL PLACE", "GRF-0012");
    refused_with(&a, "ALTER PROPERTY GRAPH BANK ALTER EDGE TABLE CITY DROP LABEL PLACE", "GRF-0012");
    refused_with(&a, "ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE ACCOUNT ALTER LABEL ACCOUNT DROP PROPERTIES (NOPE)", "GRF-0012");
    refused_with(&a, "ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE ACCOUNT ALTER LABEL PERSON DROP PROPERTIES (OWNER)", "GRF-0012");
    refused_with(&a, "ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE ACCOUNT ALTER LABEL NOPE ADD PROPERTIES (OWNER)", "GRF-0012");
    refused_with(&a, "ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE ACCOUNT ADD LABEL ACCOUNT", "label ACCOUNT twice");
    refused_with(&a, "ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE PHONE ADD LABEL PLACE PROPERTIES (DIGITS)", "GRF-0011");
    a.run("ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE PHONE ADD LABEL PLACE PROPERTIES (DIGITS AS CITY_NAME)").unwrap();
    a.run("ALTER PROPERTY GRAPH BANK ALTER VERTEX TABLE ACCOUNT ALTER LABEL PERSON ADD PROPERTIES (OWNER)").unwrap();
    a.run("ALTER PROPERTY GRAPH BANK COMPILE").unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.PG_LABELS WHERE LABEL_NAME = 'PLACE'"), ["2"]);
}
