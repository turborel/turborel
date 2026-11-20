//! Rigid patterns planned as joins (RULES.md D147): each gives the rows
//! the path search gives, which PATH_ALGO keeps it on; EXPLAIN shows the
//! join and its Expand, Expand walked in reverse and ExpandInto.

use turborel::db::{Database, Session};
use turborel::params::Params;

use super::{bank, rows};

/// The rows of rigid pattern `pattern` of BANK, its COLUMNS `columns`,
/// joined, or of a cycle by the worst-case optimal join, and the same
/// searched; and whether EXPLAIN shows each so.
fn both(s: &Session, (pattern, columns): (&str, &str)) -> (Vec<String>, Vec<String>) {
    let q = |options: &str| format!("SELECT * FROM GRAPH_TABLE (BANK MATCH {pattern} COLUMNS ({columns}) {options})");
    let (joined, searched) = (q(""), q("OPTIONS (PATH_ALGO(DFS))"));
    let shown = s.explain(&joined, &Params::new()).unwrap();
    let joins = !shown.contains("Graph table") || shown.contains("worst-case optimal join");
    assert!(joins, "{joined} is joined: {shown}");
    let shown = s.explain(&searched, &Params::new()).unwrap();
    assert!(shown.contains("Graph table"), "{searched} is searched: {shown}");
    (rows(s, &joined), rows(s, &searched))
}

/// Verifies: [EXTENSION] [GS §16 R16.2] RULES.md D147: each rigid pattern
/// of every orientation, of variables binding several tables, of shared
/// labels, of cycles, of two path patterns, of prefilters and postfilters,
/// of element functions, of each restrictor and match mode, of small
/// quantifiers unrolled ([GS §16 R16.4]), gives as joins the rows its
/// search gives, a directed and an undirected self-loop among them. Of
/// edge tables of no adjacency index, a pattern of more than 4 whole
/// scans of them is searched; of one, an edge of ends that may be null is
/// read by Expand.
#[test]
fn rigid_patterns_join_as_they_search_16() {
    let (_db, s) = bank();
    s.run("INSERT INTO TRANSFER VALUES ('t9', 'a2', 'a2', DATE '2020-10-01', 1)").unwrap();
    // Transfers of no ends, edges of no pattern, so a probe of one row
    // costs less than reading TRANSFER (RULES.md D370).
    let ended = (0..40).map(|k| format!("('n{k}', NULL, NULL, DATE '2020-01-01', 0)")).collect::<Vec<_>>().join(", ");
    s.run(&format!("INSERT INTO TRANSFER VALUES {ended}")).unwrap();
    let shown = |pattern: &str| {
        let q = format!("SELECT * FROM GRAPH_TABLE (BANK MATCH {pattern} COLUMNS (a.id AS A))");
        s.explain(&q, &Params::new()).unwrap().contains("Graph table")
    };
    // Before the commit gives the edge tables their adjacency indexes, of
    // 2 ways each, 2 joins of 1 scan, and 4 of 2.
    let (joined, searched) = both(&s, ("(a)-[t:Transfer]-(b)", "a.id AS A, t.id AS T, b.id AS B"));
    assert_eq!(joined, searched, "2 scans are joined");
    assert!(shown("(a)-[:Transfer]-(b)-[:Transfer]-(c)"), "8 scans are searched");
    s.run("COMMIT WORK").unwrap();
    assert!(!shown("(a)-[:Transfer]-(b)-[:Transfer]-(c)"), "an indexed one is joined");
    // Of ends that may be null, joined to a vertex, none is: Expand reads them.
    let q = "SELECT * FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1')-[:Transfer]->(b) COLUMNS (b.id AS B))";
    let plan = s.explain(q, &Params::new()).unwrap();
    assert!(plan.contains("Expand A.TRANSFER") && plan.contains("SRC IS NOT NULL"), "{plan}");
    let ids = "a.id AS A, t.id AS T, b.id AS B";
    let cases = [
        ("(a)-[t:Transfer]->(b)", ids),
        ("(a)<-[t:Transfer]-(b)", ids),
        ("(a)-[t:Transfer]-(b)", ids),
        ("(a)<->(b)", "a.id AS A, b.id AS B"),
        ("(a)-[t]->(b)", ids),
        ("(a)~[t]~(b)", ids),
        ("(a)-[t]-(b:Phone)", ids),
        ("(a)-[t:IsLocatedIn]->(b:Country)", "a.id AS A, b.name AS B"),
        ("(a)-[t:Transfer]->(b)-[u:Transfer]->(c) WHERE a.id <> c.id", "a.id AS A, b.id AS B, c.id AS C"),
        ("(a)-[t:Transfer]->(b)-[u:Transfer]->(a)", "a.id AS A, b.id AS B"),
        ("(a)-[t:Transfer]->(a)", "a.id AS A, t.id AS T"),
        ("(a)-[t:Transfer]->(b), (b)-[l:IsLocatedIn]->(c)", "a.id AS A, b.id AS B, c.name AS C"),
        ("DIFFERENT EDGES (a)-[t:Transfer]->(b), (b)-[u:Transfer]->(c)", "t.id AS T, u.id AS U"),
        ("DIFFERENT VERTICES (a)-[t:Transfer]->(b)-[u:Transfer]->(c)", "a.id AS A, c.id AS C"),
        ("TRAIL (a)-[t:Transfer]-(b)-[u:Transfer]-(c)", "t.id AS T, u.id AS U"),
        ("ACYCLIC (a)-[t:Transfer]->(b)-[u:Transfer]->(c)", "a.id AS A, c.id AS C"),
        ("SIMPLE (a)-[t:Transfer]->(b)-[u:Transfer]->(c)-[v:Transfer]->(d)", "a.id AS A, d.id AS D"),
        ("(a WHERE a.owner = 'Mike')-[t:Transfer WHERE t.amount > 5]->(b)", ids),
        ("(a)-[t:Transfer]->(b) WHERE t.amount > 9 AND b IS DESTINATION OF t", ids),
        ("(a:Account)-[t]-(b)", "ELEMENT_ID(t) AS E, VERTEX_ID(b) AS V"),
        ("(a)-[t:Transfer]->{1,3}(b)", "a.id AS A, b.id AS B"),
        ("(a WHERE a.id = 'a1')-[t:Transfer WHERE t.amount > 7]->{1,2}(b)", "a.id AS A, b.id AS B"),
        ("(a WHERE a.id <> 'a2')-[:Transfer]->{0,1}()", "a.id AS A"),
        ("TRAIL (a)-[:Transfer]-{2}(b)", "a.id AS A, b.id AS B"),
        ("(a)-[t:Transfer]->{1,2}(b)-[l:IsLocatedIn]->(c)", "a.id AS A, b.id AS B, c.name AS C"),
    ];
    for case in cases {
        let (joined, searched) = both(&s, case);
        assert_eq!(joined, searched, "{case:?}");
        assert!(!joined.is_empty() || case.0.starts_with("SIMPLE"), "{case:?} matches");
    }
}

