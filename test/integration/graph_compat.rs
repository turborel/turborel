//! The compatibility syntax of property graphs [GS §10]: MATCH in a WHERE
//! clause over node and edge tables or a graph's tables, with
//! SHORTEST_PATH, FOR PATH, WITHIN GROUP (GRAPH PATH) and LAST_NODE
//! (§10.1); the identity functions and predicates, JSON properties' dot
//! notation and hints (§10.2, R5.1); and PGQL and Cypher queries (§10.3).
//! The modules below hold §10.2 and §10.3.

use turborel::db::{Database, Session};

mod cypher_clauses;
mod front_ends;
mod identity;
mod match_forms;
mod reachability;

/// Schema A of people as node tables and who befriends whom, Alice to
/// Bob to Carol to Dave and Alice to Erin to Dave, as §10.1's samples
/// read them.
const PEOPLE: &[&str] = &[
    "CREATE TABLE PERSON (ID INTEGER, NAME VARCHAR(10)) AS NODE",
    "CREATE TABLE FRIEND_OF (SINCE INTEGER) AS EDGE",
    "INSERT INTO PERSON VALUES (1, 'Alice'), (2, 'Bob'), (3, 'Carol'), (4, 'Dave'), (5, 'Erin')",
    "INSERT INTO FRIEND_OF ($FROM_ID, $TO_ID, SINCE) SELECT P.$NODE_ID, Q.$NODE_ID, 2000 + P.ID * 10 + Q.ID \
     FROM PERSON P, PERSON Q WHERE (P.ID, Q.ID) IN (VALUES (1, 2), (2, 3), (3, 4), (1, 5), (5, 4))",
];

/// The tables of a graph CHAIN of no node tables, the same friendships
/// keyed by ID.
const CHAIN: &[&str] = &[
    "CREATE TABLE MEMBER (ID INTEGER PRIMARY KEY, NAME VARCHAR(10))",
    "CREATE TABLE FOLLOWS (ID INTEGER PRIMARY KEY, SRC INTEGER REFERENCES MEMBER, DST INTEGER REFERENCES MEMBER)",
    "INSERT INTO MEMBER VALUES (1, 'Alice'), (2, 'Bob'), (3, 'Carol'), (4, 'Dave'), (5, 'Erin')",
    "INSERT INTO FOLLOWS VALUES (12, 1, 2), (23, 2, 3), (34, 3, 4), (15, 1, 5), (54, 5, 4)",
    "CREATE PROPERTY GRAPH CHAIN VERTEX TABLES (MEMBER LABEL MEMBER PROPERTIES (ID, NAME)) \
     EDGE TABLES (FOLLOWS SOURCE KEY (SRC) REFERENCES MEMBER (ID) DESTINATION KEY (DST) REFERENCES MEMBER (ID) \
       LABEL FOLLOWS PROPERTIES (ID))",
];

/// A database of `PEOPLE` and `CHAIN`, and a session of A.
fn people() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    for sql in PEOPLE.iter().chain(CHAIN) {
        s.run(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    s.commit().unwrap();
    (db, s)
}

/// The rows of `query` in `s`, each its values joined by commas, sorted.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap_or_else(|e| panic!("{query}: {e:?}"));
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap_or_else(|e| panic!("{query}: {e:?}")) {
        out.push(row.iter().map(|v| v.to_string().trim().to_owned()).collect::<Vec<_>>().join(","));
    }
    out.sort();
    out
}

/// The SQLCODE `query` is refused with in `s`, and its message, which
/// holds `holds`.
fn refused(s: &Session, query: &str, holds: &str) -> i32 {
    let e = s.open_cursor(query).err().unwrap_or_else(|| panic!("{query}: accepted"));
    assert!(e.msg.contains(holds), "{query}: {} lacks {holds}", e.msg);
    e.sqlcode()
}

