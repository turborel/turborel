//! \[DESIGN\] Set functions of no grouping run in parallel (RULES.md
//! D1170): each morsel's one group is fed its batches' vectors as a set
//! function of no grouping is fed, no row hashed, and the groups merged
//! give what the plan run alone a row at a time gives, of every kind of
//! set function and value, of a filter, of DISTINCT and FILTER, of no
//! rows, and with the memory a grouping may hold too small for it.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Tables C, of the column store in groups of 2048, and R, of the row
/// store, alike: K, a key; N, an integer, null for every 13th; D, a
/// decimal, null for every 17th; F, an approximate number; T, a text,
/// null for every 19th; W, a date; B, a truth value, null for every 23rd.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    let columns = "K INTEGER PRIMARY KEY, N INTEGER, D DECIMAL(9,2), F DOUBLE PRECISION, T VARCHAR(10), W DATE, B BOOLEAN";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE C ({columns}) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE R ({columns})"
    ))
    .unwrap();
    let a = db.session("A");
    let or_null = |null: bool, v: String| if null { "NULL".to_owned() } else { v };
    for table in ["C", "R"] {
        let values: Vec<String> = (0_i64..7000)
            .map(|k| {
                let int = or_null(k % 13 == 0, (k * 7919 % 100 - 40).to_string());
                let dec = or_null(k % 17 == 0, format!("{}.{:02}", k * 31 % 9000 - 4000, k % 100));
                let approx = format!("{}.5E0", k * 17 % 1000);
                let text = or_null(k % 19 == 0, format!("'t{}'", k * 101 % 7000));
                let date = format!("DATE '20{:02}-{:02}-{:02}'", k % 30, k % 12 + 1, k % 28 + 1);
                let truth = or_null(k % 23 == 0, (k % 5 != 0).to_string().to_uppercase());
                format!("({k}, {int}, {dec}, {approx}, {text}, {date}, {truth})")
            })
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The rows of a query, or the code it failed with.
type Rows = Result<Vec<String>, turborel::error::Code>;

/// The rows of `sql` run alone a row at a time, its rows run in parallel
/// on 4 threads in morsels of 256 rows, and its plan then.
fn both(s: &Session, sql: &str) -> (Rows, Rows, String) {
    exec(s, "SET EXECUTOR.DEGREE = 1").unwrap();
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let alone = rows(s, sql);
    for set in
        ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]
    {
        exec(s, set).unwrap();
    }
    let shown = s.physical_plan(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e}"));
    (alone, rows(s, sql), shown)
}

/// Verifies: [X3.135-1986 §5.8 GR1, GR4] (RULES.md D1170) set functions
/// of no grouping run in parallel give what they give run alone a row at
/// a time: COUNT, SUM, AVG, MIN and MAX of integers, decimals and
/// approximate numbers, of which a sum is not run in parallel, MIN and
/// MAX of texts and dates, EVERY and SOME,
/// of expressions, of the rows a condition keeps, of DISTINCT and FILTER
/// arguments, and of a few rows, most morsels none; and so with the
/// memory a grouping may hold too small for its groups, run in parallel
/// or not.
#[test]
fn set_functions_of_no_grouping_are_fed_by_vectors_in_parallel() {
    let (_db, a) = tables();
    // Each shape, and whether its set functions run in parallel: a sum of
    // approximate numbers, whose total depends on its order, does not.
    let shapes = [
        ("SELECT COUNT(*), COUNT(N), SUM(N), AVG(N), MIN(N), MAX(N) FROM {t}", true),
        ("SELECT SUM(D), AVG(D), MIN(D), MAX(D), COUNT(D) FROM {t}", true),
        ("SELECT SUM(F), AVG(F), MIN(F), MAX(F), COUNT(F) FROM {t}", false),
        ("SELECT MIN(F), MAX(F), COUNT(F) FROM {t}", true),
        ("SELECT MIN(T), MAX(T), COUNT(T), MIN(W), MAX(W), COUNT(W) FROM {t}", true),
        ("SELECT EVERY(B), SOME(B), COUNT(B) FROM {t} WHERE K > 10", true),
        ("SELECT SUM(N * 2 + K), SUM(D * N), COUNT(*) FROM {t} WHERE MOD(K, 3) = 1", true),
        ("SELECT COUNT(DISTINCT N), SUM(DISTINCT N), COUNT(*) FROM {t}", true),
        ("SELECT COUNT(*) FILTER (WHERE N > 50), SUM(N) FILTER (WHERE K < 3000), MAX(D) FROM {t}", true),
        ("SELECT SUM(K), COUNT(N), MIN(T) FROM {t} WHERE K + N > 7020", true),
    ];
    for work in ["SET MEMORY.WORK_BYTES = 67108864", "SET MEMORY.WORK_BYTES = 65536"] {
        exec(&a, work).unwrap();
        for table in ["C", "R"] {
            for (shape, parallel) in shapes {
                let sql = shape.replace("{t}", table);
                let (alone, together, shown) = both(&a, &sql);
                assert!(!parallel || shown.contains("Parallel"), "{sql}\n{shown}");
                assert!(alone.as_ref().is_ok_and(|r| r.len() == 1), "{sql}: {alone:?}");
                assert_eq!(alone, together, "{work}: {sql}");
            }
        }
    }
}

/// Verifies: [X3.135-1986 §5.8 GR2, §5.25 GR1] (RULES.md D1170) set
/// functions of no grouping of no rows, run in parallel, are one row:
/// COUNT zero, every other the null value.
#[test]
fn set_functions_of_no_rows_in_parallel_are_one_row() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        let sql = format!("SELECT COUNT(*), COUNT(N), SUM(N), AVG(D), MIN(T), MAX(W) FROM {table} WHERE K + N < -100");
        let (alone, together, shown) = both(&a, &sql);
        assert!(shown.contains("Parallel"), "{sql}\n{shown}");
        assert_eq!(together, alone, "{sql}");
        assert_eq!(together.unwrap().concat(), "0,0,NULL,NULL,NULL,NULL", "{sql}");
    }
}