/// A database of the graph ROADS, ENFORCED, its adjacency index given as
/// it commits, of four cities and five roads of NOT NULL ends, and a
/// chain of 30 roads apart, so a probe of one row costs less than reading
/// ROAD (RULES.md D370); and a session of A.
fn roads() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    let chain_cities = format!("INSERT INTO CITY VALUES {}", (0..=30).map(|k| format!("('c{k}')")).collect::<Vec<_>>().join(", "));
    let chain_roads = (0..30).map(|k| format!("({}, 'c{k}', 'c{}', 1)", 100 + k, k + 1)).collect::<Vec<_>>().join(", ");
    let chain_roads = format!("INSERT INTO ROAD VALUES {chain_roads}");
    for sql in [
        "CREATE TABLE CITY (NAME VARCHAR(10) PRIMARY KEY)",
        "CREATE TABLE ROAD (ID INTEGER PRIMARY KEY, A VARCHAR(10) NOT NULL REFERENCES CITY, \
         B VARCHAR(10) NOT NULL REFERENCES CITY, KM INTEGER NOT NULL)",
        "INSERT INTO CITY VALUES ('x'), ('y'), ('z'), ('w')",
        "INSERT INTO ROAD VALUES (1, 'x', 'y', 5), (2, 'y', 'z', 7), (3, 'z', 'x', 2), (4, 'x', 'w', 1), (5, 'w', 'y', 3)",
        chain_cities.as_str(),
        chain_roads.as_str(),
        "CREATE PROPERTY GRAPH ROADS VERTEX TABLES (CITY) EDGE TABLES (ROAD \
         SOURCE KEY (A) REFERENCES CITY (NAME) DESTINATION KEY (B) REFERENCES CITY (NAME))",
        "COMMIT WORK",
    ] {
        s.run(sql).unwrap();
    }
    (db, s)
}

/// Verifies: [EXTENSION] [GS §16 R16.1, R16.2] RULES.md D147: a rigid
/// pattern's edges are read through the adjacency index, from a vertex
/// bound before as Expand, from its destination as Expand walked in
/// reverse; the rows are the search's, which a search hint keeps.
#[test]
fn edges_are_expanded_either_way_16() {
    let (_db, s) = roads();
    let cases = [
        ("SELECT * FROM GRAPH_TABLE (ROADS MATCH (a WHERE a.name = 'x')-[r]->(b) COLUMNS (b.name AS B))", "Expand A.ROAD", ["w", "y"]),
        (
            "SELECT * FROM GRAPH_TABLE (ROADS MATCH (a WHERE a.name = 'y')<-[r]-(b) COLUMNS (b.name AS B))",
            "Expand (reverse) A.ROAD",
            ["w", "x"],
        ),
    ];
    for (q, shown, expected) in cases {
        let plan = s.explain(q, &Params::new()).unwrap();
        assert!(plan.contains(shown), "{shown}: {plan}");
        assert_eq!(rows(&s, q), expected);
    }
    // [GS §16 R16.8] A hint of how to search keeps the search.
    let hinted = "SELECT * FROM GRAPH_TABLE (ROADS MATCH (a WHERE a.name = 'y')<-[r]-(b) COLUMNS (b.name AS B) OPTIONS (GRAPH_START (b)))";
    assert!(s.explain(hinted, &Params::new()).unwrap().contains("Graph table"));
    assert_eq!(rows(&s, hinted), ["w", "x"]);
}

