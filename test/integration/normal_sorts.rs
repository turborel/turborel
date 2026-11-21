//! \[DESIGN\] Sorts by normalized keys (RULES.md D364, D365): an ORDER BY of
//! a key of each kind, ASC and DESC, nulls first and last, of several keys
//! and under a collation, gives on the column store, past a work area of
//! 64 KiB, in parallel at a degree of 16 and by batches the rows the row
//! store gives row by row; and the first rows FETCH FIRST gives of a
//! stream's batches, ONLY and WITH TIES, are those the sort of rows held
//! by the comparator gives.

use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};
use crate::spilling::{SMALL, bytes};

/// The rows written to each table.
const ROWS: i64 = 4000;

/// The degree the parallel sorts ask for.
const DEGREE: i64 = 16;

/// The values of row `k`: I an integer, null every 7th; D a decimal of
/// two scales' values; F an approximate number, zeros of either sign; S a
/// string whose first 34 characters are shared, some differing only past
/// them or by trailing spaces; T a timestamp; B a truth value, null every
/// 5th; G one of 4.
fn row(k: i64) -> String {
    let int = if k % 7 == 0 { "NULL".to_owned() } else { (k * 7919 % 600 - 300).to_string() };
    let decimal = format!("{}.{:02}", k * 31 % 400 - 200, k % 3 * 25);
    let float = ["0E0", "-0E0", "1.5E0", "-2.5E2", "3E-3"][usize::try_from(k % 5).unwrap()];
    let text = format!("'a long shared start of a customer {}{}'", k * 13 % 50, ["", " ", "x", "  "][usize::try_from(k % 4).unwrap()]);
    let time = format!("TIMESTAMP '2021-03-04 {:02}:{:02}:{:02}'", k % 24, k * 7 % 60, k % 60);
    let truth = ["TRUE", "FALSE", "TRUE", "FALSE", "NULL"][usize::try_from(k % 5).unwrap()];
    format!("({k}, {int}, {decimal}, {float}, {text}, {time}, {truth}, {})", k % 4)
}

/// Tables R, of the row store, and C, of the column store, alike, C frozen.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    // As many workers as the degree whatever the machine's cores, so it is
    // not cut to fewer.
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(i128::from(DEGREE), 0))).unwrap();
    let columns = "K INTEGER, I INTEGER, D DECIMAL(9,2), F DOUBLE PRECISION, S VARCHAR(60), T TIMESTAMP, B BOOLEAN, G INTEGER";
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE R ({columns}) CREATE TABLE C ({columns}) USING columnstore")).unwrap();
    let s = db.session("A");
    for first in (0..ROWS).step_by(500) {
        let values: Vec<String> = (first..first + 500).map(row).collect();
        for table in ["R", "C"] {
            exec(&s, &format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    (db, s)
}

/// The ORDER BY clauses sorted by: each column each way under each null
/// ordering, several keys, and a string under a collation.
fn orders() -> Vec<String> {
    let mut orders = Vec::new();
    for column in ["I", "D", "F", "S", "T", "B"] {
        for way in ["ASC", "DESC"] {
            for nulls in ["", " NULLS FIRST", " NULLS LAST"] {
                orders.push(format!("{column} {way}{nulls}"));
            }
        }
    }
    orders.extend(["G DESC, I NULLS FIRST, D".to_owned(), "B, S DESC, F".to_owned(), "S COLLATE UNICODE, I DESC".to_owned()]);
    orders
}

/// The rows of `sql` of `table`, each ORDER BY of `orders`, under `set`.
fn sorted(s: &Session, (table, set): (&str, &[&str]), orders: &[String]) -> Vec<Vec<String>> {
    for statement in set {
        exec(s, statement).unwrap();
    }
    let made = orders.iter().map(|o| rows(s, &format!("SELECT * FROM {table} ORDER BY {o}")).unwrap()).collect();
    for statement in ["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.PARALLEL_MIN_ROWS = 65536"] {
        exec(s, statement).unwrap();
    }
    made
}

/// Verifies: [X3.135-1986 §8.3 GR3b] [ISO/IEC 9075-2:2023 §10.10 GR1e ii]
/// [DESIGN] RULES.md D364: an ORDER BY of a key of each kind, ASC and DESC,
/// nulls first and last, of several keys and under a collation, gives the
/// rows the row store gives row by row, in order, stably: of either store
/// by batches, past a work area of 64 KiB, and in parallel at a degree of
/// 16, its runs spilled past 64 KiB too (D380).
#[test]
fn normal_sorts_give_what_rows_give() {
    let (db, s) = tables();
    let orders = orders();
    let serial = ["SET EXECUTOR.DEGREE = 1"];
    let expected = sorted(&s, ("R", &["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'OFF'"]), &orders);
    assert!(expected.iter().all(|r| r.len() == usize::try_from(ROWS).unwrap()));
    let parallel = ["SET EXECUTOR.DEGREE = 16", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"];
    for table in ["R", "C"] {
        assert_eq!(sorted(&s, (table, &serial), &orders), expected, "{table} by batches");
        assert_eq!(sorted(&s, (table, &parallel), &orders), expected, "{table} in parallel");
    }
    db.set_setting("MEMORY.WORK_BYTES", &bytes(SMALL)).unwrap();
    for table in ["R", "C"] {
        assert_eq!(sorted(&s, (table, &serial), &orders), expected, "{table} past 64 KiB");
        assert_eq!(sorted(&s, (table, &parallel), &orders), expected, "{table} in parallel past 64 KiB");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR5, GR7d] [DESIGN] RULES.md D365:
/// FETCH FIRST of a stream's batches, of either store, ONLY and WITH TIES,
/// few rows and many, gives the rows the sort of rows held by the
/// comparator gives, past a work area of 64 KiB too.
#[test]
fn first_rows_of_batches_are_those_of_rows() {
    let (db, s) = tables();
    let mut fetches = Vec::new();
    for o in ["I", "S DESC NULLS FIRST", "B, G DESC", "S COLLATE UNICODE"] {
        for (n, ties) in [(1, "ONLY"), (10, "WITH TIES"), (3000, "ONLY"), (3000, "WITH TIES")] {
            fetches.push(format!("{o} FETCH FIRST {n} ROWS {ties}"));
        }
    }
    let expected = sorted(&s, ("R", &["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'OFF'"]), &fetches);
    let serial = ["SET EXECUTOR.DEGREE = 1"];
    for table in ["R", "C"] {
        assert_eq!(sorted(&s, (table, &serial), &fetches), expected, "{table} by batches");
    }
    db.set_setting("MEMORY.WORK_BYTES", &bytes(SMALL)).unwrap();
    assert_eq!(sorted(&s, ("C", &serial), &fetches), expected, "past 64 KiB");
}
