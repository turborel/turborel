//! Prefilters reading the path so far [GS §6.9], variables bound after
//! them, and variables of other path patterns [GS §6.8].

use super::{bank, refused, rows};

/// Verifies: [EXTENSION] [GS §6.9] a path function in a prefilter is of
/// the path so far, its element the last, of a path or a subpath
/// variable, and is tested before the selector; the path kept crosses no
/// unbounded quantifier outside every restrictor (GRF-0032).
#[test]
fn path_functions_in_a_prefilter_6_9() {
    let (_db, s) = bank();
    let at_end = "SELECT B FROM GRAPH_TABLE (BANK MATCH p = (a WHERE a.id = 'a1')-[t:Transfer]->{1,3}(b WHERE PATH_LENGTH(p) = 2) \
                  COLUMNS (b.id AS B))";
    let exact = "SELECT B FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1')-[t:Transfer]->{2}(b) COLUMNS (b.id AS B))";
    assert_eq!(rows(&s, at_end), rows(&s, exact));
    assert_eq!(rows(&s, exact), ["a2", "a5"]);
    let each = "SELECT Z, L FROM GRAPH_TABLE (BANK MATCH p = (a WHERE a.id = 'a1') [-[t:Transfer]->(m WHERE IS_ACYCLIC(p))]{1,4} (z) \
                COLUMNS (z.id AS Z, PATH_LENGTH(p) AS L))";
    let acyclic = "SELECT Z, L FROM GRAPH_TABLE (BANK MATCH ACYCLIC p = (a WHERE a.id = 'a1') [-[t:Transfer]->(m)]{1,4} (z) \
                   COLUMNS (z.id AS Z, PATH_LENGTH(p) AS L))";
    assert_eq!(rows(&s, each), rows(&s, acyclic));
    let sub = "SELECT B FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1')-[s:Transfer]->(m) \
               (q = (c)-[t:Transfer]->{1,3}(b WHERE PATH_LENGTH(q) = 1)) COLUMNS (b.id AS B))";
    let one = "SELECT B FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1')-[s:Transfer]->(m)-[t:Transfer]->(b) COLUMNS (b.id AS B))";
    assert_eq!(rows(&s, sub), rows(&s, one));
    let shortest = "SELECT B, L FROM GRAPH_TABLE (BANK MATCH ANY SHORTEST TRAIL p = (a WHERE a.id = 'a1')-[t:Transfer]->+\
                    (b WHERE PATH_LENGTH(p) >= 2) COLUMNS (b.id AS B, PATH_LENGTH(p) AS L))";
    assert!(rows(&s, shortest).contains(&"a3,5".to_owned()), "{:?}", rows(&s, shortest));
    let (code, msg) = refused(
        &s,
        "SELECT 1 FROM GRAPH_TABLE (BANK MATCH ANY SHORTEST p = (a)-[t:Transfer]->+(b WHERE PATH_LENGTH(p) > 1) COLUMNS (a.id AS A))",
    );
    assert!(code == -103 && msg.contains("GRF-0032"), "{code} {msg}");
}

