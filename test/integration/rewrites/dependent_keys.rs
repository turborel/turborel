//! A grouping by fewer columns, those functionally dependent on the
//! others carried (RULES.md D1201, `optimizer/dependent_keys.rs`).

use turborel::optimizer::REDUCE_GROUPING_KEYS;

use turborel::db::{Database, Session};

use super::{COLUMNS, alike, left, plan, tables};

/// Schema Q's tables, and of each store PA of 12 parents keyed by K, names
/// repeated and null; CH of 60 children keyed by K, each of a parent, of
/// none or of one that is not there; NU of a UNIQUE column that may be
/// null, null in two rows; WS of strings equal but for trailing spaces; SK
/// keyed by a string; GE of generated columns; FL of approximate numbers; and TP of
/// two kinds of parent keyed by K. The statistics of PA, CH and TP are
/// gathered.
pub fn with_keys() -> (Database, Session) {
    let (db, s) = tables();
    for (prefix, store) in [("", ""), (COLUMNS, " USING columnstore")] {
        for table in [
            "PA (K INTEGER NOT NULL PRIMARY KEY, N VARCHAR(10), A INTEGER)",
            "CH (K INTEGER NOT NULL PRIMARY KEY, P INTEGER, V INTEGER NOT NULL, S VARCHAR(10))",
            "NU (K INTEGER UNIQUE, N VARCHAR(10))",
            "WS (K INTEGER, S VARCHAR(10))",
            "SK (S VARCHAR(10) NOT NULL PRIMARY KEY, X INTEGER)",
            "FL (X DOUBLE PRECISION, Y INTEGER)",
            "TP (K INTEGER NOT NULL PRIMARY KEY, L VARCHAR(10))",
            "GE (A INTEGER, B INTEGER GENERATED ALWAYS AS (A * 2), S VARCHAR(10), L INTEGER GENERATED ALWAYS AS (CHAR_LENGTH(S)))",
        ] {
            s.run(&format!("CREATE TABLE {prefix}{table}{store}")).unwrap();
        }
        let parents: Vec<String> = (0..12)
            .map(|k| if k % 5 == 4 { format!("({k}, NULL, {})", k % 3) } else { format!("({k}, 'n{}', {})", k % 4, k % 3) })
            .collect();
        s.run(&format!("INSERT INTO {prefix}PA VALUES {}", parents.join(", "))).unwrap();
        let children: Vec<String> = (0..60)
            .map(|k| {
                let parent = if k % 11 == 7 { "NULL".to_owned() } else { (k % 15).to_string() };
                format!("({k}, {parent}, {}, 's{}')", k % 7, k % 3)
            })
            .collect();
        s.run(&format!("INSERT INTO {prefix}CH VALUES {}", children.join(", "))).unwrap();
        s.run(&format!("INSERT INTO {prefix}NU VALUES (1, 'a'), (2, 'b'), (NULL, 'c'), (NULL, 'd'), (3, 'a')")).unwrap();
        s.run(&format!("INSERT INTO {prefix}WS VALUES (1, 'a'), (1, 'a '), (2, 'b'), (2, 'b'), (3, 'B')")).unwrap();
        s.run(&format!("INSERT INTO {prefix}SK VALUES ('a', 1), ('A', 2), ('b', 3)")).unwrap();
        s.run(&format!("INSERT INTO {prefix}GE (A, S) VALUES (1, 'a'), (1, 'a '), (2, 'b'), (NULL, NULL), (NULL, 'b')")).unwrap();
        s.run(&format!("INSERT INTO {prefix}TP VALUES (0, 'x'), (1, 'y')")).unwrap();
        s.run(&format!("INSERT INTO {prefix}FL VALUES (1, 1), (1, 2), (2.5, 2), (0, 0), (-0E0, 5)")).unwrap();
    }
    s.commit().unwrap();
    for table in ["PA", "CH", "TP"] {
        db.analyze(&format!("Q.{table}")).unwrap();
        db.analyze(&format!("Q.{COLUMNS}{table}")).unwrap();
    }
    db.maintain().unwrap();
    (db, s)
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.26.1, §4.26.3, §4.26.7, §4.26.12,
/// §9.12]; [DESIGN] RULES.md D1201: a grouping whose columns hold a key of
/// their table, or a column an equality makes the key's, gives the same
/// rows grouped by the determining columns alone, of either store: of one
/// table, the key written first and last; of a join, with set functions
/// of the other table, DISTINCT and filtered; of a column equal to the
/// key; of the rows a LEFT join extends with nulls, the key its preserved
/// operand's; of a parent's columns under that join, null of the rows
/// extended, by its key; under HAVING and ORDER BY of a column carried;
/// of a column generated of exact numbers; and of columns each equal to a
/// constant, of no rows, one of which is still grouped by, so no group is
/// made of none.
#[test]
fn a_grouping_by_a_key_and_its_dependents_groups_by_the_key() {
    let (_db, s) = with_keys();
    for sql in [
        "SELECT P.K, P.N, P.A, COUNT(*) FROM {PA} P GROUP BY P.K, P.N, P.A",
        "SELECT P.N, COUNT(*), P.A, P.K FROM {PA} P GROUP BY P.N, P.A, P.K",
        "SELECT P.K, P.N, SUM(C.V), COUNT(DISTINCT C.S), COUNT(*) FILTER (WHERE C.V > 2) FROM {PA} P, {CH} C WHERE C.P = P.K GROUP BY P.K, P.N",
        "SELECT C.P, P.N, P.A, COUNT(*) FROM {PA} P JOIN {CH} C ON C.P = P.K GROUP BY C.P, P.N, P.A",
        "SELECT C.K, P.K, P.N, COUNT(*) FROM {CH} C LEFT JOIN {PA} P ON C.P = P.K GROUP BY C.K, P.K, P.N",
        "SELECT P.K, P.N, C.S, MAX(C.V) FROM {CH} C LEFT JOIN {PA} P ON C.P = P.K AND P.A > 0 GROUP BY P.K, P.N, C.S",
        "SELECT P.K, P.N, COUNT(*) FROM {PA} P, {CH} C WHERE C.P = P.K GROUP BY P.K, P.N HAVING COUNT(*) > 3 AND P.N <> 'n3' \
         ORDER BY P.N, P.K",
        "SELECT G.A, G.B, COUNT(*) FROM {GE} G GROUP BY G.A, G.B",
        "SELECT P.A, P.N, COUNT(*) FROM {PA} P WHERE P.A = 7 AND P.N = 'zz' GROUP BY P.A, P.N",
        "SELECT S.S, S.X, COUNT(*) FROM {SK} S, {WS} W WHERE W.K = S.X GROUP BY S.S, S.X",
    ] {
        alike(&s, REDUCE_GROUPING_KEYS, sql);
    }
    let shown = plan(&s, "SELECT K, N, A, COUNT(*) FROM PA GROUP BY K, N, A");
    let physical = shown.split("-- physical:").nth(1).unwrap_or_default();
    assert!(physical.contains("group by [PA.K] computing [COUNT(*), "), "{shown}");
    let kept = plan(&s, "SELECT K, N, COUNT(*) FROM PA GROUP BY K, N HAVING COUNT(*) > 0");
    let optimized = kept.split("-- optimized:").nth(1).and_then(|rest| rest.split("-- physical:").next()).unwrap_or_default();
    let lines: Vec<&str> = optimized.lines().map(str::trim_start).collect();
    let next_to = lines
        .windows(2)
        .any(|pair| pair.first().is_some_and(|l| l.starts_with("Filter")) && pair.get(1).is_some_and(|l| l.starts_with("Aggregate")));
    assert!(next_to, "a HAVING condition stays next to its grouping: {kept}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.26.1, §4.26.3, §4.26.7, §7.13 GR2a];
/// [DESIGN] RULES.md D1201: the rule leaves a grouping whose columns do not
/// determine each other, or not strictly: columns no key is among; a
/// UNIQUE column that may be null, two rows null in it; a FULL join's
/// equality, each side null of the other's rows; grouping sets, which
/// make a group of no rows; a key grouped by another collation than its
/// own, and the column it determines so grouped; a column generated of a
/// character string, different of two strings not distinct; an outer
/// join whose condition tells such strings apart; and an equality of an
/// approximate number and an exact one.
#[test]
fn a_grouping_whose_columns_may_not_depend_is_left() {
    let (_db, s) = with_keys();
    for sql in [
        "SELECT C.P, C.V, COUNT(*) FROM {CH} C GROUP BY C.P, C.V",
        "SELECT N.K, N.N, COUNT(*) FROM {NU} N GROUP BY N.K, N.N",
        "SELECT C.P, P.N, COUNT(*) FROM {CH} C FULL JOIN {PA} P ON C.P = P.K GROUP BY C.P, P.N",
        "SELECT P.K, P.N, COUNT(*) FROM {PA} P GROUP BY ROLLUP (P.K, P.N)",
        "SELECT MIN(S.S), MIN(S.X), COUNT(*) FROM {SK} S GROUP BY S.S COLLATE CASE_INSENSITIVE, S.X",
        "SELECT P.K, MIN(P.N), COUNT(*) FROM {PA} P GROUP BY P.K, P.N COLLATE CASE_INSENSITIVE",
        "SELECT MIN(G.S), G.L, COUNT(*) FROM {GE} G GROUP BY G.S, G.L",
        "SELECT W.K, MIN(W.S), P.K, COUNT(*) FROM {WS} W LEFT JOIN {PA} P ON P.K = W.K AND CHAR_LENGTH(W.S) = 1 GROUP BY W.K, W.S, P.K",
        "SELECT F.X, C.K, COUNT(*) FROM {FL} F, {CH} C WHERE F.X = C.K GROUP BY F.X, C.K",
    ] {
        left(&s, REDUCE_GROUPING_KEYS, sql);
    }
}
