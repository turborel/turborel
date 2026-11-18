//! \[EXTENSION\] [GS §18] A property graph's name in place of the edges
//! of a function of GRAPH_ALGO: each vertex its identity, its element
//! table and key [GS §5 R5.1], of any number of vertex tables, keys of
//! any number of columns and endpoints that reference any columns; and a
//! schema's SYS_GRAPH, each vertex its `$NODE_ID`.

use turborel::db::{Database, Session};

use super::{graph, ordered, query, refused};

/// The rows of `sql`, each identity `{"table":"T","key":[k]}` written
/// `Tk`, so a row reads as the vertices it names.
fn named(s: &Session, sql: &str) -> Vec<String> {
    let short = |row: String| row.replace("{\"table\":\"", "").replace("\",\"key\":[", "").replace("]}", "");
    query(s, sql).into_iter().map(short).collect()
}

/// The identity of the vertex of element table `table` whose key is
/// `key`, as a literal.
fn identity(table: &str, key: &str) -> String {
    format!("'{{\"table\":\"{table}\",\"key\":[{key}]}}'")
}

/// Verifies: [EXTENSION] [GS §18] [GS §5 R5.1] A property graph's name in
/// place of the edges: its edge tables' endpoints are the edges, an
/// undirected one's both ways; its vertex tables' rows the vertices, each
/// its identity, so START is one too; COST a column every edge table has;
/// of another schema by its name. A value naming no graph, or VERTICES
/// with a graph, is refused (-103).
#[test]
fn property_graphs_by_name() {
    let db = graph();
    let s = db.session("G");
    let ends = "SOURCE KEY (SRC) REFERENCES V (ID) DESTINATION KEY (DST) REFERENCES V (ID)";
    s.run(&format!("CREATE PROPERTY GRAPH PG VERTEX TABLES (V) EDGE TABLES (E KEY (SRC, DST) {ends}) OPTIONS (TRUSTED MODE)")).unwrap();
    let undirected =
        format!("CREATE PROPERTY GRAPH UG VERTEX TABLES (V) EDGE TABLES (E KEY (SRC, DST) UNDIRECTED {ends}) OPTIONS (TRUSTED MODE)");
    s.run(&undirected).unwrap();
    assert_eq!(
        query(&s, "SELECT * FROM GRAPH_ALGO.WCC(PG) AS W WHERE VERTEX LIKE '%[5]%'"),
        ["{\"table\":\"V\",\"key\":[5]},{\"table\":\"V\",\"key\":[5]}"]
    );
    let all = |call: &str| named(&s, &format!("SELECT * FROM GRAPH_ALGO.{call} AS R ORDER BY 1"));
    assert_eq!(all("WCC(PG)"), ["V1,V1", "V2,V1", "V3,V1", "V4,V1", "V5,V5"]);
    assert_eq!(all("SCC(G.PG)"), ["V1,V1", "V2,V1", "V3,V1", "V4,V4", "V5,V5"]);
    assert_eq!(all(&format!("SSSP(PG, {}, COST => 'W')", identity("V", "1"))), ["V1,0E0", "V2,1E0", "V3,3E0", "V4,8E0"]);
    assert_eq!(all("SCC(UG)"), ["V1,V1", "V2,V1", "V3,V1", "V4,V1", "V5,V5"]);
    assert_eq!(all(&format!("BFS_LEVELS(UG, {})", identity("V", "4"))), ["V1,1", "V2,2", "V3,1", "V4,0"]);
    assert_eq!(ordered(&s, "WCC(TABLE (E))").len(), 4, "a table of edges is unchanged");
    assert_eq!(refused(&s, "SELECT * FROM GRAPH_ALGO.WCC(PG, VERTICES => TABLE (V)) AS C").0, -103);
    assert_eq!(refused(&s, "SELECT * FROM GRAPH_ALGO.WCC(NOSUCH) AS C").0, -103);
}