/// Verifies: [EXTENSION] [GS §6.8] a prefilter within a quantifier that
/// reads a variable bound after it, in its iteration or after the
/// quantifier, is tested as that variable is bound, of each iteration's
/// element.
#[test]
fn prefilters_reading_later_variables_6_8() {
    let (_db, s) = bank();
    let later = "SELECT B FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1') [()-[t:Transfer WHERE y.id <> 'a5']->(y)]{1,3} (b) \
                 COLUMNS (b.id AS B))";
    let own = "SELECT B FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1') [()-[t:Transfer]->(y WHERE y.id <> 'a5')]{1,3} (b) \
               COLUMNS (b.id AS B))";
    assert_eq!(rows(&s, later), rows(&s, own));
    assert_eq!(rows(&s, own), ["a2", "a3", "a4"]);
    let after = "SELECT B, N FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a3') \
                 [-[t:Transfer WHERE t.amount > CHAR_LENGTH(b.owner)]->]{1,3} (b) COLUMNS (b.id AS B, COUNT(t.*) AS N))";
    let post = "SELECT B, N FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a3') [-[t:Transfer]->]{1,3} (b) \
                WHERE MIN(t.amount) > CHAR_LENGTH(b.owner) COLUMNS (b.id AS B, COUNT(t.*) AS N))";
    assert_eq!(rows(&s, after), rows(&s, post));
    assert_ne!(rows(&s, post), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [GS §6.8] a prefilter reading a variable of
/// another path pattern is tested with each binding of it, that pattern
/// searched first, before its own selector; two reading each other's are
/// refused.
#[test]
fn prefilters_reading_other_path_patterns_6_8() {
    let (_db, s) = bank();
    let q = "SELECT X, L FROM GRAPH_TABLE (BANK MATCH (x WHERE x.id IN ('a2', 'a3', 'a6')), \
             ANY SHORTEST (a WHERE a.id = 'a1') [-[t:Transfer]->(y WHERE y.id <> x.id)]+ (b WHERE b.id = 'a5') \
             COLUMNS (x.id AS X, COUNT(t.*) AS L))";
    assert_eq!(rows(&s, q), ["a2,2", "a6,2"]);
    let across = "SELECT X, B FROM GRAPH_TABLE (BANK MATCH (x WHERE x.owner IN ('Mike', 'Dave')), (a WHERE a.id = 'a3') \
                  [-[t:Transfer WHERE t.amount > CHAR_LENGTH(x.owner) + 2]->]{1,2} (b) COLUMNS (x.id AS X, b.id AS B))";
    let post = "SELECT X, B FROM GRAPH_TABLE (BANK MATCH (x WHERE x.owner IN ('Mike', 'Dave')), (a WHERE a.id = 'a3') \
                [-[t:Transfer]->]{1,2} (b) WHERE MIN(t.amount) > CHAR_LENGTH(x.owner) + 2 COLUMNS (x.id AS X, b.id AS B))";
    assert_eq!(rows(&s, across), rows(&s, post));
    assert_ne!(rows(&s, post), Vec::<String>::new());
    let (code, msg) = refused(
        &s,
        "SELECT 1 FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = b.id)-[t]->(c), (b WHERE b.id = a.id)-[u]->(d) \
         COLUMNS (a.id AS A))",
    );
    assert!(code == -103 && msg.contains("selector"), "{code} {msg}");
}

/// Verifies: [EXTENSION] [GS §11.2] EXISTS IN PATH p matches its pattern
/// along p's elements in path order, as FOR EACH SEGMENT of the opposite
/// condition (T22); its edges' directions are as p follows them; its
/// variables are its own.
#[test]
fn exists_in_path_11_2() {
    let (_db, s) = bank();
    let not_in = "SELECT B, L FROM GRAPH_TABLE (BANK MATCH p = (a WHERE a.id = 'a1')-[t:Transfer]->{1,4}(b) \
                  WHERE NOT EXISTS IN PATH p (-[u]->()-[v]-> WHERE u.amount >= v.amount) COLUMNS (b.id AS B, PATH_LENGTH(p) AS L))";
    let each = "SELECT B, L FROM GRAPH_TABLE (BANK MATCH p = (a WHERE a.id = 'a1')-[t:Transfer]->{1,4}(b) \
                FOR EACH SEGMENT (e1, e2) IN p WHERE e1.amount < e2.amount COLUMNS (b.id AS B, PATH_LENGTH(p) AS L))";
    assert_eq!(rows(&s, not_in), rows(&s, each));
    assert_eq!(rows(&s, each), ["a2,2", "a3,1"]);
    let through = "SELECT B FROM GRAPH_TABLE (BANK MATCH p = (a WHERE a.id = 'a3')-[t:Transfer]->{1,4}(b) \
                   WHERE EXISTS IN PATH p ((x WHERE x.id = 'a6')-[u]->(y WHERE y.id = 'a5')) COLUMNS (b.id AS B))";
    assert_eq!(rows(&s, through), ["a5"]);
    let backward = "SELECT COUNT(*) FROM GRAPH_TABLE (BANK MATCH p = (a)-[t:Transfer]->{1,2}(b) \
                    WHERE EXISTS IN PATH p (<-[u]-) COLUMNS (b.id AS B))";
    assert_eq!(rows(&s, backward), ["0"]);
    let (code, msg) =
        refused(&s, "SELECT 1 FROM GRAPH_TABLE (BANK MATCH p = (a)-[t]->(b) WHERE EXISTS IN PATH p ((a)) COLUMNS (b.id AS B))");
    assert!(code == -103 && msg.contains("its own"), "{code} {msg}");
}
