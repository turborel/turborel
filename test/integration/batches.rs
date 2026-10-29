//! \[DESIGN\] Set functions computed over a table read a batch at a time
//! (RULES.md D31, `TableStorage::scan_batches`): the same results as row by
//! row, of either layout, frozen rows changed among them; and the rows they
//! read counted as read, for serializability.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{exec, rows, sorted};

/// The rows written: K, a key; G, one of 7 groups; S, a text, null for
/// every 11th; N, a number in the order the rows were written, null for
/// every 13th.
const ROWS: i64 = 6000;

/// Tables R, of the ROWS layout, and C, of COLUMNS in groups of 2048, of
/// schema A, alike: written, their rows frozen, then some changed,
/// deleted and added, in each.
fn both(db: &Database) -> Session {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, S VARCHAR(20), N INTEGER)
           CREATE TABLE C (K INTEGER PRIMARY KEY, G INTEGER, S VARCHAR(20), N INTEGER)
             WITH (LAYOUT = 'COLUMNS', COLUMNS.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        for first in (0..ROWS).step_by(500) {
            let values: Vec<String> = (first..first + 500)
                .map(|k| {
                    let s = if k % 11 == 0 { "NULL".to_owned() } else { format!("'s{}'", k % 97) };
                    let n = if k % 13 == 0 { "NULL".to_owned() } else { k.checked_div(10).unwrap().to_string() };
                    format!("({k}, {}, {s}, {n})", k % 7)
                })
                .collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
        a.run(&format!("UPDATE {table} SET N = -N, S = 'changed' WHERE K BETWEEN 100 AND 150")).unwrap();
        a.run(&format!("DELETE FROM {table} WHERE K BETWEEN 3000 AND 3100 OR G = 6 AND K < 500")).unwrap();
        a.run(&format!("INSERT INTO {table} VALUES (-1, 1, 'new', 5), (-2, 2, NULL, NULL)")).unwrap();
        a.commit().unwrap();
    }
    a
}

/// Set functions, of no GROUP BY, read by batches where they can be.
const QUERIES: [&str; 15] = [
    "SELECT COUNT(*), SUM(N) FROM {t} WHERE G = 3 AND S IS NOT NULL AND 100 < K",
    "SELECT COUNT(*) FROM {t} WHERE N >= 100 AND N < 200 AND (S = 's1' OR S = 's2')",
    "SELECT COUNT(*) FROM {t} WHERE G <> 1 AND K > N",
    "SELECT COUNT(*) FROM {t}",
    "SELECT COUNT(*), COUNT(S), COUNT(N), SUM(N), AVG(N), MIN(N), MAX(N) FROM {t}",
    "SELECT MIN(S), MAX(S), MIN(K), MAX(K) FROM {t}",
    "SELECT COUNT(*), SUM(N) FROM {t} WHERE K BETWEEN 2000 AND 2100",
    "SELECT COUNT(*), SUM(N) FROM {t} WHERE K >= 140 AND K < 160",
    "SELECT COUNT(*), MAX(K) FROM {t} WHERE S = 's5'",
    "SELECT COUNT(*), SUM(K) FROM {t} WHERE G = 3 AND N > 250",
    "SELECT COUNT(*) FROM {t} WHERE S IS NULL",
    "SELECT COUNT(*), SUM(N) FROM {t} WHERE K < -100",
    "SELECT COUNT(*) FROM {t} WHERE N <> 7",
    "SELECT SUM(N + 0), COUNT(DISTINCT G) FROM {t}",
    "SELECT COUNT(*) FROM {t} WHERE S = 'changed'",
];

/// What `s` reads of table `table` by each query.
fn answers(s: &Session, table: &str) -> Vec<Vec<String>> {
    QUERIES.iter().map(|q| rows(s, &q.replace("{t}", table)).unwrap()).collect()
}

