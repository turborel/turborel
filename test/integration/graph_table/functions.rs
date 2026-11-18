//! The element and path functions, graph predicates and set functions of
//! group variables a GRAPH_TABLE's expressions may hold [GS §5, §6.9].

use super::{bank, rows};

/// Verifies: [EXTENSION] [GS §5 R5.1] ELEMENT_ID is an element's table
/// and key as canonical JSON text; VERTEX_ID and EDGE_ID the same as a
/// JSON value.
#[test]
fn element_identities_5() {
    let (_db, s) = bank();
    let q = "SELECT I, V, E FROM GRAPH_TABLE (BANK MATCH (a)-[t:Transfer WHERE t.id = 't6']->(b) \
             COLUMNS (ELEMENT_ID(a) AS I, JSON_SERIALIZE(VERTEX_ID(b)) AS V, JSON_SERIALIZE(EDGE_ID(t)) AS E))";
    assert_eq!(rows(&s, q), [r#"{"table":"ACCOUNT","key":["a6"]},{"table":"ACCOUNT","key":["a5"]},{"table":"TRANSFER","key":["t6"]}"#]);
}

/// Verifies: [EXTENSION] [GS §5 R5.2] SAME of two variables bound to one
/// element, ALL_DIFFERENT of none bound to one: of the 11 paths of two
/// transfers, none returns to its start.
#[test]
fn same_and_all_different_5() {
    let (_db, s) = bank();
    let of = |w: &str| {
        format!(
            "SELECT COUNT(*) FROM GRAPH_TABLE (BANK MATCH (a:Account)-[:Transfer]->(b:Account)-[:Transfer]->(c:Account) WHERE {w} COLUMNS (a.id AS A))"
        )
    };
    assert_eq!(rows(&s, &of("ALL_DIFFERENT(a, b, c)")), ["11"]);
    assert_eq!(rows(&s, &of("SAME(a, c)")), ["0"]);
    assert_eq!(rows(&s, &of("NOT SAME(a, b)")), ["11"]);
}

/// Verifies: [EXTENSION] [GS §5 R5.3] IS SOURCE OF and IS DESTINATION OF
/// of a directed edge followed either way, and IS DIRECTED; an
/// undirected edge has no source.
#[test]
fn endpoints_and_direction_5() {
    let (_db, s) = bank();
    let of =
        |w: &str| format!("SELECT X FROM GRAPH_TABLE (BANK MATCH (x)-[t:Transfer WHERE t.id = 't1']-(y) WHERE {w} COLUMNS (x.id AS X))");
    assert_eq!(rows(&s, &of("x IS SOURCE OF t")), ["a1"]);
    assert_eq!(rows(&s, &of("x IS DESTINATION OF t")), ["a3"]);
    assert_eq!(rows(&s, &of("x IS NOT SOURCE OF t")), ["a3"]);
    assert_eq!(rows(&s, &of("t IS DIRECTED")), ["a1", "a3"]);
    let phone =
        "SELECT A FROM GRAPH_TABLE (BANK MATCH (a)~[h:HasPhone]~(p:Phone) WHERE a IS SOURCE OF h OR h IS DIRECTED COLUMNS (a.id AS A))";
    assert_eq!(rows(&s, phone), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [GS §5 R5.4, R5.5, §6.2] label expressions with
/// `&`, `!`, `|` and `%`, IS LABELED, PROPERTY_EXISTS and LABELS.
#[test]
fn labels_and_properties_5() {
    let (_db, s) = bank();
    let ids = |m: &str| rows(&s, &format!("SELECT I FROM GRAPH_TABLE (BANK MATCH {m} COLUMNS (v.id AS I))"));
    assert_eq!(ids("(v:Country&!City)"), ["c1"]);
    assert_eq!(ids("(v:City|Phone)"), ["c2", "p1", "p2"]);
    assert_eq!(ids("(v) WHERE v IS LABELED City"), ["c2"]);
    assert_eq!(ids("(v) WHERE v:Country AND v IS NOT LABELED City"), ["c1"]);
    assert_eq!(ids("(v) WHERE PROPERTY_EXISTS(v, NAME)"), ["c1", "c2"]);
    assert_eq!(ids("(v:%)").len(), 10);
    let labels = "SELECT L FROM GRAPH_TABLE (BANK MATCH (v:City) COLUMNS (LABELS(v) AS L))";
    assert_eq!(rows(&s, labels), ["ARRAY['CITY', 'COUNTRY']"]);
}

/// Verifies: [EXTENSION] [GS §6.9] PATH_LENGTH, VERTICES, EDGES and the
/// restrictor tests of each trail from Dave to Aretha; PATH_TO_JSON of
/// the shortest.
#[test]
fn path_functions_6_9() {
    let (_db, s) = bank();
    let q = "SELECT N, A, T, S, CARDINALITY(VERTICES(p)), CARDINALITY(EDGES(p)) FROM GRAPH_TABLE (BANK MATCH TRAIL p = \
             (a WHERE a.owner = 'Dave')-[t:Transfer]->*(b WHERE b.owner = 'Aretha') COLUMNS (PATH_LENGTH(p) AS N, IS_ACYCLIC(p) AS A, \
             IS_TRAIL(p) AS T, IS_SIMPLE(p) AS S, VERTICES(p) AS VERTICES_OF, EDGES(p) AS EDGES_OF)) AS G (N, A, T, S, VERTICES, EDGES)";
    let q = q.replace(" AS G (N, A, T, S, VERTICES, EDGES)", "");
    let q = q.replace("CARDINALITY(VERTICES(p)), CARDINALITY(EDGES(p))", "CARDINALITY(VERTICES_OF), CARDINALITY(EDGES_OF)");
    assert_eq!(rows(&s, &q), ["2,TRUE,TRUE,TRUE,3,2", "4,TRUE,TRUE,TRUE,5,4", "5,FALSE,TRUE,FALSE,6,5"]);
    let json = "SELECT J FROM GRAPH_TABLE (BANK MATCH ANY SHORTEST p = (a WHERE a.owner = 'Dave')-[t:Transfer]->*(b WHERE b.owner = 'Aretha') \
                COLUMNS (JSON_SERIALIZE(PATH_TO_JSON(p)) AS J))";
    assert_eq!(
        rows(&s, json),
        [
            r#"[{"table":"ACCOUNT","key":["a6"]},{"table":"TRANSFER","key":["t5"]},{"table":"ACCOUNT","key":["a3"]},{"table":"TRANSFER","key":["t2"]},{"table":"ACCOUNT","key":["a2"]}]"#
        ]
    );
}

/// Verifies: [EXTENSION] [P1 §4.4] set functions of a group variable's
/// elements, COUNT (t.*) among them, in COLUMNS and the postfilter.
#[test]
fn group_aggregates_4_4() {
    let (_db, s) = bank();
    let q = "SELECT C, S, L, H, D FROM GRAPH_TABLE (BANK MATCH TRAIL (a WHERE a.owner = 'Dave')-[t:Transfer]->*(b WHERE b.owner = 'Aretha') \
             WHERE SUM(t.amount) > 20 COLUMNS (COUNT(t.*) AS C, SUM(t.amount) AS S, MIN(t.amount) AS L, MAX(t.amount) AS H, \
             COUNT(DISTINCT t.amount) AS D))";
    assert_eq!(rows(&s, q), ["4,31,4,10,4", "5,43,6,10,4"]);
}

/// Verifies: [EXTENSION] [GS §8 R8.4] `v.*` gives every property of `v`,
/// named by each.
#[test]
fn all_properties_8() {
    let (_db, s) = bank();
    let q = "SELECT * FROM GRAPH_TABLE (BANK MATCH (v:City) COLUMNS (v.*))";
    assert_eq!(rows(&s, q), ["c2,Ankh-Morpork"]);
    let named = "SELECT NAME FROM GRAPH_TABLE (BANK MATCH (v:Country) COLUMNS (v.*))";
    assert_eq!(rows(&s, named), ["Ankh-Morpork", "Zembla"]);
}
