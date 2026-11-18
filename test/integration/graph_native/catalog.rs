//! The Information Schema's views of node and edge tables (§4.2): their
//! hidden columns, GRAPH_TABLES and CONNECTION_CONSTRAINTS, the adjacency
//! index of each edge table, and the implicit graph SYS_GRAPH.

use super::{rows, social};

/// Verifies: [EXTENSION] §4.2: COLUMNS shows each pseudo-column last,
/// hidden, of the identity's type; GRAPH_TABLES each node and edge table;
/// CONNECTION_CONSTRAINTS each clause; TABLE_CONSTRAINTS a CONNECTION
/// constraint; INDEXES each table's own indexes, an edge table's
/// adjacency index among them.
#[test]
fn node_and_edge_tables_are_shown_4_2() {
    let db = social();
    let a = db.session("A");
    let columns = "SELECT COLUMN_NAME, IS_HIDDEN, DATA_TYPE, IS_NULLABLE FROM INFORMATION_SCHEMA.COLUMNS \
                   WHERE TABLE_NAME = 'KNOWS' ORDER BY ORDINAL_POSITION";
    assert_eq!(
        rows(&a, columns),
        [
            "SINCE,NO,INTEGER,YES",
            "$EDGE_ID,YES,CHARACTER VARYING,NO",
            "$FROM_ID,YES,CHARACTER VARYING,NO",
            "$TO_ID,YES,CHARACTER VARYING,NO"
        ]
    );
    assert_eq!(
        rows(&a, "SELECT TABLE_NAME, GRAPH_TABLE_TYPE FROM INFORMATION_SCHEMA.GRAPH_TABLES ORDER BY 1"),
        ["CITY,NODE", "KNOWS,EDGE", "LIVES_IN,EDGE", "PERSON,NODE"]
    );
    let connections = "SELECT CONSTRAINT_NAME, TABLE_NAME, ORDINAL_POSITION, FROM_TABLE_NAME, TO_TABLE_NAME, DELETE_RULE \
                       FROM INFORMATION_SCHEMA.CONNECTION_CONSTRAINTS";
    assert_eq!(rows(&a, connections), ["LIVES,LIVES_IN,1,PERSON,CITY,CASCADE"]);
    let kinds = "SELECT CONSTRAINT_TYPE FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE CONSTRAINT_NAME = 'LIVES'";
    assert_eq!(rows(&a, kinds), ["CONNECTION"]);
    let indexes = "SELECT INDEX_NAME, METHOD_NAME FROM INFORMATION_SCHEMA.INDEXES WHERE TABLE_NAME = 'KNOWS' ORDER BY 1";
    assert_eq!(rows(&a, indexes), ["A.KNOWS.$EDGE_ID,btree", "A.KNOWS.ADJACENCY,adjacency"]);
    let b = db.session("B");
    assert_eq!(rows(&b, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.GRAPH_TABLES"), ["0"]);
}

/// Verifies: [EXTENSION] §4.2: SYS_GRAPH has each node table as a vertex
/// table keyed by `$NODE_ID`, labelled by its name, its own columns its
/// properties; each edge table once for each pair of node tables it may
/// join, a CONNECTION constraint's pairs alone; it follows tables
/// created, altered and dropped, and goes with the last node table.
#[test]
fn sys_graph_follows_the_tables_4_2() {
    let db = social();
    let a = db.session("A");
    let elements = "SELECT ELEMENT_TABLE_KIND, ELEMENT_TABLE_ALIAS, TABLE_NAME FROM INFORMATION_SCHEMA.PG_ELEMENT_TABLES \
                    WHERE PROPERTY_GRAPH_NAME = 'SYS_GRAPH' ORDER BY 1, 2";
    assert_eq!(
        rows(&a, elements),
        [
            "EDGE,KNOWS CITY TO CITY,KNOWS",
            "EDGE,KNOWS CITY TO PERSON,KNOWS",
            "EDGE,KNOWS PERSON TO CITY,KNOWS",
            "EDGE,KNOWS PERSON TO PERSON,KNOWS",
            "EDGE,LIVES_IN,LIVES_IN",
            "VERTEX,CITY,CITY",
            "VERTEX,PERSON,PERSON"
        ]
    );
    let properties = "SELECT ELEMENT_TABLE_ALIAS, LABEL_NAME, PROPERTY_NAME FROM INFORMATION_SCHEMA.PG_PROPERTIES \
                      WHERE PROPERTY_GRAPH_NAME = 'SYS_GRAPH' AND ELEMENT_TABLE_ALIAS IN ('PERSON', 'LIVES_IN') ORDER BY 1, 3";
    assert_eq!(rows(&a, properties), ["PERSON,PERSON,AGE", "PERSON,PERSON,NAME"]);
    let keys = "SELECT ELEMENT_TABLE_ALIAS, COLUMN_NAME FROM INFORMATION_SCHEMA.PG_KEY_COLUMNS \
                WHERE PROPERTY_GRAPH_NAME = 'SYS_GRAPH' AND ELEMENT_TABLE_ALIAS IN ('PERSON', 'LIVES_IN') ORDER BY 1";
    assert_eq!(rows(&a, keys), ["LIVES_IN,$EDGE_ID", "PERSON,$NODE_ID"]);
    let ends = "SELECT EDGE_END, VERTEX_TABLE_ALIAS, EDGE_COLUMN_NAME, VERTEX_COLUMN_NAME FROM INFORMATION_SCHEMA.PG_EDGE_ENDPOINTS \
                WHERE PROPERTY_GRAPH_NAME = 'SYS_GRAPH' AND EDGE_TABLE_ALIAS = 'LIVES_IN' ORDER BY 1";
    assert_eq!(rows(&a, ends), ["DESTINATION,CITY,$TO_ID,$NODE_ID", "SOURCE,PERSON,$FROM_ID,$NODE_ID"]);
    a.run("ALTER TABLE PERSON ADD COLUMN NICK VARCHAR(10)").unwrap();
    a.run("ALTER TABLE KNOWS ADD CONSTRAINT K CONNECTION (PERSON TO PERSON)").unwrap();
    assert_eq!(rows(&a, properties), ["PERSON,PERSON,AGE", "PERSON,PERSON,NAME", "PERSON,PERSON,NICK"]);
    assert_eq!(
        rows(&a, &elements.replace("ORDER BY 1, 2", "AND ELEMENT_TABLE_KIND = 'EDGE' ORDER BY 1, 2")),
        ["EDGE,KNOWS,KNOWS", "EDGE,LIVES_IN,LIVES_IN"]
    );
    a.run("DROP TABLE KNOWS RESTRICT").unwrap();
    a.run("DROP TABLE LIVES_IN RESTRICT").unwrap();
    a.run("DROP TABLE CITY RESTRICT").unwrap();
    assert_eq!(rows(&a, elements), ["VERTEX,PERSON,PERSON"]);
    a.run("DROP TABLE PERSON RESTRICT").unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS"), ["0"]);
    a.commit().unwrap();
}
