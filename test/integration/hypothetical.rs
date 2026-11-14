//! The hypothetical set functions [ISO/IEC 9075-2:2023 §10.9, Feature
//! T612]: RANK, DENSE_RANK, PERCENT_RANK and CUME_DIST WITHIN GROUP of a
//! group, each against GR9's definition by a window function over the
//! group's rows and the hypothetical row, of one key and several, in each
//! direction and null ordering, of ties, nulls and collations; of no
//! rows; in grouping sets, filtered, prepared and in a view; past the work
//! area; of values of the groups and outer references; each Syntax Rule;
//! the names still names; and a constraint of one.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::value::{Decimal, Value};

use crate::common::{Counted, both_ways, exec, fetched, rows, sorted};
use crate::windows::prepared;

/// Schema A: T, of rows in three groups, ties, nulls, and strings unlike
/// only in case; U, of numbers.
fn ranked_rows() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (I INTEGER, G INTEGER, V INTEGER, S VARCHAR(5))
           CREATE TABLE U (X INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run(
        "INSERT INTO T VALUES (1, 1, 1, 'a'), (2, 1, 2, 'B'), (3, 1, NULL, 'c'), (4, 1, 2, 'b'), (5, 2, 0, 'e'), (6, 2, 5, 'f'), \
         (7, 2, 5, 'F'), (8, 3, NULL, 'h'), (9, 1, 4, NULL)",
    )
    .unwrap();
    a.run("INSERT INTO U VALUES (1), (2), (6)").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The rows of `sql`, in its order.
fn ordered(a: &Session, sql: &str) -> Vec<String> {
    rows(a, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// [ISO/IEC 9075-2:2023 §10.9 GR9] The window function `f` of the
/// hypothetical row of `values` among the rows of T of the group's G,
/// ordered by `keys` in `orders`: the scalar subquery GR9 gives, written
/// out.
fn window_definition(f: &str, (values, keys): (&[&str], &[&str]), orders: &[&str]) -> String {
    let names: Vec<String> = (1..=keys.len()).map(|i| format!("C{i}")).collect();
    let window: Vec<String> = names.iter().zip(orders).map(|(n, o)| format!("{n} {o}")).collect();
    format!(
        "(SELECT R FROM (SELECT M, {f}() OVER (ORDER BY {}) FROM (SELECT 0, {} FROM T AS T2 WHERE T2.G = T.G UNION ALL VALUES (1, {})) \
         AS X (M, {})) AS Y (M, R) WHERE M = 1)",
        window.join(", "),
        keys.join(", "),
        values.join(", "),
        names.join(", ")
    )
}

/// `f` WITHIN GROUP of `values`, ordered by `keys` in `orders`.
fn hypothetical(f: &str, (values, keys): (&[&str], &[&str]), orders: &[&str]) -> String {
    let by: Vec<String> = keys.iter().zip(orders).map(|(k, o)| format!("{k} {o}")).collect();
    format!("{f}({}) WITHIN GROUP (ORDER BY {})", values.join(", "), by.join(", "))
}

/// The four functions.
const FUNCTIONS: [&str; 4] = ["RANK", "DENSE_RANK", "PERCENT_RANK", "CUME_DIST"];

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR9, §9.23 SR5e to
/// SR5h] each hypothetical set function of a group is its window
/// function's value for the hypothetical row in a partition of the
/// group's rows and it: of a value before all, among ties, between and
/// after all, and of null, ascending and descending, nulls first and
/// last, nulls in the rows placed and never eliminated; by batches and
/// row by row.
#[test]
fn each_function_is_its_window_definition_10_9() {
    let (_db, a) = ranked_rows();
    let values = ["0", "2", "3", "9", "CAST(NULL AS INTEGER)"];
    let orders = ["ASC", "DESC", "ASC NULLS FIRST", "DESC NULLS LAST"];
    for f in FUNCTIONS {
        for value in values {
            let mut selected = Vec::new();
            let mut defined = Vec::new();
            for order in orders {
                selected.push(hypothetical(f, (&[value], &["V"]), &[order]));
                defined.push(window_definition(f, (&[value], &["V"]), &[order]));
            }
            let sql = format!("SELECT G, {} FROM T GROUP BY G", selected.join(", "));
            let (batched, by_rows) = both_ways(&a, &sql);
            assert_eq!(batched, sorted(&a, &format!("SELECT G, {} FROM T GROUP BY G", defined.join(", "))), "{sql}");
            assert_eq!(by_rows, batched, "{sql}");
        }
    }
    let shown = "SELECT G, RANK(2) WITHIN GROUP (ORDER BY V), DENSE_RANK(5) WITHIN GROUP (ORDER BY V), \
                 RANK(2) WITHIN GROUP (ORDER BY V DESC) FROM T GROUP BY G ORDER BY G";
    assert_eq!(ordered(&a, shown), ["1,2,4,3", "2,2,2,3", "3,1,1,2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR10c, GR9, §10.10
/// GR2] of several keys a row is placed by the first it differs in, each
/// in its own direction, as GR9's window orders them; character strings
/// by the collation of value and key, so of CASE_INSENSITIVE 'b' and 'B'
/// are peers; a value and key of numbers of different types brought to
/// the type they have in common.
#[test]
fn several_keys_collations_and_types_10_9() {
    let (_db, a) = ranked_rows();
    for f in FUNCTIONS {
        let parts: (&[&str], &[&str]) = (&["2", "'b'"], &["V", "S"]);
        for orders in [["DESC", "ASC"], ["ASC NULLS FIRST", "DESC NULLS LAST"]] {
            let sql = format!("SELECT G, {} FROM T GROUP BY G", hypothetical(f, parts, &orders));
            assert_eq!(sorted(&a, &sql), sorted(&a, &format!("SELECT G, {} FROM T GROUP BY G", window_definition(f, parts, &orders))));
        }
    }
    let collated = "SELECT RANK('b') WITHIN GROUP (ORDER BY S), RANK('b') WITHIN GROUP (ORDER BY S COLLATE CASE_INSENSITIVE), \
                    DENSE_RANK('C') WITHIN GROUP (ORDER BY S COLLATE CASE_INSENSITIVE), RANK('b' COLLATE CASE_INSENSITIVE) WITHIN GROUP (ORDER BY S) FROM T";
    assert_eq!(sorted(&a, collated), ["4,2,3,2"]);
    let typed =
        "SELECT RANK(2.5) WITHIN GROUP (ORDER BY V), RANK(2.0E0) WITHIN GROUP (ORDER BY V), RANK(2) WITHIN GROUP (ORDER BY V / 2.0) FROM T";
    assert_eq!(sorted(&a, typed), ["5,3,5"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR9, §7.13 SR17, §10.9
/// GR4] the hypothetical row of a group of no rows is alone: RANK and
/// DENSE_RANK 1, PERCENT_RANK 0, CUME_DIST 1; a filter leaves rows out of
/// the group; of grouping sets, a column a set leaves out is null in the
/// values as in the select list; a `?` value takes its key's type; a view
/// of one gives what its query does; calls unlike in their function,
/// values, keys or filter are computed apart.
#[test]
fn empty_groups_grouping_sets_parameters_and_views_10_9() {
    let (_db, a) = ranked_rows();
    let empty = "SELECT RANK(1) WITHIN GROUP (ORDER BY V), DENSE_RANK(1) WITHIN GROUP (ORDER BY V), \
                 PERCENT_RANK(1) WITHIN GROUP (ORDER BY V) = 0, CUME_DIST(1) WITHIN GROUP (ORDER BY V) = 1 FROM T WHERE 1 = 0";
    assert_eq!(sorted(&a, empty), ["1,1,TRUE,TRUE"]);
    let filtered = "SELECT G, RANK(3) WITHIN GROUP (ORDER BY V) FILTER (WHERE V > 1), RANK(3) WITHIN GROUP (ORDER BY V), \
                    RANK(G) WITHIN GROUP (ORDER BY V) FROM T GROUP BY ROLLUP (G)";
    assert_eq!(sorted(&a, filtered), ["1,3,4,1", "2,1,2,2", "3,1,1,1", "NULL,3,5,8"]);
    let by_rows =
        prepared(&a, "SELECT G, RANK(?) WITHIN GROUP (ORDER BY V) FROM T GROUP BY G ORDER BY G", &[Value::Exact(Decimal::new(2, 0))]);
    assert_eq!(by_rows, Ok(vec!["2".to_owned(), "2".to_owned(), "1".to_owned()]));
    a.run("CREATE VIEW W (G, R) AS SELECT G, CUME_DIST(2) WITHIN GROUP (ORDER BY V DESC) FROM T GROUP BY G").unwrap();
    let viewed = "SELECT G, CUME_DIST(2) WITHIN GROUP (ORDER BY V DESC) FROM T GROUP BY G";
    assert_eq!(sorted(&a, "SELECT G, R FROM W"), sorted(&a, viewed));
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR9, RULES.md D38]
/// groups past the work area, spilled, the distinct rows DENSE_RANK takes
/// kept in the bag that spills, give what they do in memory and what the
/// counts of the rows before the hypothetical row give.
#[test]
fn hypothetical_set_functions_past_the_work_area_10_9() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE M (I INTEGER, G INTEGER, V INTEGER)").unwrap();
    let a = db.session("A");
    for first in (0_u32..4000).step_by(500) {
        // Each group's rows of values 0, 1, 2 and 0 again, 0 twice.
        let values: Vec<String> =
            (first..first + 500).map(|i| format!("({i}, {}, {})", i % 997, i.checked_div(997).unwrap() % 3)).collect();
        a.run(&format!("INSERT INTO M VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    let ranked = "SELECT G, RANK(1) WITHIN GROUP (ORDER BY V), DENSE_RANK(1) WITHIN GROUP (ORDER BY V), \
                  RANK(1) WITHIN GROUP (ORDER BY V DESC), DENSE_RANK(2) WITHIN GROUP (ORDER BY V) FROM M GROUP BY G";
    let counted = "SELECT G, 1 + COUNT(CASE WHEN V < 1 THEN 1 END), 1 + COUNT(DISTINCT CASE WHEN V < 1 THEN V END), \
                   1 + COUNT(CASE WHEN V > 1 THEN 1 END), 1 + COUNT(DISTINCT CASE WHEN V < 2 THEN V END) FROM M GROUP BY G";
    let in_memory = sorted(&a, ranked);
    assert_eq!(in_memory.len(), 997);
    assert_eq!(in_memory, sorted(&a, counted));
    let spilled = Counted::default();
    db.set_spill(Box::new(spilled.clone())).unwrap();
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    assert_eq!(sorted(&a, ranked), in_memory);
    assert!(spilled.taken() > 0, "they spilled");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR14, §6.9 SR6] the
/// values are not aggregated, so of a grouped query they name grouping
/// columns, and of an ungrouped one none, else SQLCODE -103 as a select
/// list's column would be; an outer reference among them is a value of
/// the enclosing row, as GR9's subquery has it; sort keys whose columns
/// are all outer references make it the enclosing query's, of its groups,
/// its values of those groups too, and keys of a column of their own query
/// as well keep it there, an outer reference a value of the enclosing row.
#[test]
fn values_are_of_the_groups_10_9() {
    let (_db, a) = ranked_rows();
    assert_eq!(fetched(&a, "SELECT RANK(V) WITHIN GROUP (ORDER BY V) FROM T").map_err(|e| e.code), Err(Code::SyntaxRule));
    assert_eq!(fetched(&a, "SELECT G, RANK(I) WITHIN GROUP (ORDER BY V) FROM T GROUP BY G").map_err(|e| e.code), Err(Code::SyntaxRule));
    let outer = "SELECT X, (SELECT RANK(U.X) WITHIN GROUP (ORDER BY V) FROM T) FROM U";
    let defined = "SELECT X, (SELECT R FROM (SELECT M, RANK() OVER (ORDER BY C) FROM (SELECT 0, V FROM T UNION ALL VALUES (1, U.X)) \
                   AS X (M, C)) AS Y (M, R) WHERE M = 1) FROM U";
    assert_eq!(sorted(&a, outer), sorted(&a, defined));
    assert_eq!(sorted(&a, outer), ["1,2", "2,3", "6,8"]);
    let enclosing = "SELECT G, (SELECT RANK(G) WITHIN GROUP (ORDER BY T.V DESC) FROM U WHERE X = 1) FROM T GROUP BY G";
    assert_eq!(sorted(&a, enclosing), sorted(&a, "SELECT G, RANK(G) WITHIN GROUP (ORDER BY V DESC) FROM T GROUP BY G"));
    let keyed = "SELECT X, (SELECT RANK(5) WITHIN GROUP (ORDER BY V + U.X) FROM T) FROM U";
    assert_eq!(sorted(&a, keyed), ["1,5", "2,5", "6,1"]);
}

/// The SQLCODE of the query `sql`, and whether its error cites `rule`.
fn refused(a: &Session, sql: &str, rule: &str) -> (Code, bool) {
    let e = fetched(a, sql).expect_err(sql);
    let cited = format!("{}{}", e.msg, e.rule.as_deref().unwrap_or_default());
    (e.code, cited.contains(rule))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR10a, SR10b, SR10c,
/// §9.14 SR3, §6.11 SR1, §9.23 SR8c] a hypothetical set function with a
/// subquery, EXISTS, a set function or a window function in a value or
/// key, a nested window function, values unlike its keys in number, a
/// value and key of incompatible types, an array key, or OVER after it, is
/// SQLCODE -103, citing its rule.
#[test]
fn hypothetical_set_functions_misused_are_refused_10_9() {
    let (_db, a) = ranked_rows();
    for (sql, rule) in [
        ("SELECT RANK((SELECT MAX(X) FROM U)) WITHIN GROUP (ORDER BY V) FROM T", "§10.9 SR10a"),
        ("SELECT RANK(1) WITHIN GROUP (ORDER BY CASE WHEN EXISTS (SELECT X FROM U) THEN V END) FROM T", "§10.9 SR10a"),
        ("SELECT RANK(1) WITHIN GROUP (ORDER BY SUM(V)) FROM T", "§10.9 SR10a"),
        ("SELECT RANK(1) WITHIN GROUP (ORDER BY ROW_NUMBER() OVER (ORDER BY I)) FROM T", "§10.9 SR10a"),
        ("SELECT RANK(1) WITHIN GROUP (ORDER BY ROW_NUMBER(CURRENT_ROW)) FROM T", "§6.11 SR1"),
        ("SELECT RANK(1, 2) WITHIN GROUP (ORDER BY V) FROM T", "§10.9 SR10b"),
        ("SELECT RANK(1) WITHIN GROUP (ORDER BY V, S) FROM T", "§10.9 SR10b"),
        ("SELECT RANK('x') WITHIN GROUP (ORDER BY V) FROM T", "§10.9 SR10c"),
        ("SELECT RANK(ARRAY[1]) WITHIN GROUP (ORDER BY ARRAY[V]) FROM T", "§9.14 SR3"),
        ("SELECT RANK(1) WITHIN GROUP (ORDER BY V) OVER () FROM T", "§9.23 SR8c"),
    ] {
        assert_eq!(refused(&a, sql, rule), (Code::SyntaxRule, true), "{sql}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §5.2, §10.9, §6.10] the rank
/// names, no key words of SQL-92, are a hypothetical set function only
/// before values and WITHIN GROUP: a column and a select item may be so
/// named, and RANK ( ) OVER is the window function still.
#[test]
fn rank_names_name_columns_5_2() {
    let (_db, a) = ranked_rows();
    a.run("CREATE TABLE N (RANK INTEGER, CUME_DIST INTEGER)").unwrap();
    a.run("INSERT INTO N VALUES (1, 2), (3, 4)").unwrap();
    assert_eq!(sorted(&a, "SELECT RANK, CUME_DIST FROM N"), ["1,2", "3,4"]);
    assert_eq!(sorted(&a, "SELECT RANK(2) WITHIN GROUP (ORDER BY RANK) FROM N"), ["2"]);
    assert_eq!(sorted(&a, "SELECT RANK, RANK() OVER (ORDER BY RANK DESC) FROM N"), ["1,2", "3,1"]);
    assert_eq!(fetched(&a, "SELECT RANK() WITHIN GROUP (ORDER BY V) FROM T").map_err(|e| e.code), Err(Code::Syntax));
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.16, §10.9 GR9] an
/// assertion of a hypothetical set function is checked as its query says:
/// a row placed after the hypothetical row, or of a null key placed last,
/// changes nothing; one placed before it past the bound is refused.
#[test]
fn constraints_of_hypothetical_set_functions_10_9() {
    let (_db, a) = ranked_rows();
    a.run("CREATE ASSERTION LOW CHECK ((SELECT RANK(10) WITHIN GROUP (ORDER BY V) FROM T) < 10)").unwrap();
    a.run("INSERT INTO T VALUES (10, 1, 11, 'x'), (11, 1, NULL, 'y')").unwrap();
    a.run("INSERT INTO T VALUES (12, 2, 3, 'z')").unwrap();
    assert_eq!(exec(&a, "INSERT INTO T VALUES (13, 2, 3, 'w')").err(), Some(Code::Constraint));
    a.rollback().unwrap();
}
