//! \[DESIGN\] The keys an inner hash join hashes (RULES.md D1206): of its
//! equalities of one column one is hashed, the others judged of the
//! matched rows, or not at all where the joins of an operand imply them,
//! and the rows are those of the query as written
//! [X3.135-1986 §5.20 GR2b]. Outer joins, semi joins, antijoins and mark
//! joins keep every key. These are requirements on the plan; the standard
//! governs only the rows.

use turborel::db::{Database, Session};
use turborel::params::Params;

use crate::common::{exec, sorted};

/// The rows of L of each of its values of A, and those values.
const EACH: i64 = 20;
const VALUES: i64 = 200;
/// The values of A that R1 and R2 both hold in X and in Y.
const SHARED: i64 = 10;
/// The rows of R1 and of R2.
const FEW: i64 = 20;

/// Schema K. L holds each value of A twenty times and a row of a null A.
/// R holds of each value a row whose X and Y are both it, a row whose X
/// is a value of L and whose Y is none, and a row of a null Y, with rows
/// of a null X, so Y has more distinct values than X. R1 and R2 pair by
/// P = Q, their X and Y equal in the first `SHARED` pairs alone.
fn tables() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION K
           CREATE TABLE L (A INTEGER, B INTEGER)
           CREATE TABLE R (X INTEGER, Y INTEGER, Z INTEGER)
           CREATE TABLE R1 (X INTEGER, P INTEGER)
           CREATE TABLE R2 (Y INTEGER, Q INTEGER)",
    )
    .unwrap();
    let s = db.session("K");
    let list = |rows: Vec<String>| rows.join(", ");
    let rows = VALUES * EACH;
    s.run(&format!("INSERT INTO L VALUES (NULL, 0), {}", list((0..rows).map(|k| format!("({}, {k})", k % VALUES)).collect()))).unwrap();
    let of_r = |k: i64| [format!("({k}, {k}, 0)"), format!("({}, {}, 1)", k % 4, k + 1000), format!("({k}, NULL, 2)")];
    s.run(&format!("INSERT INTO R VALUES (NULL, NULL, 3), (NULL, 5, 3), {}", list((0..VALUES).flat_map(of_r).collect()))).unwrap();
    s.run(&format!("INSERT INTO R1 VALUES (NULL, 100), {}", list((0..FEW).map(|k| format!("({k}, {k})")).collect()))).unwrap();
    let y = |k: i64| if k < SHARED { k } else { k + 50 };
    s.run(&format!("INSERT INTO R2 VALUES (NULL, 100), {}", list((0..FEW).map(|k| format!("({}, {k})", y(k))).collect()))).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// The physical plan of `sql` and its rows, planned as `mode` says.
fn planned(s: &Session, mode: &str, sql: &str) -> (String, Vec<String>) {
    exec(s, &format!("SET PLANNER.JOIN_ENUMERATION = '{mode}'")).unwrap();
    let shown = s.explain(sql, &Params::new()).unwrap();
    let physical = shown.split("-- physical:").nth(1).unwrap().to_owned();
    (physical, sorted(s, sql))
}

/// The ways the join order is chosen.
const MODES: [&str; 2] = ["GREEDY", "ADAPTIVE"];

