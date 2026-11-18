//! A GRAPH_TABLE's rows clauses, match modes and selectors, and how it
//! reads its element tables: laterally of the table references before it,
//! through views, and at a point in system time [GS §7, §8].

use super::{bank, rows};

/// The shortest path of transfers from Dave's account to Aretha's.
const DAVE_TO_ARETHA: &str = "ANY SHORTEST (a WHERE a.id = 'a6')-[t:Transfer]->*(b WHERE b.id = 'a2')";

/// Verifies: [EXTENSION] [GS §8 R8.3] ONE ROW PER VERTEX gives a row of
/// each vertex of the path, ONE ROW PER STEP one of each edge with the
/// vertices either side, and ONE ROW PER MATCH one of the match.
#[test]
fn rows_per_vertex_and_step_8() {
    let (_db, s) = bank();
    let per = |clause: &str, columns: &str| {
        rows(&s, &format!("SELECT * FROM GRAPH_TABLE (BANK MATCH {DAVE_TO_ARETHA} {clause} COLUMNS ({columns}))"))
    };
    assert_eq!(per("ONE ROW PER VERTEX (v)", "v.id AS V, a.id AS A"), ["a2,a6", "a3,a6", "a6,a6"]);
    assert_eq!(per("ONE ROW PER STEP (x, e, y)", "x.id AS X, e.id AS E, y.id AS Y"), ["a3,t2,a2", "a6,t5,a3"]);
    assert_eq!(per("ONE ROW PER MATCH", "b.id AS B"), ["a2"]);
    let single = "SELECT * FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1') ONE ROW PER STEP (x, e, y) COLUMNS (x.id AS X, e.id AS E))";
    assert_eq!(rows(&s, single), ["a1,NULL"]);
}

/// Verifies: [EXTENSION] [GS §8 R8.1] a GRAPH_TABLE reads the columns of
/// the table references before it, each row its own matches; of a LEFT
/// join, a row of none null extended.
#[test]
fn lateral_reads_8() {
    let (_db, s) = bank();
    let q = "SELECT A.OWNER, G.N FROM ACCOUNT AS A, GRAPH_TABLE (BANK MATCH (x WHERE x.id = A.ID)-[t:Transfer]->(y) \
             COLUMNS (y.id AS N)) AS G WHERE A.ID IN ('a3', 'a6')";
    assert_eq!(rows(&s, q), ["Dave,a3", "Dave,a5", "Mike,a2", "Mike,a5"]);
    let left = "SELECT A.ID, G.N FROM ACCOUNT AS A LEFT JOIN GRAPH_TABLE (BANK MATCH (x WHERE x.id = A.ID)-[t:Transfer WHERE \
                t.amount > 9]->(y) COLUMNS (y.id AS N)) AS G ON TRUE";
    assert_eq!(rows(&s, left), ["a1,NULL", "a2,a4", "a3,a2", "a4,a6", "a5,NULL", "a6,a3"]);
}

/// Verifies: [EXTENSION] [GS §7.3] DIFFERENT EDGES keeps the matches of
/// no edge bound twice, REPEATABLE ELEMENTS every one.
#[test]
fn match_modes_7_3() {
    let (_db, s) = bank();
    let of = |mode: &str| {
        rows(
            &s,
            &format!(
                "SELECT COUNT(*) FROM GRAPH_TABLE (BANK MATCH {mode} (a)-[e:Transfer]->(b), (a)-[f:Transfer]->(c) COLUMNS (a.id AS A))"
            ),
        )
    };
    assert_eq!(of("REPEATABLE ELEMENTS"), ["12"]);
    assert_eq!(of(""), ["12"]);
    assert_eq!(of("DIFFERENT EDGES"), ["4"]);
}