/// Verifies: [EXTENSION] [GS §16 R16.2, R16.8] RULES.md D148: a rigid
/// pattern whose node patterns close a cycle, over edge tables each with
/// an adjacency index, is matched by the worst-case optimal join, of one
/// path pattern or several, of prefilters and postfilters; it
/// gives the rows of the search and of the plan NO_WCOJ asks, a join
/// unless its restrictor's variables repeat.
#[test]
fn cycles_take_the_worst_case_optimal_join_16() {
    let (_db, s) = roads();
    s.run("INSERT INTO ROAD VALUES (6, 'y', 'x', 4), (7, 'z', 'z', 1)").unwrap();
    s.run("COMMIT WORK").unwrap();
    let cases = [
        ("(a)-[r]->(b)-[s]->(c)-[t]->(a)", "a.name AS A, b.name AS B, c.name AS C"),
        ("(a)-[r]->(b), (b)-[s]->(c), (c)-[t]->(a)", "a.name AS A, b.name AS B, c.name AS C"),
        ("(a)-[r]-(b)-[s]-(a)", "a.name AS A, r.id AS R, s.id AS S"),
        ("(a)-[r]->(a)", "a.name AS A, r.id AS R"),
        ("TRAIL (a)-[r]-(b)-[s]-(a)", "r.id AS R, s.id AS S"),
        ("(a)-[r]->(b WHERE b.name <> 'w')-[s]->(c)-[t]->(a) WHERE c.name <> a.name AND r.km < 6", "a.name AS A, c.name AS C"),
    ];
    for (pattern, columns) in cases {
        let q = |options: &str| format!("SELECT * FROM GRAPH_TABLE (ROADS MATCH {pattern} COLUMNS ({columns}) {options})");
        let (wcoj, joined, searched) = (q(""), q("OPTIONS (NO_WCOJ)"), q("OPTIONS (PATH_ALGO(DFS))"));
        let plan = s.explain(&wcoj, &Params::new()).unwrap();
        assert!(plan.contains("worst-case optimal join"), "{wcoj}: {plan}");
        let plan = s.explain(&joined, &Params::new()).unwrap();
        assert!(!plan.contains("worst-case optimal join"), "{joined}: {plan}");
        let expected = rows(&s, &searched);
        assert!(!expected.is_empty(), "{pattern}");
        assert_eq!(rows(&s, &wcoj), expected, "{pattern}");
        assert_eq!(rows(&s, &joined), expected, "{pattern}");
    }
}

/// Verifies: [EXTENSION] [GS §16 R16.6] RULES.md D148: a path pattern
/// written as an earlier one is, anonymous elements aside, is searched
/// once, as EXPLAIN shows, and gives the rows it gives searched again;
/// an edge pattern written twice in a join is joined once.
#[test]
fn common_sub_patterns_are_matched_once_16() {
    let (_db, s) = bank();
    let path = "ANY SHORTEST (a WHERE a.id = 'a1')-[:Transfer]->+(b)";
    let q = format!("SELECT * FROM GRAPH_TABLE (BANK MATCH {path}, {path} COLUMNS (a.id AS A, b.id AS B))");
    let plan = s.explain(&q, &Params::new()).unwrap();
    assert!(plan.contains("path 2: as path 1, searched once"), "{plan}");
    let once = format!("SELECT * FROM GRAPH_TABLE (BANK MATCH {path} COLUMNS (a.id AS A, b.id AS B))");
    assert_eq!(rows(&s, &q), rows(&s, &once));
    let other = format!("SELECT * FROM GRAPH_TABLE (BANK MATCH {path}, ANY SHORTEST (a)-[:Transfer]->+(b) COLUMNS (a.id AS A))");
    assert!(!s.explain(&other, &Params::new()).unwrap().contains("as path 1"));
    // Either way, once: a join of each way, not of each pair of ways.
    let edge = "(a)-[t:Transfer]-(b), (a)-[t:Transfer]-(b)";
    let (joined, searched) = both(&s, (edge, "a.id AS A, t.id AS T"));
    assert_eq!(joined, searched);
    let plan = s.explain(&format!("SELECT * FROM GRAPH_TABLE (BANK MATCH {edge} COLUMNS (a.id AS A))"), &Params::new()).unwrap();
    assert_eq!(plan.split("-- physical:").nth(1).unwrap_or("").matches("A.TRANSFER").count(), 2, "{plan}");
}
