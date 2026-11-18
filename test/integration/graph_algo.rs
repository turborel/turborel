//! \[EXTENSION\] The graph algorithm library of the graph standard's §18
//! (RULES.md D-GRAPH-ALGO): the functions of GRAPH_ALGO over a table of
//! edges, their arguments positional and named, their results joined to
//! tables, INTO PROPERTY, and INFORMATION_SCHEMA.GRAPH_ALGO_RUNS.

use turborel::db::{Database, Session};
use turborel::error::SqlError;

use crate::common::{failed, rows};

mod named;

/// Schema G: V (ID INTEGER PRIMARY KEY, NAME), vertices 1 to 5; E (SRC,
/// DST, W DOUBLE PRECISION, LABEL), the edges 1 → 2 (1), 2 → 3 (2),
/// 3 → 1 (1), 3 → 4 (5) and 1 → 4 (10); vertex 5 is of no edge.
fn graph() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION G \
         CREATE TABLE V (ID INTEGER PRIMARY KEY, NAME VARCHAR(10)) \
         CREATE TABLE E (SRC INTEGER, DST INTEGER, W DOUBLE PRECISION, LABEL VARCHAR(5))",
    )
    .unwrap();
    let s = db.session("G");
    s.run("INSERT INTO V VALUES (1, 'a'), (2, 'b'), (3, 'c'), (4, 'd'), (5, 'e')").unwrap();
    s.run("INSERT INTO E VALUES (1, 2, 1, 'x'), (2, 3, 2, 'x'), (3, 1, 1, 'y'), (3, 4, 5, 'y'), (1, 4, 10, 'x')").unwrap();
    s.commit().unwrap();
    db
}

/// The rows of `sql`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|code| panic!("{sql}: {code:?}"))
}

/// The rows of the algorithm invocation `call`, ordered by its columns.
fn ordered(s: &Session, call: &str) -> Vec<String> {
    query(s, &format!("SELECT * FROM GRAPH_ALGO.{call} AS R ORDER BY 1, 2"))
}

/// The SQLCODE `sql` is refused with, and its message.
fn refused(s: &Session, sql: &str) -> (i32, String) {
    let e: SqlError = failed(s, sql);
    (e.sqlcode(), e.msg)
}

/// Verifies: [EXTENSION] [GS §18] Each algorithm of GRAPH_ALGO over the
/// table E: WCC and SCC label each vertex by the least key of its
/// component, VERTICES adding the vertex of no edge; BFS_LEVELS and SSSP
/// give the vertices reached from START, SSSP by the least sum of COST;
/// TRIANGLE_COUNT, LOCAL_CLUSTERING and K_CORE read the edges with no
/// direction; BETWEENNESS counts the shortest paths through each vertex;
/// JACCARD_SIMILARITY the pairs of a threshold; PAGERANK's ranks sum to 1.
#[test]
fn every_algorithm_over_a_table_of_edges() {
    let db = graph();
    let s = db.session("G");
    assert_eq!(ordered(&s, "WCC(TABLE (E), VERTICES => TABLE (V))"), ["1,1", "2,1", "3,1", "4,1", "5,5"]);
    assert_eq!(ordered(&s, "SCC(TABLE (E))"), ["1,1", "2,1", "3,1", "4,4"]);
    assert_eq!(ordered(&s, "BFS_LEVELS(TABLE (E), 1)"), ["1,0", "2,1", "3,2", "4,1"]);
    assert_eq!(ordered(&s, "SSSP(TABLE (E), 1, COST => 'w')"), ["1,0E0", "2,1E0", "3,3E0", "4,8E0"]);
    assert_eq!(ordered(&s, "SSSP(TABLE (E), START => 4)"), ["4,0E0"]);
    assert_eq!(ordered(&s, "BFS_LEVELS(TABLE (E), 9)"), Vec::<String>::new(), "a START of no vertex reaches none");
    assert_eq!(ordered(&s, "TRIANGLE_COUNT(TABLE (E))"), ["1,2", "2,1", "3,2", "4,1"]);
    let clustering = "SELECT VERTEX, CAST(COEFFICIENT * 1000 AS INTEGER) FROM GRAPH_ALGO.LOCAL_CLUSTERING(TABLE (E)) AS C ORDER BY 1";
    assert_eq!(query(&s, clustering), ["1,667", "2,1000", "3,667", "4,1000"]);
    assert_eq!(ordered(&s, "K_CORE(TABLE (E), VERTICES => TABLE (V))"), ["1,2", "2,2", "3,2", "4,2", "5,0"]);
    assert_eq!(ordered(&s, "BETWEENNESS(TABLE (E))"), ["1,1E0", "2,1E0", "3,2E0", "4,0E0"]);
    assert_eq!(ordered(&s, "JACCARD_SIMILARITY(TABLE (E), THRESHOLD => 0.9)"), ["2,4,1E0"]);
    let sum =
        "SELECT COUNT(*), CAST(SUM(RANK) * 1000000 AS INTEGER) FROM GRAPH_ALGO.PAGERANK(TABLE (E), DAMPING => 0.85, ITERATIONS => 50) AS P";
    assert_eq!(query(&s, sum), ["4,1000000"]);
    for communities in ["LOUVAIN(TABLE (E), SEED => 7)", "LABEL_PROPAGATION(TABLE (E))"] {
        let q = format!("SELECT COUNT(*) FROM GRAPH_ALGO.{communities} AS C WHERE COMMUNITY IN (SELECT ID FROM V)");
        assert_eq!(query(&s, &q), ["4"], "{communities}");
    }
}

