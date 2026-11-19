//! PREV, FIRST and FOR EACH SEGMENT [GS §11.1], and inverse distribution
//! functions of a group variable [P1 §4.4].

use super::{bank, refused, rows};

/// Verifies: [EXTENSION] [GS §11.1 R11.2] PREV of an edge variable in a
/// quantified pattern's WHERE is its binding in the iteration before,
/// null in the first: transfers each later than the one before.
#[test]
fn prev_is_the_iteration_before_11_1() {
    let (_db, s) = bank();
    let q = "SELECT B, N FROM GRAPH_TABLE (BANK MATCH TRAIL (a WHERE a.id = 'a3') \
             [-[t:Transfer]-> WHERE PREV(t) IS NULL OR t.when_at > PREV(t).when_at]{1,3} (b) \
             COLUMNS (b.id AS B, COUNT(t.*) AS N))";
    assert_eq!(rows(&s, q), ["a1,2", "a2,1", "a4,2", "a5,1", "a6,3"]);
    let q = "SELECT B FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a3') \
             [-[t:Transfer]-> WHERE PREV(t) IS NULL]{2} (b) COLUMNS (b.id AS B))";
    assert_eq!(rows(&s, q), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [GS §11.1] FIRST of an edge variable in a
/// quantified pattern's WHERE is its binding in the first iteration, its
/// own in the first.
#[test]
fn first_is_the_first_iteration_11_1() {
    let (_db, s) = bank();
    let q = "SELECT B FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1') \
             [-[t:Transfer]-> WHERE t.amount >= FIRST(t).amount]{1,3} (b) COLUMNS (b.id AS B))";
    assert_eq!(rows(&s, q), ["a2", "a3", "a4"]);
    let q = "SELECT B FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1') \
             [-[t:Transfer]-> WHERE FIRST(t) = ELEMENT_ID(t)]{1,2} (b) COLUMNS (b.id AS B))";
    assert_eq!(rows(&s, q), ["a3"]);
    let q = "SELECT B FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a6') \
             [-[t:Transfer]-> WHERE t.amount <= FIRST(t).amount]{1,3} (b) COLUMNS (b.id AS B))";
    assert_eq!(rows(&s, q), ["a1", "a2", "a3", "a4", "a5", "a5"]);
}

/// Verifies: [EXTENSION] [GS §11.1 R11.1] PREV and FIRST are of a
/// prefilter, and not nested (GRF-0041); of no property it is GRF-0012.
#[test]
fn prev_and_first_rules_11_1() {
    let (_db, s) = bank();
    let cases = [
        ("SELECT 1 FROM GRAPH_TABLE (BANK MATCH (a)-[t]->(b) COLUMNS (PREV(t) AS P))", "written in a prefilter"),
        ("SELECT 1 FROM GRAPH_TABLE (BANK MATCH (a) [-[t]-> WHERE PREV(PREV(t)) IS NULL]{1,2} (b) COLUMNS (a.id AS A))", "GRF-0041"),
        ("SELECT 1 FROM GRAPH_TABLE (BANK MATCH (a) [-[t]-> WHERE PREV(t).nothing IS NULL]{1,2} (b) COLUMNS (a.id AS A))", "GRF-0012"),
    ];
    for (q, holds) in cases {
        let (code, msg) = refused(&s, q);
        assert!(code == -103 && msg.contains(holds), "{q}: {code} {msg}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR10] [P1 §4.4]
/// PERCENTILE_CONT and PERCENTILE_DISC of a group variable's elements.
#[test]
fn inverse_distribution_of_a_group_4_4() {
    let (_db, s) = bank();
    let q = "SELECT B, D, C FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1')-[t:Transfer]->{2}(b) COLUMNS (b.id AS B, \
             PERCENTILE_DISC(0.75) WITHIN GROUP (ORDER BY t.amount) AS D, \
             PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY t.amount DESC) AS C))";
    assert_eq!(rows(&s, q), ["a2,1E1,9E0", "a5,8E0,7E0"]);
}
/// Verifies: [EXTENSION] [GS §11.1] FOR EACH SEGMENT (e1, e2) IN p keeps
/// a match whose every two edges one after the other meet its condition,
/// of a path or a subpath variable.
#[test]
fn each_segment_11_1() {
    let (_db, s) = bank();
    let q = "SELECT B FROM GRAPH_TABLE (BANK MATCH p = (a WHERE a.id = 'a1')-[t:Transfer]->{1,3}(b) \
             FOR EACH SEGMENT (e1, e2) IN p WHERE e1.amount <= e2.amount COLUMNS (b.id AS B))";
    assert_eq!(rows(&s, q), ["a2", "a3", "a4"]);
    let q = "SELECT B FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1')-[s:Transfer]->(m) (q = (c)-[t:Transfer]->{2}(b)) \
             FOR EACH SEGMENT (e1, e2) IN q WHERE e1.when_at < e2.when_at COLUMNS (b.id AS B))";
    assert_eq!(rows(&s, q), ["a1", "a4"]);
    let (code, msg) =
        refused(&s, "SELECT 1 FROM GRAPH_TABLE (BANK MATCH (a) FOR EACH SEGMENT (e1, e2) IN p WHERE e1.id = e2.id COLUMNS (a.id AS A))");
    assert_eq!(code, -103, "{msg}");
}
