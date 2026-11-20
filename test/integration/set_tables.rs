//! \[DESIGN\] EXCEPT and INTERSECT counted in the executor's table of
//! groups (RULES.md D359): of rows read one at a time and by batch, held in
//! the work area or past it, serial and in parallel, they keep the rows,
//! duplicates and order [ISO/IEC 9075:1992 §7.10 GR1b] gives, as a model of
//! the operands counts them.

use std::collections::HashMap;

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Rows of L, the first operand.
const FIRST: i64 = 3000;

/// Rows of M, the second operand.
const SECOND: i64 = 2000;

/// A row of L: A of 2,400 values, null every 13th; B of 3.
fn first_row(k: i64) -> (Option<i64>, i64) {
    ((k % 13 != 0).then_some(k % 2400), k % 3)
}

/// A row of M: A of 1,999 values, null every 17th; B of 3; so many keys
/// that their table outgrows a work area of 64 KiB.
fn second_row(k: i64) -> (Option<i64>, i64) {
    ((k % 17 != 0).then_some(k % 1999), k % 3)
}

/// A row as `rows` shows it.
fn shown((a, b): (Option<i64>, i64)) -> String {
    format!("{},{b}", a.map_or_else(|| "NULL".to_owned(), |a| a.to_string()))
}

/// The rows of L EXCEPT or INTERSECT, `ALL` or not, M keeps, in L's order:
/// of ALL each duplicate in M matches the first unmatched in L; else each
/// row once, its first.
fn modelled((intersect, all): (bool, bool)) -> Vec<String> {
    let mut counts: HashMap<(Option<i64>, i64), i64> = HashMap::new();
    for k in 0..SECOND {
        *counts.entry(second_row(k)).or_default() += 1;
    }
    let mut given = std::collections::HashSet::new();
    let mut kept = Vec::new();
    for k in 0..FIRST {
        let row = first_row(k);
        let n = counts.entry(row).or_default();
        let keeps = if all {
            let matched = *n > 0;
            *n -= i64::from(matched);
            matched == intersect
        } else {
            (*n > 0) == intersect && given.insert(row)
        };
        if keeps {
            kept.push(shown(row));
        }
    }
    kept
}

/// L and M, written; four workers whatever the machine's cores.
fn operands() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE L (A INTEGER, B INTEGER)
           CREATE TABLE M (A INTEGER, B INTEGER)",
    )
    .unwrap();
    let s = db.session("S");
    for (table, count, row) in [("L", FIRST, first_row as fn(i64) -> (Option<i64>, i64)), ("M", SECOND, second_row)] {
        let values: Vec<String> = (0..count).map(|k| format!("({})", shown(row(k)))).collect();
        s.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    (db, s)
}

/// Verifies: [ISO/IEC 9075:1992 §7.10 GR1b] [DESIGN] RULES.md D359, D38,
/// D295: EXCEPT, EXCEPT ALL, INTERSECT and INTERSECT ALL of operands with
/// duplicates and nulls keep, in the first operand's order, the rows the
/// model keeps: read a row at a time and by batch, counted in the table,
/// past a work area too small for it, and in parallel by partitions.
#[test]
fn set_operations_keep_what_their_counts_say() {
    let (_db, s) = operands();
    for (op, intersect) in [("EXCEPT", false), ("INTERSECT", true)] {
        for all in [false, true] {
            let sql = format!("SELECT A, B FROM L {op}{} SELECT A, B FROM M", if all { " ALL" } else { "" });
            let expected = modelled((intersect, all));
            assert!(!expected.is_empty(), "{sql}");
            for (how, settings) in [
                ("a row at a time", &["SET EXECUTOR.BATCHES = 'OFF'"][..]),
                ("by batch", &["SET EXECUTOR.BATCHES = 'ON'"][..]),
                ("past the work area", &["SET MEMORY.WORK_BYTES = 65536"][..]),
                ("in parallel", &["SET MEMORY.WORK_BYTES TO DEFAULT", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256"][..]),
            ] {
                for set in settings {
                    exec(&s, set).unwrap();
                }
                exec(&s, "SET EXECUTOR.PARALLEL_MIN_ROWS = 0").unwrap();
                let parallel = s.physical_plan(&sql, &Params::new()).unwrap().contains("Parallel");
                assert_eq!(parallel, how == "in parallel", "{sql}, {how}");
                assert_eq!(rows(&s, &sql), Ok(expected.clone()), "{sql}, {how}");
            }
            exec(&s, "SET EXECUTOR.DEGREE = 1").unwrap();
        }
    }
}

/// Verifies: [ISO/IEC 9075:1992 §7.10 GR1b i] [DESIGN] RULES.md D359: rows
/// whose values compare equal are duplicates in the table whatever their
/// spelling: texts with and without trailing spaces, decimals of one value
/// at two scales, nulls; the first operand's row kept is its own, as it was
/// written, by batch and a row at a time alike.
#[test]
fn equal_values_are_duplicates_in_the_table() {
    let (_db, s) = operands();
    s.run("CREATE TABLE P (T VARCHAR(8), D DECIMAL(6, 2))").unwrap();
    s.run("CREATE TABLE Q (T VARCHAR(8), D DECIMAL(6, 1))").unwrap();
    s.run("INSERT INTO P VALUES ('a  ', 1.50), ('b', 2.00), (NULL, NULL), ('a', 1.5), ('c', 3.25), ('b ', 2)").unwrap();
    s.run("INSERT INTO Q VALUES ('a', 1.5), (NULL, NULL), ('b  ', 2.0)").unwrap();
    s.commit().unwrap();
    for batches in ["OFF", "ON"] {
        exec(&s, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        let rows_of = |op: &str| rows(&s, &format!("SELECT T, D FROM P {op} SELECT T, D FROM Q")).unwrap();
        assert_eq!(rows_of("EXCEPT"), ["c,3.25"], "{batches}");
        assert_eq!(rows_of("INTERSECT"), ["a  ,1.50", "b,2.00", "NULL,NULL"], "{batches}");
        assert_eq!(rows_of("INTERSECT ALL"), ["a  ,1.50", "b,2.00", "NULL,NULL"], "{batches}");
        assert_eq!(rows_of("EXCEPT ALL"), ["a,1.50", "c,3.25", "b ,2.00"], "{batches}");
    }
}