/// Verifies: [EXTENSION] [GS §18] A result joins to the table of its
/// vertices by key; it is TABLE ( ... ) of a `<PTF derived table>` too,
/// its columns renamed by a derived column list; the edges are any query,
/// so a subgraph of a label is a WHERE, and an edge of a null key is
/// none; the threads EXECUTOR.GRAPH_THREADS
/// says give the same rows.
#[test]
fn results_join_to_tables() {
    let db = graph();
    let s = db.session("G");
    let joined = "SELECT V.NAME, C.COMPONENT FROM V JOIN GRAPH_ALGO.SCC(TABLE (E)) AS C ON V.ID = C.VERTEX ORDER BY V.NAME";
    assert_eq!(query(&s, joined), ["a,1", "b,1", "c,1", "d,4"]);
    let wrapped = "SELECT N, L FROM TABLE (GRAPH_ALGO.BFS_LEVELS(TABLE (E), 3)) AS B (N, L) ORDER BY N";
    assert_eq!(query(&s, wrapped), ["1,1", "2,2", "3,0", "4,1"]);
    let labelled = "SELECT * FROM GRAPH_ALGO.WCC(TABLE (SELECT SRC, DST FROM E WHERE LABEL = 'y')) AS W ORDER BY 1";
    assert_eq!(query(&s, labelled), ["1,1", "3,1", "4,1"]);
    s.run("INSERT INTO E VALUES (5, NULL, 1, 'n'), (NULL, 5, 1, 'n')").unwrap();
    assert_eq!(query(&s, "SELECT COUNT(*) FROM GRAPH_ALGO.WCC(TABLE (E)) AS W"), ["4"], "an edge of a null key is none");
    let one = ordered(&s, "PAGERANK(TABLE (E))");
    s.run("SET EXECUTOR.GRAPH_THREADS = 4").unwrap();
    assert_eq!(ordered(&s, "PAGERANK(TABLE (E))"), one);
}

/// Verifies: [EXTENSION] [GS §18, §22] An invocation is refused with
/// SQLCODE -103 if GRAPH_ALGO has no function of its name, an argument
/// names no parameter or follows a named one unnamed, START is missing,
/// or a key is not comparable with the source's; with -301 and GRF-0034
/// if COST names no column, a cost is not a number, or SSSP's costs make
/// a negative cycle the start reaches, a damping past 1 too; or the
/// edges are a value that names no graph.
#[test]
fn invocations_refused() {
    let db = graph();
    let s = db.session("G");
    assert_eq!(refused(&s, "SELECT * FROM GRAPH_ALGO.CENTRALITY(TABLE (E)) AS C").0, -103);
    assert_eq!(refused(&s, "SELECT * FROM GRAPH_ALGO.WCC(TABLE (E), DAMPING => 1) AS C").0, -103);
    assert_eq!(refused(&s, "SELECT * FROM GRAPH_ALGO.SSSP(TABLE (E), COST => 'W', 1) AS C").0, -103);
    assert_eq!(refused(&s, "SELECT * FROM GRAPH_ALGO.BFS_LEVELS(TABLE (E)) AS C").0, -103);
    assert_eq!(refused(&s, "SELECT * FROM GRAPH_ALGO.WCC(TABLE (SELECT SRC, LABEL FROM E)) AS C").0, -103);
    let (code, msg) = refused(&s, "SELECT * FROM GRAPH_ALGO.SSSP(TABLE (E), 1, COST => 'NONE') AS C");
    assert!(code == -301 && msg.contains("GRF-0034"), "{code} {msg}");
    let (code, msg) = refused(&s, "SELECT * FROM GRAPH_ALGO.SSSP(TABLE (E), 1, COST => 'LABEL') AS C");
    assert!(code == -301 && msg.contains("GRF-0034"), "{code} {msg}");
    s.run("INSERT INTO E VALUES (2, 1, -5, 'z')").unwrap();
    let (code, msg) = refused(&s, "SELECT * FROM GRAPH_ALGO.SSSP(TABLE (E), 1, COST => 'W') AS C");
    assert!(code == -301 && msg.contains("GRF-0034") && msg.contains("negative cycle"), "{code} {msg}");
    let state = failed(&s, "SELECT * FROM GRAPH_ALGO.SSSP(TABLE (E), 1, COST => 'W') AS C").sqlstate().as_str().to_owned();
    assert_eq!(state, "22W34", "GRF-0034's SQLSTATE");
    assert_eq!(refused(&s, "SELECT * FROM GRAPH_ALGO.LOUVAIN(TABLE (E), COST => 'W') AS C").0, -301);
    assert_eq!(refused(&s, "SELECT * FROM GRAPH_ALGO.PAGERANK(TABLE (E), DAMPING => 2) AS C").0, -301);
    assert_eq!(refused(&s, "SELECT * FROM GRAPH_ALGO.WCC(E) AS C").0, -103);
    assert_eq!(refused(&s, "SELECT * FROM GRAPH_ALGO.WCC(TABLE (E), VERTICES => V) AS C").0, -103);
    assert_eq!(failed(&s, "SELECT * FROM GRAPH_ALGO.WCC(TABLE (NOSUCH)) AS C").sqlcode(), -103);
}

