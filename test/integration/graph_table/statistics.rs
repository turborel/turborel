//! A graph's statistics (RULES.md D117): INFORMATION_SCHEMA.GRAPH_STATISTICS,
//! gathered as its tables' statistics are and gathered again once a
//! database reopened asks for them; the planner's estimate of a probe of
//! an edge table by its endpoint columns; and the end a GRAPH_TABLE's
//! search starts from, by the statistics and by the hints GRAPH_START and
//! GRAPH_DIRECTION, each way giving the same rows; and a search's estimate
//! of a step, by the label transitions.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;

use super::rows;
use crate::common::{Scratch, os};

/// Nodes 1 to 8, the first six of group 0; links from each of them to 7,
/// from 7 to 8, and from 1 to 2 and 2 to 3: out-degrees 2, 2, 1, 1, 1, 1
/// and 1, in-degrees 6 (of node 7), 1, 1 and 1.
const FIXTURE: &[&str] = &[
    "CREATE TABLE N (ID INTEGER PRIMARY KEY, GRP INTEGER NOT NULL)",
    "CREATE TABLE E (ID INTEGER PRIMARY KEY, S INTEGER NOT NULL REFERENCES N, D INTEGER NOT NULL REFERENCES N, W INTEGER)",
    "INSERT INTO N VALUES (1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (6, 0), (7, 1), (8, 1)",
    "INSERT INTO E VALUES (1, 1, 7, 1), (2, 2, 7, 1), (3, 3, 7, 2), (4, 4, 7, 2), (5, 5, 7, NULL), (6, 6, 7, 3), \
     (7, 7, 8, 3), (8, 1, 2, 4), (9, 2, 3, 4)",
    "CREATE PROPERTY GRAPH G \
     VERTEX TABLES (N LABEL NODE PROPERTIES (ID, GRP)) \
     EDGE TABLES (E SOURCE KEY (S) REFERENCES N (ID) DESTINATION KEY (D) REFERENCES N (ID) \
       LABEL LINK PROPERTIES (ID, W, W * 2 AS W2))",
    "COMMIT WORK",
];

/// The fixture in `db`, its background work done only when asked; a
/// session of A.
fn fixture(db: &Database) -> Session {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    for sql in FIXTURE {
        s.run(sql).unwrap();
    }
    s
}

/// Has the statistics of the fixture's tables gathered now.
fn gather(db: &Database) {
    db.analyze("A.N").unwrap();
    db.analyze("A.E").unwrap();
}

/// The rows of GRAPH_STATISTICS of graph G, in `s`.
fn statistics(s: &Session) -> Vec<String> {
    let q = "SELECT STATISTIC_KIND, ELEMENT_TABLE_ALIAS, PROPERTY_NAME, SOURCE_LABEL, EDGE_LABEL, DESTINATION_LABEL, \
             ROW_COUNT, DISTINCT_VALUES, NULL_FRACTION, OUT_DEGREE_AVERAGE, OUT_DEGREE_MEDIAN, OUT_DEGREE_P90, \
             OUT_DEGREE_MAXIMUM, IN_DEGREE_AVERAGE, IN_DEGREE_MEDIAN, IN_DEGREE_MAXIMUM \
             FROM INFORMATION_SCHEMA.GRAPH_STATISTICS WHERE PROPERTY_GRAPH_NAME = 'G'";
    let out = rows(s, q);
    s.run("COMMIT WORK").unwrap();
    out
}

/// What GRAPH_STATISTICS shows of G once its tables' statistics are
/// gathered.
const GATHERED: &[&str] = &[
    "EDGE,E,NULL,NULL,NULL,NULL,9.00,NULL,NULL,1.28,1.00,2.00,2.00,2.25,1.00,6.00",
    "PROPERTY,E,ID,NULL,NULL,NULL,NULL,9.00,0.00,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
    "PROPERTY,E,W,NULL,NULL,NULL,NULL,4.00,0.11,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
    "PROPERTY,E,W2,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
    "PROPERTY,N,GRP,NULL,NULL,NULL,NULL,2.00,0.00,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
    "PROPERTY,N,ID,NULL,NULL,NULL,NULL,8.00,0.00,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
    "TRANSITION,NULL,NULL,NODE,LINK,NODE,9.00,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
    "VERTEX,N,NULL,NULL,NULL,NULL,8.00,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
];