/// The bank of [GS §23.1] as tables of schema B: accounts a1 to a6,
/// places c1 and c2, phones p1 and p2, each table's keys from 1, so keys
/// of the three tables meet; the transfers t1 to t8, where each account
/// is, and its phones; and the graph BANK of them, `HAS_PHONE` undirected.
fn bank() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B \
         CREATE TABLE ACCOUNT (ACCT_ID INTEGER PRIMARY KEY, OWNER VARCHAR(10)) \
         CREATE TABLE PLACE (PLACE_ID INTEGER PRIMARY KEY, NAME VARCHAR(20)) \
         CREATE TABLE PHONE (PHONE_ID INTEGER PRIMARY KEY) \
         CREATE TABLE TRANSFER (TXN_ID INTEGER PRIMARY KEY, FROM_ACCT INTEGER, TO_ACCT INTEGER, AMOUNT INTEGER) \
         CREATE TABLE LOCATED_IN (ACCT_ID INTEGER PRIMARY KEY, PLACE_ID INTEGER) \
         CREATE TABLE HAS_PHONE (ACCT_ID INTEGER NOT NULL, PHONE_ID INTEGER NOT NULL, PRIMARY KEY (ACCT_ID, PHONE_ID))",
    )
    .unwrap();
    let s = db.session("B");
    for sql in [
        "INSERT INTO ACCOUNT VALUES (1, 'Scott'), (2, 'Aretha'), (3, 'Mike'), (4, 'Jay'), (5, 'Charles'), (6, 'Dave')",
        "INSERT INTO PLACE VALUES (1, 'Zembla'), (2, 'Ankh-Morpork')",
        "INSERT INTO PHONE VALUES (1), (2)",
        "INSERT INTO TRANSFER VALUES (1, 1, 3, 8), (2, 3, 2, 10), (3, 2, 4, 10), (4, 4, 6, 10), (5, 6, 3, 10), (6, 6, 5, 4), \
         (7, 3, 5, 6), (8, 5, 1, 9)",
        "INSERT INTO LOCATED_IN VALUES (1, 1), (2, 2), (3, 1), (4, 2), (5, 1), (6, 2)",
        "INSERT INTO HAS_PHONE VALUES (1, 1), (5, 1), (2, 2), (3, 2)",
        "CREATE PROPERTY GRAPH BANK VERTEX TABLES (ACCOUNT, PLACE, PHONE) EDGE TABLES ( \
           TRANSFER SOURCE KEY (FROM_ACCT) REFERENCES ACCOUNT (ACCT_ID) DESTINATION KEY (TO_ACCT) REFERENCES ACCOUNT (ACCT_ID), \
           LOCATED_IN SOURCE KEY (ACCT_ID) REFERENCES ACCOUNT (ACCT_ID) DESTINATION KEY (PLACE_ID) REFERENCES PLACE (PLACE_ID), \
           HAS_PHONE UNDIRECTED SOURCE KEY (ACCT_ID) REFERENCES ACCOUNT (ACCT_ID) \
             DESTINATION KEY (PHONE_ID) REFERENCES PHONE (PHONE_ID)) OPTIONS (TRUSTED MODE)",
    ] {
        s.run(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    s.commit().unwrap();
    db
}

/// Verifies: [EXTENSION] [GS §18] [GS §23.1] [GS §5 R5.1] The bank graph
/// of three vertex tables whose keys meet: each vertex is its table and
/// key, so its ten vertices stay ten; the accounts and phones are one
/// strong component through the cycles of transfers and the undirected
/// phones, each place its own; BFS_LEVELS from Dave reaches every vertex.
#[test]
fn several_vertex_tables_keep_their_vertices_apart() {
    let db = bank();
    let s = db.session("B");
    assert_eq!(query(&s, "SELECT COUNT(*), COUNT(DISTINCT COMPONENT) FROM GRAPH_ALGO.WCC(BANK) AS W"), ["10,1"]);
    assert_eq!(query(&s, "SELECT COUNT(*), COUNT(DISTINCT COMPONENT) FROM GRAPH_ALGO.SCC(BANK) AS W"), ["10,3"]);
    let levels = format!("SELECT * FROM GRAPH_ALGO.BFS_LEVELS(BANK, {}) AS L ORDER BY DEPTH, VERTEX", identity("ACCOUNT", "6"));
    assert_eq!(
        named(&s, &levels),
        [
            "ACCOUNT6,0",
            "ACCOUNT3,1",
            "ACCOUNT5,1",
            "PLACE2,1",
            "ACCOUNT1,2",
            "ACCOUNT2,2",
            "PHONE1,2",
            "PHONE2,2",
            "PLACE1,2",
            "ACCOUNT4,3"
        ]
    );
    let ranks = "SELECT CAST(SUM(RANK) * 1000 AS INTEGER) FROM GRAPH_ALGO.PAGERANK(BANK) AS R";
    assert_eq!(query(&s, ranks), ["1000"]);
}

/// Verifies: [EXTENSION] [GS §18] [GS §4.1 R4.2] Keys and endpoints of
/// two columns: each vertex's identity holds its key's values in order,
/// and an edge joins the vertices its two columns reference.
#[test]
fn keys_of_several_columns() {
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
    let all = |call: &str| named(&s, &format!("SELECT * FROM GRAPH_ALGO.{call} AS R ORDER BY 1"));
    assert_eq!(all("WCC(CG)"), ["R\"eu\",1,R\"eu\",1", "R\"eu\",2,R\"eu\",1", "R\"us\",1,R\"eu\",1", "R\"us\",2,R\"us\",2"]);
    assert_eq!(all(&format!("BFS_LEVELS(CG, {})", identity("R", "\"eu\",1"))), ["R\"eu\",1,0", "R\"eu\",2,1", "R\"us\",1,2"]);
}

/// Verifies: [EXTENSION] [GS §18] [GS §4.1 R4.2] An endpoint that
/// references columns other than its vertex table's key: an edge joins the
/// vertex whose referenced columns hold its own, named by its key, and an
/// edge referencing no vertex is none.
#[test]
fn endpoints_referencing_other_columns() {
    let db = graph();
    let s = db.session("G");
    s.run("CREATE TABLE ACC (ID INTEGER PRIMARY KEY, IBAN VARCHAR(10) NOT NULL UNIQUE)").unwrap();
    s.run("CREATE TABLE PAY (PNO INTEGER PRIMARY KEY, PAYER VARCHAR(10), PAYEE VARCHAR(10), PAID DOUBLE PRECISION)").unwrap();
    s.run("INSERT INTO ACC VALUES (1, 'IT01'), (2, 'DE02'), (3, 'FR03')").unwrap();
    s.run("INSERT INTO PAY VALUES (1, 'IT01', 'DE02', 5), (2, 'DE02', 'FR03', 7), (3, 'FR03', 'XX99', 1)").unwrap();
    s.run(
        "CREATE PROPERTY GRAPH PAYS VERTEX TABLES (ACC) EDGE TABLES (PAY SOURCE KEY (PAYER) REFERENCES ACC (IBAN) \
         DESTINATION KEY (PAYEE) REFERENCES ACC (IBAN)) OPTIONS (TRUSTED MODE)",
    )
    .unwrap();
    let sssp = format!("SELECT * FROM GRAPH_ALGO.SSSP(PAYS, {}, COST => 'PAID') AS D ORDER BY 1", identity("ACC", "1"));
    assert_eq!(named(&s, &sssp), ["ACC1,0E0", "ACC2,5E0", "ACC3,1.2E1"]);
    assert_eq!(query(&s, "SELECT COUNT(*) FROM GRAPH_ALGO.WCC(PAYS) AS W"), ["3"], "no vertex of XX99");
}

/// Verifies: [EXTENSION] [GS §18] [GS §4.2] SYS_GRAPH, the implicit
/// graph of a schema's node and edge tables, in place of the edges: each
/// vertex its `$NODE_ID`, joinable to its node table, of any of its node
/// tables; each edge its `$FROM_ID` and `$TO_ID`, COST one of its columns.
#[test]
fn native_graphs_by_name() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION H").unwrap();
    let s = db.session("H");
    s.run("CREATE TABLE N (NAME VARCHAR(5)) AS NODE").unwrap();
    s.run("CREATE TABLE M (NAME VARCHAR(5)) AS NODE").unwrap();
    s.run("CREATE TABLE L (W DOUBLE PRECISION) AS EDGE").unwrap();
    s.run("INSERT INTO N VALUES ('a'), ('b'), ('c'), ('d')").unwrap();
    s.run("INSERT INTO M VALUES ('m')").unwrap();
    let id = |t: &str, name: &str| query(&s, &format!("SELECT $NODE_ID FROM {t} WHERE NAME = '{name}'")).remove(0);
    let edges = [
        (id("N", "a"), id("N", "b"), 1),
        (id("N", "b"), id("N", "c"), 2),
        (id("N", "c"), id("N", "a"), 1),
        (id("N", "c"), id("M", "m"), 4),
    ];
    for (from, to, w) in edges {
        s.run(&format!("INSERT INTO L ($FROM_ID, $TO_ID, W) VALUES ('{from}', '{to}', {w})")).unwrap();
    }
    let components = "SELECT COUNT(*), COUNT(DISTINCT COMPONENT) FROM GRAPH_ALGO.WCC(SYS_GRAPH) AS W";
    assert_eq!(query(&s, components), ["5,2"]);
    let a = id("N", "a");
    let levels =
        format!("SELECT N.NAME, B.DEPTH FROM GRAPH_ALGO.BFS_LEVELS(SYS_GRAPH, '{a}') AS B JOIN N ON N.$NODE_ID = B.VERTEX ORDER BY 1");
    assert_eq!(query(&s, &levels), ["a,0", "b,1", "c,2"]);
    let costs =
        format!("SELECT M.NAME, D.DISTANCE FROM GRAPH_ALGO.SSSP(H.SYS_GRAPH, '{a}', COST => 'W') AS D JOIN M ON M.$NODE_ID = D.VERTEX");
    assert_eq!(query(&s, &costs), ["m,7E0"]);
}