/// Verifies: [EXTENSION] [GS §10.1] (its first sample): MATCH of a pattern over node
/// and edge tables, with a condition of a table beside it.
#[test]
fn match_reads_node_tables_10_1() {
    let (_db, s) = people();
    let q = "SELECT p2.name FROM person p1, friend_of f, person p2 WHERE MATCH(p1-(f)->p2) AND p1.name = 'Alice'";
    assert_eq!(rows(&s, q), ["Bob", "Erin"]);
    let q = "SELECT p1.name, f.since, p3.name FROM person p1, friend_of f, person p2, friend_of g, person p3 \
             WHERE MATCH(p1-(f)->p2-(g)->p3) AND p2.name = 'Erin'";
    assert_eq!(rows(&s, q), ["Alice,2015,Dave"]);
    let q = "SELECT * FROM person p1, friend_of f, person p2 WHERE MATCH(p2<-(f)-p1) AND p2.name = 'Bob'";
    assert_eq!(rows(&s, q), ["1,Alice,2012,2,Bob"]);
}

/// Verifies: [EXTENSION] [GS §10.1] (its second sample): SHORTEST_PATH with FOR PATH
/// tables and STRING_AGG, LAST_VALUE and COUNT WITHIN GROUP (GRAPH PATH),
/// the path's elements in order; ANY SHORTEST from each start to each end.
#[test]
fn shortest_path_aggregates_its_path_10_1() {
    let (_db, s) = people();
    let q = "SELECT p1.name, STRING_AGG(p2.name, '->') WITHIN GROUP (GRAPH PATH) AS route, \
             LAST_VALUE(p2.name) WITHIN GROUP (GRAPH PATH) AS last_name, COUNT(p2.id) WITHIN GROUP (GRAPH PATH) AS hops \
             FROM person p1, friend_of FOR PATH f, person FOR PATH p2 WHERE MATCH(SHORTEST_PATH(p1(-(f)->p2)+))";
    let all = [
        "Alice,Bob,Bob,1",
        "Alice,Bob->Carol,Carol,2",
        "Alice,Erin,Erin,1",
        "Alice,Erin->Dave,Dave,2",
        "Bob,Carol,Carol,1",
        "Bob,Carol->Dave,Dave,2",
        "Carol,Dave,Dave,1",
        "Erin,Dave,Dave,1",
    ];
    assert_eq!(rows(&s, q), all);
    let bounded = "SELECT p1.name, LAST_VALUE(p2.name) WITHIN GROUP (GRAPH PATH) \
                   FROM person p1, friend_of FOR PATH f, person FOR PATH p2 WHERE MATCH(SHORTEST_PATH(p1(-(f)->p2){1,1})) AND p1.id = 1";
    assert_eq!(rows(&s, bounded), ["Alice,Bob", "Alice,Erin"]);
    let backward = "SELECT p1.name, STRING_AGG(p2.name, '<-') WITHIN GROUP (GRAPH PATH) \
                    FROM person p1, friend_of FOR PATH f, person FOR PATH p2 WHERE MATCH(SHORTEST_PATH((p2<-(f)-)+p1)) AND p1.name = 'Bob'";
    assert_eq!(rows(&s, backward), ["Bob,Carol", "Bob,Dave<-Carol"]);
}

/// Verifies: [EXTENSION] [GS §10.1]: LAST_NODE ( x ) = LAST_NODE ( y ) is SAME of the
/// two paths' last nodes, and LAST_NODE ( x ) starts a pattern there.
#[test]
fn last_node_joins_paths_10_1() {
    let (_db, s) = people();
    let q = "SELECT a.name, b.name, LAST_VALUE(x.name) WITHIN GROUP (GRAPH PATH) \
             FROM person a, person b, friend_of FOR PATH f, person FOR PATH x, friend_of FOR PATH g, person FOR PATH y \
             WHERE MATCH(SHORTEST_PATH(a(-(f)->x)+) AND SHORTEST_PATH(b(-(g)->y)+) AND LAST_NODE(x) = LAST_NODE(y)) \
             AND a.name = 'Bob' AND b.name = 'Erin'";
    assert_eq!(rows(&s, q), ["Bob,Erin,Dave"]);
    let q = "SELECT a.name, c.name FROM person a, friend_of FOR PATH f, person FOR PATH x, friend_of g, person c \
             WHERE MATCH(SHORTEST_PATH(a(-(f)->x)+) AND LAST_NODE(x)-(g)->c) AND a.name = 'Bob'";
    assert_eq!(rows(&s, q), ["Bob,Dave"]);
}