/// Verifies: [DESIGN] RULES.md D31, set functions read by batches: COUNT,
/// SUM, AVG, MIN and MAX of no GROUP BY, of a table of each layout, give
/// what they give row by row: nulls left out, frozen rows changed, deleted
/// and added to, under filters a scan is told and not, none matched; and
/// a SUM over an expression, grouped row by row, agrees with one over the
/// column.
#[test]
fn set_functions_by_batch_answer_as_row_by_row() {
    let db = Database::new();
    let a = both(&db);
    let columns = answers(&a, "C");
    assert_eq!(columns, answers(&a, "R"));
    let sums = rows(&a, "SELECT SUM(N), SUM(N + 0) FROM C").unwrap();
    let pair: Vec<&str> = sums[0].split(',').collect();
    assert_eq!(pair[0], pair[1], "by batch and row by row");
    assert_eq!(columns[11], vec!["0,NULL".to_owned()], "no row: a count of 0 and a null sum");
}

/// Verifies: [DESIGN] RULES.md D31, D29, serializability under batches:
/// a set function read by batches counts the rows it reads as read, so of
/// two transactions each reading what the other writes, the second to
/// commit is refused (SQLCODE -911), in a table of either layout.
#[test]
fn rows_read_by_batch_are_read() {
    let db = Database::new();
    let a = both(&db);
    for table in ["R", "C"] {
        let (one, two) = (db.session("A"), db.session("A"));
        assert_eq!(rows(&one, &format!("SELECT COUNT(*) FROM {table} WHERE K = -1")), Ok(vec!["1".to_owned()]));
        assert_eq!(rows(&two, &format!("SELECT SUM(N) FROM {table}")).map(|r| r.len()), Ok(1));
        exec(&one, &format!("UPDATE {table} SET N = 0 WHERE K = 3")).unwrap();
        exec(&two, &format!("UPDATE {table} SET N = 0 WHERE K = -1")).unwrap();
        one.commit().unwrap();
        assert_eq!(two.commit().err().map(|e| e.code), Some(Code::Conflict), "{table}");
    }
    drop(a);
}

/// Grouped set functions, read by batches where they can be; sorted, as
/// groups are given in no order an ORDER BY does not give.
const GROUPED: [&str; 6] = [
    "SELECT G, COUNT(*), COUNT(S), SUM(N), AVG(N), MIN(S), MAX(K) FROM {t} GROUP BY G",
    "SELECT S, COUNT(*) FROM {t} GROUP BY S",
    "SELECT G, S, SUM(N) FROM {t} WHERE K BETWEEN 1000 AND 2500 GROUP BY G, S",
    "SELECT G, COUNT(*) FROM {t} WHERE S = 's5' AND N > 10 GROUP BY G",
    "SELECT G, COUNT(*) FROM {t} WHERE K < -100 GROUP BY G",
    "SELECT G, SUM(N + 0), COUNT(DISTINCT S) FROM {t} GROUP BY G",
];

/// Verifies: [DESIGN] RULES.md D31, grouped set functions read by batches:
/// each group's set functions give what they give row by row, of a table
/// of each layout, frozen rows changed among them, grouped by one column
/// or two, nulls a group of their own, filtered or not, no row matched;
/// and groups past the work area, their rows written and grouped as row
/// by row groups them, given in the order their first rows came.
#[test]
fn groups_by_batch_answer_as_row_by_row() {
    let db = Database::new();
    let a = both(&db);
    let grouped = |s: &Session, table: &str| -> Vec<Vec<String>> { GROUPED.iter().map(|q| sorted(s, &q.replace("{t}", table))).collect() };
    let columns = grouped(&a, "C");
    assert_eq!(columns, grouped(&a, "R"));
    assert_eq!(columns[1].len(), 100, "97 texts, 'changed', 'new' and null");
    assert!(columns[4].is_empty(), "no row, no group");
    // One group for each row: past a work area of 64 KiB.
    exec(&a, "SET WORK_MEMORY = 65536").unwrap();
    let many = "SELECT K, COUNT(*), SUM(N) FROM {t} GROUP BY K";
    assert_eq!(sorted(&a, &many.replace("{t}", "C")), sorted(&a, &many.replace("{t}", "R")));
    // A table of rows gives its batches in the order of its scan: its
    // groups by batch come in the order row by row gives them.
    let by_row: Vec<String> = rows(&a, "SELECT K, COUNT(*), SUM(N), SUM(N + 0) FROM R GROUP BY K")
        .unwrap()
        .iter()
        .map(|r| r.rsplit_once(',').unwrap().0.to_owned())
        .collect();
    assert_eq!(rows(&a, &many.replace("{t}", "R")).unwrap(), by_row);
}