/// Verifies: [X3.135-1986 §5.20 GR2b]; [X3.135-1986 §5.11 GR2]; [DESIGN]
/// RULES.md D1206, D302: of two equalities of one column of L, an inner
/// hash join hashes the one of more distinct values, R.Y, however the
/// conjuncts are written, and judges the other in the filter over it.
/// The rows are the query's. A row of R whose Y equals L.A and whose X
/// does not is a pair the filter refuses, and a null in A, X or Y matches
/// nothing, as a key or as the filter.
#[test]
fn an_inner_hash_join_hashes_one_key_of_a_column() {
    let db = tables();
    let s = db.session("K");
    let rows = VALUES * EACH;
    let expected = vec![format!("{rows},{}", (0..rows).sum::<i64>())];
    let stated = ["L, R WHERE L.A = R.X AND L.A = R.Y", "R, L WHERE R.Y = L.A AND R.X = L.A"];
    for (from, mode) in stated.iter().flat_map(|f| MODES.map(|m| (f, m))) {
        let (plan, got) = planned(&s, mode, &format!("SELECT COUNT(*), SUM(L.B) FROM {from}"));
        assert!(plan.contains("HashJoin L.A = R.Y\n") || plan.contains("HashJoin L.A = R.Y  ("), "{mode} {from}\n{plan}");
        assert!(plan.contains("Filter L.A = R.X") || plan.contains("Filter R.X = L.A"), "{mode} {from}\n{plan}");
        assert_eq!(got, expected, "{mode} {from}");
    }
    // X = Y of a row of R that pairs with no row of L by its X is refused.
    let (_, none) = planned(&s, "ADAPTIVE", "SELECT COUNT(*) FROM L, R WHERE L.A = R.X AND L.A = R.Y AND R.Z = 1");
    assert_eq!(none, vec!["0".to_owned()]);
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR1]; [DESIGN] RULES.md D1206: a
/// LEFT and a FULL outer join, a semi join, an antijoin and a mark join
/// of two equalities of one column hash both, as they judge a condition
/// beyond their keys pair by pair, and give their rows. The row of L of a
/// null A is preserved by the outer joins, kept by the antijoin, and no
/// row of R matches it.
#[test]
fn outer_semi_anti_and_mark_joins_keep_every_key() {
    let db = tables();
    let s = db.session("K");
    let rows = VALUES * EACH;
    let unmatched = 2 + 2 * VALUES;
    let exists = "EXISTS (SELECT * FROM R WHERE R.X = L.A AND R.Y = L.A)";
    let cases = [
        ("SELECT COUNT(*), COUNT(R.Z) FROM L LEFT JOIN R ON L.A = R.X AND L.A = R.Y".to_owned(), format!("{},{rows}", rows + 1)),
        (
            "SELECT COUNT(*), COUNT(R.Z), COUNT(L.B) FROM L FULL JOIN R ON L.A = R.X AND L.A = R.Y".to_owned(),
            format!("{},{},{}", rows + 1 + unmatched, rows + unmatched, rows + 1),
        ),
        (format!("SELECT COUNT(*) FROM L WHERE {exists}"), rows.to_string()),
        (format!("SELECT COUNT(*) FROM L WHERE NOT {exists}"), "1".to_owned()),
        (format!("SELECT COUNT(*) FROM L WHERE L.B < 0 OR {exists}"), rows.to_string()),
    ];
    for ((sql, expected), mode) in cases.iter().flat_map(|c| MODES.map(|m| (c, m))) {
        let (plan, got) = planned(&s, mode, sql);
        assert!(plan.contains("(hashed on L.A = R.X AND L.A = R.Y)"), "{mode} {sql}\n{plan}");
        assert_eq!(got, vec![expected.clone()], "{mode} {sql}");
    }
}

/// The rows of L of the values R1 and R2 share.
fn shared() -> Vec<String> {
    vec![(SHARED * EACH).to_string()]
}

/// Verifies: [X3.135-1986 §5.11]; [DESIGN] RULES.md D1206: L joined to
/// the join of R1 and R2 that judged R1.X = R2.Y, of `L.A = R1.X AND L.A
/// = R2.Y` one is hashed and the other judged nowhere, being true of
/// every matched pair, and the rows are the greedy order's.
#[test]
fn an_equality_the_built_operands_joins_imply_is_not_judged() {
    let db = tables();
    let s = db.session("K");
    let sql = "SELECT COUNT(*) FROM L, R1, R2 WHERE L.A = R1.X AND L.A = R2.Y AND R1.X = R2.Y";
    let (plan, got) = planned(&s, "ADAPTIVE", sql);
    assert!(plan.contains("(L (R1 R2))"), "{plan}");
    assert!(plan.contains("HashJoin L.A = R1.X  ("), "{plan}");
    assert!(!plan.contains("L.A = R2.Y"), "{plan}");
    assert_eq!(got, shared());
    assert_eq!(planned(&s, "GREEDY", sql).1, shared());
}

/// Verifies: [X3.135-1986 §5.20 GR2b]; [DESIGN] RULES.md D1206: where R1
/// and R2 are joined by another equality, P = Q, nothing below judged
/// R1.X = R2.Y, so `L.A = R2.Y` is not implied and stays in the filter
/// over the join. Half the pairs of R1 and R2 differ in X and Y, and the
/// filter refuses their rows.
#[test]
fn an_equality_no_join_below_judged_stays_in_the_filter() {
    let db = tables();
    let s = db.session("K");
    let sql = "SELECT COUNT(*) FROM L, R1, R2 WHERE L.A = R1.X AND L.A = R2.Y AND R1.P = R2.Q";
    let (plan, got) = planned(&s, "ADAPTIVE", sql);
    assert!(plan.contains("(L (R1 R2))"), "{plan}");
    assert!(plan.contains("Filter L.A = R2.Y") && plan.contains("HashJoin L.A = R1.X\n"), "{plan}");
    assert_eq!(got, shared());
    assert_eq!(planned(&s, "GREEDY", sql).1, shared());
}