/// Verifies: [EXTENSION] [GS §7.2] ANY CHEAPEST keeps a path of least
/// COST, ALL CHEAPEST each, CHEAPEST k the k cheapest; a negative COST
/// fails, 22W34.
#[test]
fn cheapest_paths_7_2() {
    let (_db, s) = bank();
    let of = |selector: &str| {
        rows(
            &s,
            &format!(
                "SELECT T, C FROM GRAPH_TABLE (BANK MATCH {selector} p = (a WHERE a.id = 'a6')-[t:Transfer]->*(b WHERE b.id = 'a2') \
                 COLUMNS (ARRAY_AGG(t.id) AS T, SUM(t.amount) AS C))"
            ),
        )
    };
    assert_eq!(of("ANY CHEAPEST COST t.amount"), ["ARRAY['t5', 't2'],20"]);
    let t5_dear = "ANY CHEAPEST COST (CASE WHEN t.id = 't5' THEN 100 ELSE 1 END)";
    assert_eq!(of(t5_dear), ["ARRAY['t6', 't8', 't1', 't2'],31"]);
    assert_eq!(of("CHEAPEST 2 COST t.amount"), ["ARRAY['t5', 't2'],20", "ARRAY['t6', 't8', 't1', 't2'],31"]);
    assert_eq!(of("ALL CHEAPEST"), ["ARRAY['t5', 't2'],20"]);
    let negative = "SELECT 1 FROM GRAPH_TABLE (BANK MATCH ANY CHEAPEST COST (t.amount - 9) (a WHERE a.id = 'a6')-[t:Transfer]->*(b) COLUMNS (a.id AS A))";
    let e = s.open_cursor(negative).and_then(|mut c| c.fetch().map(|_| ())).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "22W34");
}

/// Verifies: [EXTENSION] [GS §4.1] an element table may be a view, its
/// rows the view's, read with the graph owner's privileges.
#[test]
fn views_are_element_tables_4_1() {
    let (_db, s) = bank();
    s.run("CREATE VIEW LARGE AS SELECT ID, SRC, DST, AMOUNT FROM TRANSFER WHERE AMOUNT >= 10").unwrap();
    s.run(
        "CREATE PROPERTY GRAPH LARGE_BANK VERTEX TABLES (ACCOUNT KEY (ID) PROPERTIES (ID)) EDGE TABLES (LARGE KEY (ID) \
         SOURCE KEY (SRC) REFERENCES ACCOUNT (ID) DESTINATION KEY (DST) REFERENCES ACCOUNT (ID) PROPERTIES (ID, AMOUNT)) \
         OPTIONS (TRUSTED MODE)",
    )
    .unwrap();
    let q = "SELECT E FROM GRAPH_TABLE (LARGE_BANK MATCH TRAIL (a WHERE a.id = 'a3')-[e]->+(b WHERE b.id = 'a3') COLUMNS (ARRAY_AGG(e.id) AS E))";
    assert_eq!(rows(&s, q), ["ARRAY['t2', 't3', 't4', 't5']"]);
}

/// A versioned graph of accounts 1 and 2 and a transfer 10 between them,
/// committed, then the transfer deleted: the start of its version.
fn history(s: &turborel::db::Session) -> String {
    let period = "S TIMESTAMP(6) GENERATED ALWAYS AS ROW START, E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, PERIOD FOR SYSTEM_TIME (S, E)";
    s.run(&format!("CREATE TABLE P (ID INTEGER PRIMARY KEY, {period}) WITH SYSTEM VERSIONING")).unwrap();
    s.run(&format!(
        "CREATE TABLE T (ID INTEGER PRIMARY KEY, SRC INTEGER REFERENCES P, DST INTEGER REFERENCES P, {period}) WITH SYSTEM VERSIONING"
    ))
    .unwrap();
    s.run(
        "CREATE PROPERTY GRAPH H VERTEX TABLES (P PROPERTIES (ID)) EDGE TABLES (T SOURCE KEY (SRC) REFERENCES P (ID) \
         DESTINATION KEY (DST) REFERENCES P (ID) PROPERTIES (ID))",
    )
    .unwrap();
    s.run("INSERT INTO P (ID) VALUES (1), (2)").unwrap();
    s.run("INSERT INTO T (ID, SRC, DST) VALUES (10, 1, 2)").unwrap();
    s.run("COMMIT WORK").unwrap();
    let start = rows(s, "SELECT S FROM T").remove(0);
    s.run("COMMIT WORK").unwrap();
    s.run("DELETE FROM T").unwrap();
    s.run("COMMIT WORK").unwrap();
    format!("TIMESTAMP '{start}'")
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.6 GR2c] a GRAPH_TABLE
/// FOR SYSTEM_TIME AS OF a point in time, or AS OF TIMESTAMP, reads each
/// element table as it was then.
#[test]
fn read_as_of_a_point_in_time_7_6() {
    let (_db, s) = bank();
    let start = history(&s);
    let edges = |at: &str| rows(&s, &format!("SELECT I FROM GRAPH_TABLE (H MATCH (a)-[t]->(b) COLUMNS (t.id AS I)) {at}"));
    assert_eq!(edges(""), Vec::<String>::new());
    assert_eq!(edges(&format!("FOR SYSTEM_TIME AS OF {start}")), ["10"]);
    assert_eq!(edges(&format!("AS G AS OF TIMESTAMP {start}")), ["10"]);
}
