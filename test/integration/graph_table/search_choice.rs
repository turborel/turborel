//! How a GRAPH_TABLE's search is chosen [GS §15, §17]: MS-BFS or BFS from
//! each start by cost, on workers when the work pays for them, as a
//! PATH_ALGO hint names [GS §16 R16.8], and a frontier expanded at once
//! through an adjacency index; the rows the same every way, and EXPLAIN
//! showing the choice.

use turborel::db::{Database, Session};
use turborel::params::Params;

use super::{refused, rows};

/// The people of the social graph.
const PEOPLE: i64 = 40;

/// A database of 40 people and who knows whom, a few edges out of each,
/// and its graph SOCIAL; a session of S.
fn social() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    s.run("CREATE TABLE PERSON (ID INTEGER PRIMARY KEY, CITY INTEGER)").unwrap();
    s.run("CREATE TABLE KNOWS (ID INTEGER PRIMARY KEY, A INTEGER REFERENCES PERSON, B INTEGER REFERENCES PERSON)").unwrap();
    let people: Vec<String> = (0..PEOPLE).map(|p| format!("({p}, {})", p % 7)).collect();
    s.run(&format!("INSERT INTO PERSON VALUES {}", people.join(", "))).unwrap();
    let mut knows = Vec::new();
    for p in 0..PEOPLE {
        knows.push((p, (p * 7 + 3) % PEOPLE));
        if p % 3 == 0 {
            knows.push((p, (p + 1) % PEOPLE));
        }
        if p % 5 == 0 {
            knows.push((p, (p * p) % PEOPLE));
        }
    }
    let edges: Vec<String> = knows.iter().enumerate().map(|(k, (a, b))| format!("({k}, {a}, {b})")).collect();
    s.run(&format!("INSERT INTO KNOWS VALUES {}", edges.join(", "))).unwrap();
    s.run(
        "CREATE PROPERTY GRAPH SOCIAL VERTEX TABLES (PERSON LABEL PERSON PROPERTIES (ID, CITY)) \
         EDGE TABLES (KNOWS SOURCE KEY (A) REFERENCES PERSON (ID) DESTINATION KEY (B) REFERENCES PERSON (ID) \
         LABEL KNOWS PROPERTIES (ID))",
    )
    .unwrap();
    (db, s)
}

/// Searches from every person, and the PATH_ALGO hints each may take:
/// ANY SHORTEST both ways along KNOWS and forward; ALL SHORTEST from
/// people of city 2; TRAIL of one to three edges from ten people.
const SEARCHES: [(&str, &[&str]); 4] = [
    ("ANY SHORTEST p = (a)-[k:KNOWS]-*(b)", &["BFS", "MSBFS"]),
    ("ANY SHORTEST p = (a)-[k:KNOWS]->+(b)", &["BFS", "MSBFS"]),
    ("ALL SHORTEST p = (a WHERE a.city = 2)-[k:KNOWS]->*(b)", &["BFS"]),
    ("TRAIL p = (a WHERE a.id < 10)-[k:KNOWS]->{1,3}(b)", &["DFS"]),
];

/// Search `pattern`, its paths' ends, lengths and edges, the same each
/// run, of hint `hint` if it is not empty.
fn query(pattern: &str, hint: &str) -> String {
    let hint = if hint.is_empty() { String::new() } else { format!(", PATH_ALGO({hint})") };
    format!(
        "SELECT A, B, N, K FROM GRAPH_TABLE (SOCIAL MATCH {pattern} \
         COLUMNS (a.id AS A, b.id AS B, PATH_LENGTH(p) AS N, ARRAY_AGG(k.id) AS K) OPTIONS (DETERMINISTIC_PATHS{hint}))"
    )
}

/// Verifies: [EXTENSION] [GS §15, §17] [GS §16 R16.8] each search gives
/// the same rows whichever algorithm a PATH_ALGO hint forces, MS-BFS or
/// BFS from each start, on one worker or four from any work, its edges
/// listed by a scan or followed through the adjacency index its graph is
/// given as it commits, read of its tree and of its compacted adjacency.
#[test]
fn every_choice_gives_the_same_rows() {
    let (db, s) = social();
    let want: Vec<Vec<String>> = SEARCHES.iter().map(|(pattern, _)| rows(&s, &query(pattern, ""))).collect();
    assert!(want.iter().all(|r| r.len() > 10), "{want:?}");
    for stage in ["scanned", "indexed", "compacted"] {
        if stage == "indexed" {
            s.run("COMMIT WORK").unwrap();
        }
        if stage == "compacted" {
            db.maintain().unwrap();
        }
        for (threads, least) in [(1, 16_384), (4, 0)] {
            s.run(&format!("SET EXECUTOR.GRAPH_THREADS = {threads}")).unwrap();
            s.run(&format!("SET GRAPH.PARALLEL_MIN_STATES = {least}")).unwrap();
            for ((pattern, hints), want) in SEARCHES.iter().zip(&want) {
                for hint in [""].iter().chain(hints.iter()) {
                    assert_eq!(&rows(&s, &query(pattern, hint)), want, "{stage}, {threads} workers, {pattern} {hint}");
                }
            }
        }
    }
}

