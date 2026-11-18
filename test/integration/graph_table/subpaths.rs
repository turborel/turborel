//! Subpath variables, and set functions in the WHERE of a parenthesized
//! path pattern [GS §6.4, §6.8, §6.9, §7.4].

use super::{bank, refused, rows};

/// Verifies: [EXTENSION] [GS §6.4, §6.9] a subpath variable binds the
/// part of the path its parenthesized path pattern matches, and each
/// path function is of that part: two of the three edges here, and
/// acyclic where the whole path is no more.
#[test]
fn subpath_variables_6_4() {
    let (_db, s) = bank();
    let q = "SELECT C, L, W, Q, P FROM GRAPH_TABLE (BANK MATCH p = (a WHERE a.id = 'a1')-[s]->(m) (q = (b)-[t:Transfer]->{2}(c)) \
             COLUMNS (c.id AS C, PATH_LENGTH(q) AS L, PATH_LENGTH(p) AS W, IS_ACYCLIC(q) AS Q, IS_ACYCLIC(p) AS P))";
    assert_eq!(rows(&s, q), ["a1,2,3,TRUE,FALSE", "a4,2,3,TRUE,TRUE"]);
    let q = "SELECT C, V FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1') (q = -[t:Transfer]->(c)) \
             COLUMNS (c.id AS C, CARDINALITY(VERTICES(q)) AS V))";
    assert_eq!(rows(&s, q), ["a3,2"]);
}

/// Verifies: [EXTENSION] [GS §6.6, §6.7] a subpath variable of one
/// alternative of a union is a conditional singleton: its path functions
/// are null of a match through another.
#[test]
fn conditional_subpaths_6_7() {
    let (_db, s) = bank();
    let q = "SELECT C, L FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a6') \
             ((q = (b)-[t WHERE t.amount > 5]->(c)) | (b)-[u WHERE u.amount < 5]->(c)) COLUMNS (c.id AS C, PATH_LENGTH(q) AS L))";
    assert_eq!(rows(&s, q), ["a3,1", "a5,NULL"]);
}

/// Verifies: [EXTENSION] [GS §6.6, §6.9, §7.2] a subpath variable within
/// a quantifier, a path variable declared twice, and a selector within
/// a parenthesized path pattern (GRF-0033) each break a Syntax Rule.
#[test]
fn subpath_rules_6_6() {
    let (_db, s) = bank();
    let cases = [
        ("SELECT 1 FROM GRAPH_TABLE (BANK MATCH (a) ((q = (b)-[t]->(c))){1,2} COLUMNS (a.id AS A))", "within a quantifier"),
        ("SELECT 1 FROM GRAPH_TABLE (BANK MATCH (q = (a)-[t]->(b)) (q = (b)-[u]->(c)) COLUMNS (a.id AS A))", "declared twice"),
        ("SELECT 1 FROM GRAPH_TABLE (BANK MATCH (a) (ANY SHORTEST (b)-[t]->+(c)) COLUMNS (a.id AS A))", "GRF-0033"),
    ];
    for (q, holds) in cases {
        let (code, msg) = refused(&s, q);
        assert!(code == -103 && msg.contains(holds), "{q}: {code} {msg}");
    }
}

/// Verifies: [EXTENSION] [GS §6.6, §6.8] a set function in the WHERE of
/// a parenthesized path pattern is over the elements its variable is
/// bound to within it: each iteration's own, of a quantified one.
#[test]
fn set_functions_in_a_group_filter_6_8() {
    let (_db, s) = bank();
    let q = "SELECT COUNT(*) FROM GRAPH_TABLE (BANK MATCH (a) ((b)-[t]->(c) WHERE COUNT(t.*) > 0){1,2} COLUMNS (a.id AS A))";
    let unfiltered = "SELECT COUNT(*) FROM GRAPH_TABLE (BANK MATCH (a) ((b)-[t]->(c)){1,2} COLUMNS (a.id AS A))";
    assert_eq!(rows(&s, q), rows(&s, unfiltered));
    let q = "SELECT D, S FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1') ((b)-[t]->{1,2}(c) WHERE SUM(t.amount) > 15) \
             COLUMNS (c.id AS D, SUM(t.amount) AS S))";
    assert_eq!(rows(&s, q), ["a2,18"]);
    let q = "SELECT D, N FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1') ((b)-[t:Transfer]->{1,2}(c) WHERE COUNT(t.*) = 2){1,2} (d) \
             COLUMNS (d.id AS D, COUNT(t.*) AS N))";
    assert_eq!(rows(&s, q), ["a2,2", "a3,4", "a5,2", "a6,4"]);
}

/// Verifies: [EXTENSION] [GS §7.4 R7.3, R7.4] a set function in a
/// group's WHERE over a variable a restrictor bounds there, or a
/// singleton there of a group quantified without bound, is taken; over
/// one unbounded there it is GRF-0032.
#[test]
fn bounded_where_the_filter_reads_7_4() {
    let (_db, s) = bank();
    let q = "SELECT D FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1') (TRAIL (b)-[t:Transfer]->+(c) WHERE COUNT(t.*) = 3) \
             COLUMNS (c.id AS D))";
    assert_eq!(rows(&s, q), ["a1", "a4"]);
    let q = "SELECT D FROM GRAPH_TABLE (BANK MATCH ANY SHORTEST (a WHERE a.id = 'a1') \
             ((b)-[t:Transfer]->(c) WHERE COUNT(t.*) = 1)* (d WHERE d.id = 'a4') COLUMNS (d.id AS D))";
    assert_eq!(rows(&s, q), ["a4"]);
    let q = "SELECT 1 FROM GRAPH_TABLE (BANK MATCH ANY SHORTEST (a) ((b)-[t]->*(c) WHERE COUNT(t.*) > 1) COLUMNS (a.id AS A))";
    let (code, msg) = refused(&s, q);
    assert!(code == -103 && msg.contains("GRF-0032"), "{code} {msg}");
}