/// Verifies: [EXTENSION] [GS §10.1]: OR and NOT around MATCH, an edge alias written
/// twice, and a quantifier outside SHORTEST_PATH.
#[test]
fn match_under_or_and_not_10_1() {
    let (_db, s) = people();
    let either = "SELECT DISTINCT p.name, q.name FROM person p, person q, friend_of f \
                  WHERE (MATCH(p-(f)->q) OR MATCH(q-(f)->p)) AND p.name = 'Bob'";
    assert_eq!(rows(&s, either), ["Bob,Alice", "Bob,Carol"]);
    let none = "SELECT COUNT(*) FROM person p, friend_of f, person q WHERE p.name = 'Carol' AND NOT MATCH(p-(f)->q)";
    assert_eq!(rows(&s, none), ["24"]);
    let twice = "SELECT COUNT(*) FROM person a, friend_of f, person b, person c WHERE MATCH(a-(f)->b AND c-(f)->b)";
    assert_eq!(rows(&s, twice), ["5"]);
    let twice = "SELECT COUNT(*) FROM person a, friend_of f, person b, person c WHERE MATCH(a-(f)->b AND c-(f)->b) AND a.id <> c.id";
    assert_eq!(rows(&s, twice), ["0"]);
    let two = "SELECT p1.name, STRING_AGG(p2.name, ',') WITHIN GROUP (GRAPH PATH) \
               FROM person p1, friend_of FOR PATH f, person FOR PATH p2 WHERE MATCH(p1(-(f)->p2){2})";
    assert_eq!(rows(&s, two), ["Alice,Bob,Carol", "Alice,Erin,Dave", "Bob,Carol,Dave"]);
}

/// Verifies: [EXTENSION] [GS §10.1]: MATCH over the tables of a graph of no node
/// tables, the graph its tables are of.
#[test]
fn match_reads_a_graphs_tables_10_1() {
    let (_db, s) = people();
    let q = "SELECT b.name, f.id FROM member a, follows f, member b WHERE MATCH(a-(f)->b) AND a.name = 'Alice'";
    assert_eq!(rows(&s, q), ["Bob,12", "Erin,15"]);
    let q = "SELECT a.name, STRING_AGG(b.name, '->') WITHIN GROUP (GRAPH PATH) \
             FROM member a, follows FOR PATH f, member FOR PATH b WHERE MATCH(SHORTEST_PATH(a(-(f)->b)+)) AND a.id = 2";
    assert_eq!(rows(&s, q), ["Bob,Carol", "Bob,Carol->Dave"]);
}

/// Verifies: [EXTENSION] [GS §10.1]: what a MATCH may not do, each refused with
/// SQLCODE -103.
#[test]
fn match_refusals_10_1() {
    let (_db, s) = people();
    let cases = [
        ("SELECT 1 FROM person p, friend_of f WHERE MATCH(p-(f)->q)", "no table of the FROM clause"),
        ("SELECT 1 FROM person p, follows f, member q WHERE MATCH(p-(f)->q)", "no one property graph"),
        ("SELECT 1 FROM person p JOIN friend_of f ON MATCH(p-(f)->p)", "WHERE clause alone"),
        ("SELECT COUNT(*) FROM person p, friend_of f, person q GROUP BY p.id HAVING MATCH(p-(f)->q)", "WHERE clause alone"),
        ("SELECT 1 FROM person p, friend_of FOR PATH f, person q", "a MATCH that is a conjunct"),
        (
            "SELECT 1 FROM person p, friend_of FOR PATH f, person FOR PATH q WHERE p.id = 1 OR MATCH(SHORTEST_PATH(p(-(f)->q)+))",
            "a MATCH that is a conjunct",
        ),
    ];
    for (q, holds) in cases {
        assert_eq!(refused(&s, q, holds), -103, "{q}");
    }
    s.run(
        "CREATE PROPERTY GRAPH TWIN VERTEX TABLES (MEMBER LABEL TWIN_MEMBER) EDGE TABLES (FOLLOWS SOURCE KEY (SRC) \
           REFERENCES MEMBER (ID) DESTINATION KEY (DST) REFERENCES MEMBER (ID) LABEL TWIN_FOLLOWS)",
    )
    .unwrap();
    let q = "SELECT 1 FROM member a, follows f, member b WHERE MATCH(a-(f)->b)";
    assert_eq!(refused(&s, q, "several property graphs"), -103);
}