/// The plan of `query` in `s`.
fn plan(s: &Session, query: &str) -> String {
    s.explain(query, &Params::new()).unwrap()
}

/// Verifies: [EXTENSION] [GS §16 R16.9] EXPLAIN shows each search's
/// algorithm and workers and the estimate they were chosen by: MS-BFS of
/// ANY SHORTEST from every person; BFS from each as PATH_ALGO says; on
/// four workers in morsels of starts once GRAPH.PARALLEL_MIN_STATES lets
/// so little work use them.
#[test]
fn explain_shows_the_choice() {
    let (_db, s) = social();
    let q = |hint: &str| format!("SELECT * FROM GRAPH_TABLE (SOCIAL MATCH ANY SHORTEST (a)-[k:KNOWS]->*(b) COLUMNS (b.id AS B){hint})");
    let shown = plan(&s, &q(""));
    assert!(shown.contains("MS-BFS by cost: 40 starts, est. reach "), "{shown}");
    assert!(shown.contains(" of 40 vertices"), "{shown}");
    let shown = plan(&s, &q(" OPTIONS (PATH_ALGO(BFS))"));
    assert!(shown.contains("PathSearch-BFS by cost: 40 starts"), "{shown}");
    s.run("SET EXECUTOR.GRAPH_THREADS = 4").unwrap();
    assert!(plan(&s, &q(" OPTIONS (PATH_ALGO(BFS))")).contains("PathSearch-BFS by cost"), "too little work for workers");
    s.run("SET GRAPH.PARALLEL_MIN_STATES = 0").unwrap();
    let shown = plan(&s, &q(" OPTIONS (PATH_ALGO(BFS))"));
    assert!(shown.contains("PathSearch-BFS (morsels of starts, 4 workers) by cost: 40 starts"), "{shown}");
    let shown = plan(&s, &q(" OPTIONS (PATH_ALGO(DIJKSTRA))"));
    assert!(shown.contains("GRF-0070 PATH_ALGO(DIJKSTRA) cannot answer this pattern"), "{shown}");
}

/// Verifies: [EXTENSION] [GS §16 R16.8] a PATH_ALGO hint whose algorithm
/// cannot answer the pattern is refused, SQLCODE -103 (GRF-0070):
/// DIJKSTRA of no CHEAPEST, BIBFS of starts not one with one end, DFS of
/// a shortest selector, MSBFS of a TRAIL.
#[test]
fn hints_that_cannot_answer_are_refused() {
    let (_db, s) = social();
    for (pattern, hint) in [
        ("ANY SHORTEST (a)-[k:KNOWS]->*(b)", "DIJKSTRA"),
        ("ANY SHORTEST (a)-[k:KNOWS]->*(b)", "BIBFS"),
        ("ANY SHORTEST (a)-[k:KNOWS]->*(b)", "DFS"),
        ("TRAIL (a WHERE a.id = 1)-[k:KNOWS]->{1,2}(b)", "MSBFS"),
    ] {
        let q = format!("SELECT * FROM GRAPH_TABLE (SOCIAL MATCH {pattern} COLUMNS (b.id AS B) OPTIONS (PATH_ALGO({hint})))");
        let (code, msg) = refused(&s, &q);
        assert_eq!(code, -103, "{hint}: {msg}");
        assert!(msg.contains("GRF-0070") && msg.contains(&format!("PATH_ALGO({hint})")), "{msg}");
    }
    let q = "SELECT B FROM GRAPH_TABLE (SOCIAL MATCH ANY CHEAPEST (a WHERE a.id = 1)-[k:KNOWS]->+(b WHERE b.id = 10) COLUMNS (b.id AS B) \
             OPTIONS (PATH_ALGO(DIJKSTRA)))";
    assert_eq!(rows(&s, q), ["10"]);
}
