//! The conformance tests of the standard (§23.2), T1 to T12, on the bank
//! fixture (§23.1), each with the rows it gives.

use super::{bank, refused, rows};

/// The path of T1 to T4 from Dave to Aretha, under `prefix`, each match
/// its transfers in order.
fn dave_to_aretha(prefix: &str) -> String {
    format!(
        "SELECT T FROM GRAPH_TABLE (BANK MATCH {prefix} p = (a WHERE a.owner = 'Dave')-[t:Transfer]->*(b WHERE b.owner = 'Aretha') \
         COLUMNS (ARRAY_AGG(t.id) AS T))"
    )
}

/// Verifies: [EXTENSION] [GS §23.2 T1] [P1 §5.2] TRAIL keeps the three
/// paths from Dave to Aretha that repeat no edge.
#[test]
fn t1_trail_paths_23_2() {
    let (_db, s) = bank();
    assert_eq!(
        rows(&s, &dave_to_aretha("TRAIL")),
        ["ARRAY['t5', 't2']", "ARRAY['t5', 't7', 't8', 't1', 't2']", "ARRAY['t6', 't8', 't1', 't2']"]
    );
}

/// Verifies: [EXTENSION] [GS §23.2 T2] [P1 §5.2] ACYCLIC keeps the first
/// two alone, as the third passes a3 twice.
#[test]
fn t2_acyclic_paths_23_2() {
    let (_db, s) = bank();
    assert_eq!(rows(&s, &dave_to_aretha("ACYCLIC")), ["ARRAY['t5', 't2']", "ARRAY['t6', 't8', 't1', 't2']"]);
}

/// Verifies: [EXTENSION] [GS §23.2 T3] [P1 §5.1] ANY SHORTEST of no
/// restrictor keeps the shortest.
#[test]
fn t3_any_shortest_23_2() {
    let (_db, s) = bank();
    assert_eq!(rows(&s, &dave_to_aretha("ANY SHORTEST")), ["ARRAY['t5', 't2']"]);
}

/// Verifies: [EXTENSION] [GS §23.2 T4] [P1 §5.3] WALK and no selector
/// over an unbounded quantifier is refused, GRF-0031.
#[test]
fn t4_walk_refused_23_2() {
    let (_db, s) = bank();
    let (code, msg) = refused(&s, &dave_to_aretha("WALK"));
    assert_eq!(code, -103, "{msg}");
    assert!(msg.contains("GRF-0031"), "{msg}");
}

/// Verifies: [EXTENSION] [GS §23.2 T5, T6] [P1 §4.5] a union of node
/// patterns binds Ankh-Morpork, a city and a country, once; a multiset
/// alternation once for each alternative.
#[test]
fn t5_t6_union_and_alternation_23_2() {
    let (_db, s) = bank();
    let union = "SELECT N FROM GRAPH_TABLE (BANK MATCH (c:City) | (c:Country) COLUMNS (c.name AS N))";
    assert_eq!(rows(&s, union), ["Ankh-Morpork", "Zembla"]);
    let alternation = "SELECT N FROM GRAPH_TABLE (BANK MATCH (c:City) |+| (c:Country) COLUMNS (c.name AS N))";
    assert_eq!(rows(&s, alternation), ["Ankh-Morpork", "Ankh-Morpork", "Zembla"]);
}

/// Verifies: [EXTENSION] [GS §23.2 T7] [P1 §4.4] a prefilter on the
/// middle vertex is tested as the path is searched: of ALL SHORTEST, the
/// path through Jay, the blocked account. The fixture's t6 from Dave to
/// Charles makes it 5 edges, t1 t2 t3 t4 t6, one fewer than the path the
/// standard lists, which takes t5 and t7 instead (CONFORMANCE.md).
#[test]
fn t7_prefilter_23_2() {
    let (_db, s) = bank();
    let q = "SELECT Q, T FROM GRAPH_TABLE (BANK MATCH ALL SHORTEST (p WHERE owner = 'Scott')-[s]->+(q WHERE is_blocked = 'yes')\
             -[t]->+(r WHERE owner = 'Charles') COLUMNS (q.id AS Q, ARRAY_AGG(s.id) || ARRAY_AGG(t.id) AS T))";
    assert_eq!(rows(&s, q), ["a4,ARRAY['t1', 't2', 't3', 't4', 't6']"]);
}

