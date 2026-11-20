//! [GS §10.1] MATCH in a WHERE clause over FROM clauses of joined and
//! derived tables, two MATCH conjuncts, MATCH nested in conditions under
//! OR, and SELECT * of each.

use super::{people, refused, rows};

/// Verifies: [EXTENSION] [GS §10.1]: two MATCH conjuncts of a WHERE clause
/// are one MATCH of both patterns, and a MATCH reads tables of a joined
/// table of the FROM clause.
#[test]
fn match_conjuncts_and_joined_tables_10_1() {
    let (_db, s) = people();
    let two = "SELECT a.name, c.name FROM person a, friend_of f, person b, friend_of g, person c \
               WHERE MATCH(a-(f)->b) AND MATCH(b-(g)->c) AND a.name = 'Alice'";
    assert_eq!(rows(&s, two), ["Alice,Carol", "Alice,Dave"]);
    let joined = "SELECT b.name FROM person a CROSS JOIN friend_of f CROSS JOIN person b WHERE MATCH(a-(f)->b) AND a.name = 'Bob'";
    assert_eq!(rows(&s, joined), ["Carol"]);
}

/// Verifies: [EXTENSION] [GS §10.1], [ISO/IEC 9075:1992 §7.9 SR4]: SELECT *
/// of a MATCH is each table's columns, of a joined table's tables and of a
/// derived table too; over a collection derived table it is not supported
/// (SQLCODE -900).
#[test]
fn select_star_of_a_match_10_1() {
    let (_db, s) = people();
    let joined = "SELECT * FROM person a CROSS JOIN friend_of f, person b WHERE MATCH(a-(f)->b) AND a.name = 'Carol'";
    assert_eq!(rows(&s, joined), ["3,Carol,2034,4,Dave"]);
    let derived = "SELECT * FROM person a, friend_of f, person b, (VALUES (7)) AS v (x) WHERE MATCH(a-(f)->b) AND a.name = 'Carol'";
    assert_eq!(rows(&s, derived), ["3,Carol,2034,4,Dave,7"]);
    let unnest = "SELECT * FROM person a, friend_of f, person b, UNNEST(ARRAY[1]) AS u (x) WHERE MATCH(a-(f)->b)";
    assert_eq!(refused(&s, unnest, "SELECT *"), -900);
}

/// Verifies: [EXTENSION] [GS §10.1]: a MATCH under OR may sit in an AND, an
/// IS test or a parenthesized condition of it, each read as the IN
/// predicate of its tables' keys.
#[test]
fn matches_nested_under_or_10_1() {
    let (_db, s) = people();
    let anded = "SELECT DISTINCT p.name FROM person p, person q, friend_of f \
                 WHERE p.name = 'Dave' OR (MATCH(p-(f)->q) AND q.name = 'Dave')";
    assert_eq!(rows(&s, anded), ["Carol", "Dave", "Erin"]);
    let tested = "SELECT DISTINCT p.name FROM person p, person q, friend_of f \
                  WHERE p.name = 'Alice' OR (MATCH(p-(f)->q) IS TRUE AND q.name = 'Carol')";
    assert_eq!(rows(&s, tested), ["Alice", "Bob"]);
}
