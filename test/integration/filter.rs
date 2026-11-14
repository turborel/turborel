//! The `<filter clause>` [ISO/IEC 9075-2:2023 §10.9, Feature T612]: each
//! set function, COUNT(*), DISTINCT, ARRAY_AGG and LISTAGG fed only the
//! rows its filter is true of, grouped and not, by batches and row by row,
//! in grouping sets and prepared statements, over windows growing, moving
//! and with nested window functions, past the work area; an argument not
//! computed of a row not taken; each Syntax Rule; FILTER still a name; and
//! a constraint of a filtered set function.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError, SqlState};
use turborel::value::{Decimal, Value};

use crate::common::{Counted, both_ways, exec, fetched, rows, sorted};

/// Schema A: T, of rows in three groups, a null V in two of them and a
/// group of null V alone; U, of numbers.
fn rows_of_groups() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (I INTEGER, G INTEGER, V INTEGER, S VARCHAR(5))
           CREATE TABLE U (X INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run(
        "INSERT INTO T VALUES (1, 1, 1, 'a'), (2, 1, 2, 'b'), (3, 1, NULL, 'c'), (4, 1, 3, 'd'), (5, 2, 0, 'e'), (6, 2, 5, 'f'), \
         (7, 2, 5, 'g'), (8, 3, NULL, 'h')",
    )
    .unwrap();
    a.run("INSERT INTO U VALUES (1), (4), (9)").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The rows of `sql`, in its order.
fn ordered(a: &Session, sql: &str) -> Vec<String> {
    rows(a, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR4, GR5, GR7] each
/// set function of a group is fed only the rows its filter is true of:
/// COUNT(*) counts them, SUM, AVG, MAX and COUNT(DISTINCT ...) of them are
/// as of a CASE that is null elsewhere, alike by batches and row by row; a
/// group no row of which passes gives COUNT 0 and SUM null; a filter
/// unknown takes no row; one call filtered and one not are two.
#[test]
fn grouped_set_functions_take_the_rows_filtered_10_9() {
    let (_db, a) = rows_of_groups();
    let counted = "SELECT G, COUNT(*) FILTER (WHERE V > 1), SUM(V) FILTER (WHERE V > 1) FROM T GROUP BY G ORDER BY G";
    assert_eq!(ordered(&a, counted), ["1,2,5", "2,2,10", "3,0,NULL"]);
    let filtered = "SELECT G, COUNT(*) FILTER (WHERE V > 1), SUM(V) FILTER (WHERE V > 1), COUNT(DISTINCT V) FILTER (WHERE V <> 2), \
                    MAX(S) FILTER (WHERE V < 3), AVG(V) FILTER (WHERE S <> 'b'), EVERY(V > 0) FILTER (WHERE I > 1) FROM T GROUP BY G";
    let written = "SELECT G, COUNT(CASE WHEN V > 1 THEN 1 END), SUM(CASE WHEN V > 1 THEN V END), COUNT(DISTINCT CASE WHEN V <> 2 THEN V END), \
                   MAX(CASE WHEN V < 3 THEN S END), AVG(CASE WHEN S <> 'b' THEN V END), EVERY(CASE WHEN I > 1 THEN V > 0 END) FROM T GROUP BY G";
    let (batched, by_rows) = both_ways(&a, filtered);
    assert_eq!(batched, sorted(&a, written));
    assert_eq!(by_rows, batched);
    assert_eq!(sorted(&a, "SELECT COUNT(*) FILTER (WHERE 1 = 0), SUM(V) FILTER (WHERE 1 = 0) FROM T"), ["0,NULL"]);
    assert_eq!(sorted(&a, "SELECT COUNT(*) FILTER (WHERE V > 0), COUNT(*) FILTER (WHERE NOT V > 0) FROM T"), ["5,1"]);
    assert_eq!(sorted(&a, "SELECT SUM(V), SUM(V) FILTER (WHERE V > 1), SUM(V) FILTER (WHERE V > 2) FROM T"), ["16,15,13"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR4, GR13, GR11,
/// NOTE 528] ARRAY_AGG keeps the null values of the rows its filter takes,
/// and of none is null; LISTAGG joins the values of the rows taken alone.
#[test]
fn array_agg_and_listagg_take_the_rows_filtered_10_9() {
    let (_db, a) = rows_of_groups();
    let sql = "SELECT G, CARDINALITY(ARRAY_AGG(V ORDER BY I) FILTER (WHERE S <> 'b')), \
               LISTAGG(S, '') WITHIN GROUP (ORDER BY I) FILTER (WHERE V IS NULL OR V > 1), \
               ARRAY_AGG(V ORDER BY I) FILTER (WHERE V > 9) IS NULL FROM T GROUP BY G ORDER BY G";
    assert_eq!(ordered(&a, sql), ["1,3,bcd,TRUE", "2,3,fg,TRUE", "3,1,h,TRUE"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR4, GR7a] a row its
/// filter does not take is not aggregated, so its argument is not
/// computed, row by row, by batches and over a window, nested window
/// functions in it or not: a division by its zero raises nothing; a null
/// of it is not eliminated, so no 01003, though a null of a row taken is.
#[test]
fn rows_not_taken_compute_no_argument_10_9() {
    let (_db, a) = rows_of_groups();
    for sql in [
        "SELECT SUM(V / V) FILTER (WHERE V <> 0) FROM T",
        "SELECT G, SUM(V / V) FILTER (WHERE V <> 0) FROM T GROUP BY G",
        "SELECT SUM(V / V) FILTER (WHERE V <> 0) OVER (ORDER BY I) FROM T",
        "SELECT SUM(V / V) FILTER (WHERE V <> 0) OVER (ORDER BY I ROWS BETWEEN 1 PRECEDING AND CURRENT ROW) FROM T",
        "SELECT SUM(VALUE_OF(V AT FRAME_ROW) / V) FILTER (WHERE V <> 0) OVER (ORDER BY I) FROM T",
        "SELECT SUM(V / V + 0 * ROW_NUMBER(CURRENT_ROW)) FILTER (WHERE V <> 0) OVER (ORDER BY I) FROM T",
    ] {
        let (batched, by_rows) = both_ways(&a, sql);
        assert_eq!(batched, by_rows, "{sql}");
    }
    assert_eq!(sorted(&a, "SELECT SUM(V / V) FILTER (WHERE V <> 0) FROM T"), ["5"]);
    sorted(&a, "SELECT SUM(V) FILTER (WHERE V IS NOT NULL) FROM T");
    assert_eq!(a.warning(), None, "no null of a row taken");
    sorted(&a, "SELECT SUM(V) FILTER (WHERE V IS NOT NULL) OVER (ORDER BY I) FROM T");
    assert_eq!(a.warning(), None, "no null of a row taken in a window");
    sorted(&a, "SELECT SUM(V) FILTER (WHERE S <> 'z') FROM T");
    assert_eq!(a.warning().map(SqlState::as_str), Some("01003"), "a null of a row taken");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR4, §7.15] over a
/// window each frame's set function takes the rows of it its filter is
/// true of, as of a CASE null elsewhere: a frame growing, moving, of a
/// partition, excluding rows, DISTINCT, and of nested window functions
/// computed once or for each frame.
#[test]
fn window_set_functions_take_the_rows_filtered_10_9() {
    let (_db, a) = rows_of_groups();
    let filtered = "SELECT I, SUM(V) FILTER (WHERE V > 1) OVER (ORDER BY I), \
                    SUM(V) FILTER (WHERE V > 1) OVER (ORDER BY I ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING), \
                    COUNT(*) FILTER (WHERE V > 1) OVER (PARTITION BY G), \
                    MAX(S) FILTER (WHERE V < 3) OVER (ORDER BY I ROWS BETWEEN 2 PRECEDING AND CURRENT ROW EXCLUDE CURRENT ROW), \
                    COUNT(DISTINCT V) FILTER (WHERE V <> 2) OVER (PARTITION BY G), \
                    SUM(VALUE_OF(V AT FRAME_ROW - 1, 0)) FILTER (WHERE V > 1) OVER (ORDER BY I), \
                    SUM(V * ROW_NUMBER(CURRENT_ROW)) FILTER (WHERE V > 1) OVER (ORDER BY I ROWS 2 PRECEDING) FROM T";
    let written = "SELECT I, SUM(CASE WHEN V > 1 THEN V END) OVER (ORDER BY I), \
                   SUM(CASE WHEN V > 1 THEN V END) OVER (ORDER BY I ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING), \
                   COUNT(CASE WHEN V > 1 THEN 1 END) OVER (PARTITION BY G), \
                   MAX(CASE WHEN V < 3 THEN S END) OVER (ORDER BY I ROWS BETWEEN 2 PRECEDING AND CURRENT ROW EXCLUDE CURRENT ROW), \
                   COUNT(DISTINCT CASE WHEN V <> 2 THEN V END) OVER (PARTITION BY G), \
                   SUM(CASE WHEN V > 1 THEN VALUE_OF(V AT FRAME_ROW - 1, 0) END) OVER (ORDER BY I), \
                   SUM(CASE WHEN V > 1 THEN V * ROW_NUMBER(CURRENT_ROW) END) OVER (ORDER BY I ROWS 2 PRECEDING) FROM T";
    assert_eq!(sorted(&a, filtered), sorted(&a, written));
    let by_row = "SELECT I, COUNT(*) FILTER (WHERE V > 1) OVER (ORDER BY I) FROM T ORDER BY I";
    assert_eq!(ordered(&a, by_row), ["1,0", "2,1", "3,1", "4,2", "5,2", "6,3", "7,4", "8,4"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR4, §7.13, §20.7]
/// a filtered set function in grouping sets, of each set's groups, its
/// filter reading a column its set leaves out as null [§7.13 SR17 m iii];
/// with a `?` in its filter, as of the value given; in a view.
#[test]
fn filters_in_grouping_sets_statements_and_views_10_9() {
    let (_db, a) = rows_of_groups();
    let rolled = "SELECT G, SUM(V) FILTER (WHERE V > 1), COUNT(*) FILTER (WHERE V > 1) FROM T GROUP BY ROLLUP (G)";
    let written = "SELECT G, SUM(CASE WHEN V > 1 THEN V END), COUNT(CASE WHEN V > 1 THEN 1 END) FROM T GROUP BY ROLLUP (G)";
    assert_eq!(sorted(&a, rolled), sorted(&a, written));
    let left_out = "SELECT COUNT(*) FILTER (WHERE G IS NULL) FROM T GROUP BY ROLLUP (G)";
    assert_eq!(sorted(&a, left_out), ["0", "0", "0", "8"]);
    let p = a.prepare("SELECT G, SUM(V) FILTER (WHERE V > ?) FROM T GROUP BY G ORDER BY G").unwrap();
    let mut c = a.open_prepared(&p, &[Value::Exact(Decimal::new(2, 0))]).unwrap();
    let mut got = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        got.push(format!("{},{}", row[0], row[1]));
    }
    assert_eq!(got, ["1,3", "2,10", "3,NULL"]);
    drop(c);
    a.run("CREATE VIEW F (G, N) AS SELECT G, COUNT(*) FILTER (WHERE V > 1) FROM T GROUP BY G").unwrap();
    assert_eq!(sorted(&a, "SELECT G, N FROM F"), ["1,2", "2,2", "3,0"]);
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR4, RULES.md D38]
/// filtered set functions of groups and of a window's partition past the
/// work area, spilled, are as in memory and as a CASE null elsewhere.
#[test]
fn filters_past_the_work_area_10_9() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE M (I INTEGER, G INTEGER, V INTEGER)").unwrap();
    let a = db.session("A");
    for first in (0..4000).step_by(500) {
        let values: Vec<String> = (first..first + 500).map(|i| format!("({i}, {}, {})", i % 997, i % 13)).collect();
        a.run(&format!("INSERT INTO M VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    let grouped =
        "SELECT G, COUNT(*) FILTER (WHERE V > 6), SUM(V) FILTER (WHERE V > 6), COUNT(DISTINCT V) FILTER (WHERE V < 4) FROM M GROUP BY G";
    let grouped_case = "SELECT G, COUNT(CASE WHEN V > 6 THEN 1 END), SUM(CASE WHEN V > 6 THEN V END), COUNT(DISTINCT CASE WHEN V < 4 THEN V END) \
                        FROM M GROUP BY G";
    let windowed = "SELECT I, SUM(V) FILTER (WHERE V > 6) OVER (PARTITION BY G ORDER BY I ROWS 3 PRECEDING) FROM M";
    let windowed_case = "SELECT I, SUM(CASE WHEN V > 6 THEN V END) OVER (PARTITION BY G ORDER BY I ROWS 3 PRECEDING) FROM M";
    let in_memory = (sorted(&a, grouped), sorted(&a, windowed));
    assert_eq!(in_memory.0.len(), 997);
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    assert_eq!((sorted(&a, grouped), sorted(&a, windowed)), in_memory.clone());
    assert!(counted.taken() > 0, "they spilled");
    assert_eq!((sorted(&a, grouped_case), sorted(&a, windowed_case)), in_memory);
}

/// The SQLCODE of the query `sql`, and whether its error cites `rule`.
fn refused(a: &Session, sql: &str, rule: &str) -> (Code, bool) {
    let e: SqlError = fetched(a, sql).expect_err(sql);
    let cited = format!("{}{}", e.msg, e.rule.as_deref().unwrap_or_default());
    (e.code, cited.contains(rule))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.9 SR5, SR6, §10.9 SR8,
/// §6.11 SR1] a filter with a set function, a query expression in a
/// comparison, IN, EXISTS or a scalar subquery, a window function, a
/// nested window function or an outer reference is SQLCODE -103, citing
/// its rule; so is one of a column of the set function of an enclosing
/// query, whose filter is over that query's rows, though one of no column
/// is taken.
#[test]
fn filters_misused_are_refused_10_9() {
    let (_db, a) = rows_of_groups();
    for (sql, rule) in [
        ("SELECT COUNT(*) FILTER (WHERE SUM(V) > 1) FROM T", "§6.9 SR5"),
        ("SELECT G FROM T GROUP BY G HAVING MAX(V) FILTER (WHERE COUNT(*) > 1) > 0", "§6.9 SR5"),
        ("SELECT COUNT(*) FILTER (WHERE V IN (SELECT X FROM U)) FROM T", "§10.9 SR8"),
        ("SELECT COUNT(*) FILTER (WHERE EXISTS (SELECT X FROM U)) FROM T", "§10.9 SR8"),
        ("SELECT COUNT(*) FILTER (WHERE V > (SELECT MAX(X) FROM U)) FROM T", "§10.9 SR8"),
        ("SELECT COUNT(*) FILTER (WHERE RANK() OVER (ORDER BY I) > 1) FROM T", "§10.9 SR8"),
        ("SELECT SUM(V) FILTER (WHERE ROW_NUMBER(CURRENT_ROW) > 1) OVER (ORDER BY I) FROM T", "§6.11 SR1"),
        ("SELECT X FROM U WHERE X < (SELECT COUNT(*) FILTER (WHERE V > U.X) FROM T)", "§10.9 SR8"),
        ("SELECT (SELECT COUNT(*) FROM U WHERE X < SUM(T.V) FILTER (WHERE T.V > 1)) FROM T", "§10.9 SR8"),
    ] {
        assert_eq!(refused(&a, sql, rule), (Code::SyntaxRule, true), "{sql}");
    }
    let windowed = "SELECT X FROM U WHERE X < (SELECT MAX(Q) FROM (SELECT SUM(V) FILTER (WHERE V > U.X) OVER () FROM T) AS W (Q))";
    assert_eq!(refused(&a, windowed, "outer reference").0, Code::SyntaxRule);
    let outer =
        "SELECT (SELECT COUNT(*) FROM U WHERE X < SUM(T.V) FILTER (WHERE 1 = 1)), (SELECT COUNT(*) FROM U WHERE X < SUM(T.V)) FROM T";
    assert_eq!(sorted(&a, outer), ["3,3"]);
    let none = "SELECT (SELECT COUNT(*) FROM U WHERE X < SUM(T.V) FILTER (WHERE 1 = 0)) FROM T";
    assert_eq!(sorted(&a, none), ["0"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §5.2, §10.9] FILTER, no key
/// word of SQL-92, is read only after a set function where `( WHERE`
/// follows, so it names a select item and a column; not after GROUPING,
/// which is no aggregate function, nor before a parenthesis without
/// WHERE, each a syntax error.
#[test]
fn filter_names_a_column_5_2() {
    let (_db, a) = rows_of_groups();
    assert_eq!(sorted(&a, "SELECT FILTER FROM (SELECT COUNT(*) FILTER FROM T) AS Q"), ["8"]);
    a.run("CREATE TABLE N (FILTER INTEGER)").unwrap();
    a.run("INSERT INTO N VALUES (1), (2), (3)").unwrap();
    assert_eq!(sorted(&a, "SELECT SUM(FILTER) FILTER (WHERE FILTER > 1) FROM N"), ["5"]);
    assert_eq!(fetched(&a, "SELECT GROUPING(G) FILTER (WHERE V > 1) FROM T GROUP BY G").map_err(|e| e.code), Err(Code::Syntax));
    assert_eq!(fetched(&a, "SELECT COUNT(*) FILTER (V > 1) FROM T").map_err(|e| e.code), Err(Code::Syntax));
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.16, §10.9 GR4] an
/// assertion of a filtered set function, of a table's rows and of groups,
/// is checked as its query says, each filtered row counting and no other:
/// rows the filter does not take change nothing, one it takes past the
/// bound is refused.
#[test]
fn constraints_of_filtered_set_functions_10_9() {
    let (_db, a) = rows_of_groups();
    a.run("CREATE ASSERTION BIG CHECK ((SELECT COUNT(*) FILTER (WHERE V > 4) FROM T) < 3)").unwrap();
    a.run("CREATE ASSERTION HEAVY CHECK (NOT EXISTS (SELECT G FROM T GROUP BY G HAVING SUM(V) FILTER (WHERE V > 1) > 15))").unwrap();
    a.run("INSERT INTO T VALUES (9, 2, 1, 'i'), (10, 2, 4, 'j')").unwrap();
    assert_eq!(exec(&a, "INSERT INTO T VALUES (11, 2, 2, 'k')").err(), Some(Code::Constraint), "HEAVY");
    assert_eq!(exec(&a, "INSERT INTO T VALUES (12, 1, 6, 'l')").err(), Some(Code::Constraint), "BIG");
    a.run("INSERT INTO T VALUES (13, 1, 1, 'm')").unwrap();
    a.rollback().unwrap();
}