/// Verifies: [EXTENSION] RULES.md D117: GRAPH_STATISTICS shows a graph's
/// element tables, its properties and its label transitions, every
/// statistic NULL until its tables' statistics are gathered; then each
/// table's rows, an edge table's out-degree and in-degree distribution,
/// a property's distinct values and share NULL (none of one that is an
/// expression), and the edges of each transition; a user holding no
/// SELECT on the graph sees none of it.
#[test]
fn graph_statistics_show_degrees_properties_and_transitions() {
    let db = Database::new();
    let s = fixture(&db);
    let unknown = statistics(&s);
    assert_eq!(unknown.len(), 7, "{unknown:?}");
    assert!(unknown.iter().all(|r| r.split(',').skip(6).all(|v| v == "NULL")), "{unknown:?}");
    gather(&db);
    assert_eq!(statistics(&s), GATHERED);
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    assert_eq!(statistics(&db.session("B")), Vec::<String>::new());
}

/// The database in `dir`, its background work done only when asked.
fn opened(dir: &Path) -> Database {
    let db = Database::new();
    db.open_files(&os(), dir, &OpenOptions::default()).unwrap();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D117, D25: a graph's statistics are a
/// database's again once it reopens and its maintenance gathers its
/// tables' statistics, due as they are first asked for; and follow its
/// changes once committed.
#[test]
fn graph_statistics_are_recovered_after_reopening() {
    let scratch = Scratch::new("graph-statistics-reopened");
    let dir = scratch.path("db");
    let db = opened(&dir);
    let s = fixture(&db);
    gather(&db);
    assert_eq!(statistics(&s), GATHERED);
    db.close().unwrap();
    drop((s, db));
    let db = opened(&dir);
    let s = db.session("A");
    assert!(statistics(&s).iter().all(|r| r.split(',').skip(6).all(|v| v == "NULL")), "none before maintenance");
    db.maintain().unwrap();
    assert_eq!(statistics(&s), GATHERED, "gathered again");
    s.run("INSERT INTO E VALUES (10, 8, 7, 5), (11, 3, 7, 5)").unwrap();
    s.run("COMMIT WORK").unwrap();
    db.analyze("A.E").unwrap();
    let edge = statistics(&s).into_iter().find(|r| r.starts_with("EDGE")).unwrap();
    assert_eq!(edge, "EDGE,E,NULL,NULL,NULL,NULL,11.00,NULL,NULL,1.37,1.00,2.00,2.00,2.75,1.00,8.00");
}

/// The estimated rows each probe of the Expand of `sql`'s plan finds, in
/// `s`.
fn each(s: &Session, sql: &str) -> String {
    let plan = s.physical_plan(sql, &Params::new()).unwrap();
    let line = plan.lines().find(|l| l.contains("Expand")).unwrap_or_else(|| panic!("{plan}"));
    line.split("est. rows each=").nth(1).and_then(|r| r.split(',').next()).unwrap().to_owned()
}

/// Verifies: [EXTENSION] RULES.md D117: the planner estimates the rows a
/// probe of an edge table's adjacency index by its source column finds as
/// the average out-degree its graph's statistics know, once gathered, in
/// place of its guess: 800 links of 200 nodes, 4 of each.
#[test]
fn explain_shows_the_fan_out_of_an_edge_table() {
    let db = Database::new();
    let s = fixture(&db);
    let nodes: Vec<String> = (9..=200).map(|i| format!("({i}, {})", 1 + i % 50)).collect();
    s.run(&format!("INSERT INTO N VALUES {}", nodes.join(", "))).unwrap();
    let links: Vec<String> = (10..=800).map(|i| format!("({i}, {}, {}, NULL)", 1 + i % 200, 1 + (i * 7) % 200)).collect();
    s.run(&format!("INSERT INTO E VALUES {}", links.join(", "))).unwrap();
    s.run("COMMIT WORK").unwrap();
    let q = "SELECT N.ID, E.ID FROM N, E WHERE N.GRP = 0 AND E.S = N.ID";
    assert_eq!(each(&s, q), "8", "a guess");
    gather(&db);
    assert_eq!(each(&s, q), "4", "the average out-degree");
}

/// Of each node of group 0, the paths of one to three links to node 8,
/// in `options`.
fn to_eight(options: &str) -> String {
    format!(
        "SELECT A, B, N, J FROM GRAPH_TABLE (G MATCH p = (a WHERE a.GRP = 0)-[e]->{{1,3}}(b WHERE b.ID = 8) \
         COLUMNS (a.ID AS A, b.ID AS B, PATH_LENGTH(p) AS N, JSON_SERIALIZE(PATH_TO_JSON(p)) AS J) {options})"
    )
}

/// The SQLSTATE `query` fails with in `s`, or "00000" if it does not.
fn outcome(s: &Session, query: &str) -> String {
    let got = s.open_cursor(query).and_then(|mut c| {
        while c.fetch()?.is_some() {}
        Ok(())
    });
    got.map_or_else(|e| e.sqlstate().as_str().to_owned(), |()| "00000".to_owned())
}

/// Verifies: [EXTENSION] RULES.md D117, [GS §16 R16.8]: a search whose
/// both ends are filtered starts, once statistics are gathered, from the
/// end of fewer candidates times fan-out, here its last vertex, and finds
/// the same rows, each path in its order, as from its first; so do the
/// hints GRAPH_START of either end and GRAPH_DIRECTION of its edge
/// variable; ANY SHORTEST, whose path depends on the order of the
/// search, is searched from its first vertex.
#[test]
fn a_search_starts_from_the_cheaper_end_with_the_same_rows() {
    let db = Database::new();
    let s = fixture(&db);
    let forward = rows(&s, &to_eight(""));
    assert_eq!(forward.len(), 8, "{forward:?}");
    for hint in ["OPTIONS (GRAPH_START(a))", "OPTIONS (GRAPH_START(b))", "OPTIONS (GRAPH_DIRECTION(e, REVERSE))"] {
        assert_eq!(rows(&s, &to_eight(hint)), forward, "{hint}");
    }
    gather(&db);
    assert_eq!(rows(&s, &to_eight("")), forward, "searched from b");
    assert_eq!(rows(&s, &to_eight("OPTIONS (GRAPH_START(a))")), forward);
    let shortest = to_eight("").replace("MATCH p", "MATCH ANY SHORTEST p");
    assert_eq!(rows(&s, &shortest).len(), 6);
}

/// Verifies: [EXTENSION] RULES.md D117: which end a search starts from is
/// seen in the states it makes: from node 8 the search fits a
/// MAX_SEARCH_STATES that from the six nodes of group 0 does not, once
/// statistics are gathered or GRAPH_START names b, and not if GRAPH_START
/// names a.
#[test]
fn the_end_chosen_makes_fewer_states() {
    let db = Database::new();
    let s = fixture(&db);
    let limit = "OPTIONS (MAX_SEARCH_STATES 20)";
    assert_eq!(outcome(&s, &to_eight(limit)), "54061");
    assert_eq!(outcome(&s, &to_eight("OPTIONS (MAX_SEARCH_STATES 20, GRAPH_START(b))")), "00000");
    gather(&db);
    assert_eq!(outcome(&s, &to_eight(limit)), "00000");
    assert_eq!(outcome(&s, &to_eight("OPTIONS (MAX_SEARCH_STATES 20, GRAPH_START(a))")), "54061");
}

/// Verifies: [EXTENSION] [GS §15, §17] RULES.md D118: a search's estimate
/// of a step follows the edges its labels match, as the graph's label
/// transitions count them: of 40 KNOWS edges among 80 people beside 800
/// LIKES edges, ANY SHORTEST over KNOWS from every person is BFS from
/// each, of the rows MS-BFS gives, and over LIKES, any edge or either
/// label, MS-BFS.
#[test]
fn a_step_is_estimated_by_its_label_transitions() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    let people: Vec<String> = (0..80).map(|i| format!("({i})")).collect();
    let knows: Vec<String> = (0..40).map(|i| format!("({i}, {i}, {})", i + 1)).collect();
    let likes: Vec<String> = (0..800).map(|i| format!("({i}, {}, {})", i % 80, (i * 7) % 80)).collect();
    for sql in [
        "CREATE TABLE P (ID INTEGER PRIMARY KEY)".to_owned(),
        "CREATE TABLE K (ID INTEGER PRIMARY KEY, S INTEGER NOT NULL REFERENCES P, D INTEGER NOT NULL REFERENCES P)".to_owned(),
        "CREATE TABLE L (ID INTEGER PRIMARY KEY, S INTEGER NOT NULL REFERENCES P, D INTEGER NOT NULL REFERENCES P)".to_owned(),
        format!("INSERT INTO P VALUES {}", people.join(", ")),
        format!("INSERT INTO K VALUES {}", knows.join(", ")),
        format!("INSERT INTO L VALUES {}", likes.join(", ")),
        "CREATE PROPERTY GRAPH SN VERTEX TABLES (P LABEL PERSON) EDGE TABLES ( \
         K SOURCE KEY (S) REFERENCES P (ID) DESTINATION KEY (D) REFERENCES P (ID) LABEL KNOWS, \
         L SOURCE KEY (S) REFERENCES P (ID) DESTINATION KEY (D) REFERENCES P (ID) LABEL LIKES)"
            .to_owned(),
        "COMMIT WORK".to_owned(),
    ] {
        s.run(&sql).unwrap();
    }
    for table in ["A.P", "A.K", "A.L"] {
        db.analyze(table).unwrap();
    }
    let q = |edge: &str| format!("SELECT * FROM GRAPH_TABLE (SN MATCH ANY SHORTEST (a)-[{edge}]->*(b) COLUMNS (a.id AS A, b.id AS B)");
    let shown = s.explain(&format!("{})", q(":KNOWS")), &Params::new()).unwrap();
    assert!(shown.contains("PathSearch-BFS by cost: 80 starts, est. reach 1 of 80"), "{shown}");
    for edge in [":LIKES", "", ":KNOWS|LIKES"] {
        let shown = s.explain(&format!("{})", q(edge)), &Params::new()).unwrap();
        assert!(shown.contains("MS-BFS by cost: 80 starts, est. reach 80 of 80"), "{edge}: {shown}");
    }
    let forced = format!("{} OPTIONS (PATH_ALGO(MSBFS)))", q(":KNOWS"));
    assert_eq!(rows(&s, &format!("{})", q(":KNOWS"))), rows(&s, &forced));
}