/// Verifies: [EXTENSION] [GS §18] INTO PROPERTY writes a result as a
/// column of the vertices' table: added of DOUBLE PRECISION, BIGINT or
/// the key's type if the table has none of its name, each row of a
/// result row set, the others null; of a column there already, set
/// again; the key the table's primary key unless named; in the
/// statement's transaction, so ROLLBACK undoes it. A result of pairs is
/// no property (-103).
#[test]
fn into_property_writes_a_column() {
    let db = graph();
    let s = db.session("G");
    s.run("GRAPH_ALGO.BFS_LEVELS(TABLE (E), 1) INTO PROPERTY HOPS OF V").unwrap();
    assert_eq!(query(&s, "SELECT ID, HOPS FROM V ORDER BY ID"), ["1,0", "2,1", "3,2", "4,1", "5,NULL"]);
    s.run("GRAPH_ALGO.SCC(TABLE (E)) INTO PROPERTY PART OF V (ID)").unwrap();
    assert_eq!(query(&s, "SELECT ID, PART FROM V ORDER BY ID"), ["1,1", "2,1", "3,1", "4,4", "5,NULL"]);
    s.run("GRAPH_ALGO.BFS_LEVELS(TABLE (E), 3) INTO PROPERTY HOPS OF V").unwrap();
    assert_eq!(query(&s, "SELECT ID, HOPS FROM V ORDER BY ID"), ["1,1", "2,2", "3,0", "4,1", "5,NULL"]);
    let types = "SELECT COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'V' AND ORDINAL_POSITION > 2 ORDER BY 1";
    assert_eq!(query(&s, types), ["HOPS,BIGINT", "PART,INTEGER"]);
    s.commit().unwrap();
    s.run("GRAPH_ALGO.PAGERANK(TABLE (E)) INTO PROPERTY RANK OF V").unwrap();
    assert_eq!(query(&s, "SELECT COUNT(RANK) FROM V"), ["4"]);
    s.rollback().unwrap();
    assert_eq!(failed(&s, "SELECT RANK FROM V").sqlcode(), -103, "no column RANK");
    assert_eq!(s.run("GRAPH_ALGO.JACCARD_SIMILARITY(TABLE (E)) INTO PROPERTY J OF V").unwrap_err().sqlcode(), -103);
}

/// Verifies: [EXTENSION] [GS §20] INFORMATION_SCHEMA.GRAPH_ALGO_RUNS shows
/// each run, numbered from 1: its algorithm, its value arguments, its
/// user, its graph's vertices and edges, its rows, its threads, those
/// EXECUTOR.GRAPH_THREADS says, and how it ended, a failed run's message; the database's owner sees every run,
/// another user only their own.
#[test]
fn runs_are_shown() {
    let db = graph();
    let s = db.session("G");
    query(&s, "SELECT * FROM GRAPH_ALGO.PAGERANK(TABLE (E), DAMPING => 0.5) AS P");
    failed(&s, "SELECT * FROM GRAPH_ALGO.SSSP(TABLE (E), 1, COST => 'NONE') AS P");
    let shown = "SELECT RUN_ID, ALGORITHM, ARGUMENTS, USER_NAME, VERTICES, EDGES, ROWS_RETURNED, THREADS, STATUS, MESSAGE IS NULL \
                 FROM INFORMATION_SCHEMA.GRAPH_ALGO_RUNS ORDER BY RUN_ID";
    let runs = query(&s, shown);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(runs[0].starts_with("1,PAGERANK,DAMPING => 5E-1,G,") && runs[0].ends_with(",G,4,5,4,1,DONE,TRUE"), "{runs:?}");
    assert!(runs[1].starts_with("2,SSSP,START => 1, COST => 'NONE',G,0,0,0,") && runs[1].ends_with("FAILED,FALSE"), "{runs:?}");
    s.run("SET EXECUTOR.GRAPH_THREADS = 4").unwrap();
    query(&s, "SELECT * FROM GRAPH_ALGO.WCC(TABLE (E)) AS W");
    assert_eq!(query(&s, "SELECT THREADS FROM INFORMATION_SCHEMA.GRAPH_ALGO_RUNS WHERE RUN_ID = 3"), ["4"]);
    let other = db.session("H");
    assert_eq!(query(&other, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.GRAPH_ALGO_RUNS"), ["0"]);
}