/// Verifies: [DESIGN] RULES.md D31, D38: grouped by batch, datetimes and
/// intervals make the groups they make row by row: dates, intervals, and
/// timestamps of one instant written at two time zones, one group.
#[test]
fn datetimes_group_by_batch_as_row_by_row() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, D DATE, I INTERVAL DAY TO HOUR, W TIMESTAMP WITH TIME ZONE)
           WITH (LAYOUT = 'COLUMNS', COLUMNS.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    for first in (0..3000).step_by(500) {
        let values: Vec<String> = (first..first + 500)
            .map(|k| {
                let zone = if k % 2 == 0 { "+00:00" } else { "+01:00" };
                let hour = if k % 2 == 0 { 10 } else { 11 };
                format!(
                    "({k}, DATE '2026-10-{:02}', INTERVAL '{} {}' DAY TO HOUR, TIMESTAMP '2026-10-28 {hour}:00:00{zone}')",
                    k % 28 + 1,
                    k % 5,
                    k % 3
                )
            })
            .collect();
        a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    for column in ["D", "I", "W"] {
        let by_batch = sorted(&a, &format!("SELECT {column}, COUNT(*) FROM T GROUP BY {column}"));
        let by_row: Vec<String> = sorted(&a, &format!("SELECT {column}, COUNT(*), SUM(K + 0) FROM T GROUP BY {column}"))
            .iter()
            .map(|r| r.rsplit_once(',').unwrap().0.to_owned())
            .collect();
        assert_eq!(by_batch, by_row, "{column}");
    }
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM (SELECT W FROM T GROUP BY W) X"), Ok(vec!["1".to_owned()]), "one instant");
}

/// Verifies: [DESIGN] RULES.md D31, [X3.135-1986 §5.9 GR5]: a filter that
/// can raise an error raises it by batch as row by row, judged whole a row
/// at a time, not narrowed first by its conjuncts judged over vectors.
#[test]
fn a_filter_that_raises_raises_by_batch() {
    let db = Database::new();
    let a = both(&db);
    for table in ["R", "C"] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE K > 5000 AND 10 / (G - G) > 0");
        assert_eq!(rows(&a, &sql).err(), Some(Code::Data), "{table}: division by zero");
    }
}

/// Projections read by batches where they can be; sorted, as their rows
/// come in no order an ORDER BY does not give.
const PROJECTIONS: [&str; 6] = [
    "SELECT K, S FROM {t}",
    "SELECT N, 1, 'x', S FROM {t} WHERE G = 2 AND S IS NOT NULL",
    "SELECT K FROM {t} WHERE K BETWEEN 2000 AND 2100",
    "SELECT S, K FROM {t} WHERE S = 'changed'",
    "SELECT K + 1, S FROM {t} WHERE N > 300",
    "SELECT K FROM {t} WHERE K < -100",
];

/// Verifies: [DESIGN] RULES.md D31, projections read by batches: of a table
/// of the COLUMNS layout, columns and constants give the rows a table of
/// ROWS gives, frozen rows changed among them, filtered or not, none
/// matched; one of an expression is made row by row, alike; and a cursor
/// reading a few of its rows reads them.
#[test]
fn projections_by_batch_answer_as_row_by_row() {
    let db = Database::new();
    let a = both(&db);
    let projected = |table: &str| -> Vec<Vec<String>> { PROJECTIONS.iter().map(|q| sorted(&a, &q.replace("{t}", table))).collect() };
    let columns = projected("C");
    assert_eq!(columns, projected("R"));
    assert_eq!(columns[0].len(), 6000 - 101 - 71 + 2, "every row");
    assert_eq!(columns[5], Vec::<String>::new());
    let mut cursor = a.open_cursor("SELECT K, S FROM C").unwrap();
    for _ in 0..3 {
        assert_eq!(cursor.fetch().unwrap().map(|r| r.len()), Some(2));
    }
    drop(cursor);
    a.commit().unwrap();
}