/// Verifies: [EXTENSION] [GS §23.2 T8] [P1 §4.4] the same condition as a
/// postfilter is tested after the selector, which kept the shortest path,
/// not through Jay: no rows.
#[test]
fn t8_postfilter_23_2() {
    let (_db, s) = bank();
    let q = "SELECT Q FROM GRAPH_TABLE (BANK MATCH ALL SHORTEST (p WHERE owner = 'Scott')->+(q)->+(r WHERE owner = 'Charles') \
             WHERE q.is_blocked = 'yes' COLUMNS (q.id AS Q))";
    assert_eq!(rows(&s, q), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [GS §23.2 T9] [P1 §4.5, §5.2] Jay's loops of
/// transfers above 5 million back to Jay, then where Jay is: of a union,
/// one binding of each loop; of a multiset alternation, two, as
/// Ankh-Morpork is a city and a country.
#[test]
fn t9_loops_23_2() {
    let (_db, s) = bank();
    let of = |op: &str| {
        format!(
            "SELECT N, P FROM GRAPH_TABLE (BANK MATCH TRAIL x = (a WHERE a.owner = 'Jay') [-[b:Transfer WHERE b.amount > 5]->]+ (a) \
             [-[:isLocatedIn]->(c:City) {op} -[:isLocatedIn]->(c:Country)] COLUMNS (c.id AS P, COUNT(b.*) AS N))"
        )
    };
    assert_eq!(rows(&s, &of("|")), ["4,c2", "7,c2"]);
    assert_eq!(rows(&s, &of("|+|")), ["4,c2", "4,c2", "7,c2", "7,c2"]);
}

/// Verifies: [EXTENSION] [GS §23.2 T10] [P1 §4.1] undirected edges, and a
/// variable repeated, joined on itself: the transfers between two
/// accounts of one phone.
#[test]
fn t10_shared_phone_23_2() {
    let (_db, s) = bank();
    let q = "SELECT P, S, T, D FROM GRAPH_TABLE (BANK MATCH (p:Phone)~[:hasPhone]~(s:Account)-[t:Transfer]->(d:Account)~[:hasPhone]~(p) \
             COLUMNS (p.id AS P, s.id AS S, t.id AS T, d.id AS D))";
    assert_eq!(rows(&s, q), ["p1,a5,t8,a1", "p2,a3,t2,a2"]);
}

/// Verifies: [EXTENSION] [GS §23.2 T11] [P1 §4.1] an edge pattern of any
/// orientation matches a directed edge each way.
#[test]
fn t11_any_orientation_23_2() {
    let (_db, s) = bank();
    let q = "SELECT X, Y FROM GRAPH_TABLE (BANK MATCH (x)-[e:Transfer WHERE e.id = 't1']-(y) COLUMNS (x.id AS X, y.id AS Y))";
    assert_eq!(rows(&s, q), ["a1,a3", "a3,a1"]);
}

/// Verifies: [EXTENSION] [GS §23.2 T12] [GS §7.4 R7.4] a prefilter
/// aggregating a group variable unbounded where it reads it is refused,
/// GRF-0032; as a postfilter it is tested of each path, here of none.
#[test]
fn t12_unbounded_aggregate_23_2() {
    let (_db, s) = bank();
    let bracketed =
        "SELECT 1 FROM GRAPH_TABLE (BANK MATCH ALL SHORTEST [(x)-[e]->*(y) WHERE COUNT(e)/(COUNT(e)+1) > 1] COLUMNS (x.id AS X))";
    let (code, msg) = refused(&s, bracketed);
    assert_eq!(code, -103, "{msg}");
    assert!(msg.contains("GRF-0032"), "{msg}");
    let post = "SELECT 1 FROM GRAPH_TABLE (BANK MATCH ALL SHORTEST (x)-[e]->*(y) WHERE COUNT(e)/(COUNT(e)+1) > 1 COLUMNS (x.id AS X))";
    assert_eq!(rows(&s, post), Vec::<String>::new());
}