/// Verifies: [EXTENSION] [GS §15, §17] RULES.md D118: a step's estimate
/// counts the label transitions of its edge's labels between the labels of
/// the node patterns beside it: of LINK edges, 40 between people and 800
/// from people to cities, ANY SHORTEST over LINK from person to person is
/// BFS from each start, and from person to city MS-BFS.
#[test]
fn a_step_is_estimated_by_its_endpoint_labels() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    let ids: Vec<String> = (0..80).map(|i| format!("({i})")).collect();
    let knows: Vec<String> = (0..40).map(|i| format!("({i}, {i}, {})", i + 1)).collect();
    let lives: Vec<String> = (0..800).map(|i| format!("({i}, {}, {})", i % 80, (i * 7) % 80)).collect();
    for sql in [
        "CREATE TABLE P (ID INTEGER PRIMARY KEY)".to_owned(),
        "CREATE TABLE C (ID INTEGER PRIMARY KEY)".to_owned(),
        "CREATE TABLE K (ID INTEGER PRIMARY KEY, S INTEGER NOT NULL REFERENCES P, D INTEGER NOT NULL REFERENCES P)".to_owned(),
        "CREATE TABLE L (ID INTEGER PRIMARY KEY, S INTEGER NOT NULL REFERENCES P, D INTEGER NOT NULL REFERENCES C)".to_owned(),
        format!("INSERT INTO P VALUES {}", ids.join(", ")),
        format!("INSERT INTO C VALUES {}", ids.join(", ")),
        format!("INSERT INTO K VALUES {}", knows.join(", ")),
        format!("INSERT INTO L VALUES {}", lives.join(", ")),
        "CREATE PROPERTY GRAPH SN VERTEX TABLES (P LABEL PERSON, C LABEL CITY) EDGE TABLES ( \
         K SOURCE KEY (S) REFERENCES P (ID) DESTINATION KEY (D) REFERENCES P (ID) LABEL LINK, \
         L SOURCE KEY (S) REFERENCES P (ID) DESTINATION KEY (D) REFERENCES C (ID) LABEL LINK)"
            .to_owned(),
        "COMMIT WORK".to_owned(),
    ] {
        s.run(&sql).unwrap();
    }
    for table in ["A.P", "A.C", "A.K", "A.L"] {
        db.analyze(table).unwrap();
    }
    let q =
        |to: &str| format!("SELECT * FROM GRAPH_TABLE (SN MATCH ANY SHORTEST (a)((x:PERSON)-[:LINK]->(y:{to}))*(b) COLUMNS (b.id AS B))");
    let shown = s.explain(&q("PERSON"), &Params::new()).unwrap();
    assert!(shown.contains("PathSearch-BFS by cost: 160 starts, est. reach 1 of 160"), "{shown}");
    let shown = s.explain(&q("CITY"), &Params::new()).unwrap();
    assert!(shown.contains("MS-BFS by cost: 160 starts"), "{shown}");
}
