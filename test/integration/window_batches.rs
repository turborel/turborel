//! Window functions over batches (RULES.md D560) against the same window
//! a row at a time (EXECUTOR.BATCHES OFF) [ISO/IEC 9075-2:2023 §6.10,
//! §7.15, §9.23]: every rank function, LEAD and LAG, each value function
//! and the built-in set functions, over frames of ROWS, RANGE and GROUPS
//! with every exclusion, partitions and orderings of numbers, strings
//! longer than a sort key's field and dates, nulls first and last, of a
//! table of rows and one of columns. Errors too must be the same.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{Rng, exec, rows};

/// Rows of W and WC from a fixed seed: partitions P with a null one,
/// ordering keys K with peers and nulls, exact D and approximate F with
/// nulls, strings S sharing a long start, and dates T.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE W (I INTEGER, P INTEGER, K INTEGER, D DECIMAL(9,2), F DOUBLE PRECISION, S VARCHAR(60), T DATE)
           CREATE TABLE WC (I INTEGER, P INTEGER, K INTEGER, D DECIMAL(9,2), F DOUBLE PRECISION, S VARCHAR(60), T DATE)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    let mut rng = Rng::new(0x5EED_0560);
    let mut values = Vec::new();
    let signed = |n: usize| i64::try_from(n).unwrap();
    for row in 0..300 {
        let part = if rng.chance(5) { "NULL".to_string() } else { rng.below(4).to_string() };
        let key = if rng.chance(8) { "NULL".to_string() } else { rng.below(40).to_string() };
        let exact = if rng.chance(10) { "NULL".to_string() } else { format!("{}.{:02}", signed(rng.below(2000)) - 1000, rng.below(100)) };
        let approx = if rng.chance(10) { "NULL".to_string() } else { format!("{}.5E{}", signed(rng.below(100)) - 50, rng.below(5)) };
        let text = format!("'a long shared start of a string, past the key: {}'", rng.below(6));
        let date = format!("DATE '2024-0{}-{:02}'", 1 + rng.below(3), 1 + rng.below(28));
        values.push(format!("({row}, {part}, {key}, {exact}, {approx}, {text}, {date})"));
    }
    for table in ["W", "WC"] {
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    (db, a)
}

/// The rows of `sql`, sorted, or its error's code: over batches and a row
/// at a time.
fn both(s: &Session, sql: &str) -> (Result<Vec<String>, Code>, Result<Vec<String>, Code>) {
    let sorted = |r: Result<Vec<String>, Code>| {
        r.map(|mut v| {
            v.sort();
            v
        })
    };
    let batched = sorted(rows(s, sql));
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let by_rows = sorted(rows(s, sql));
    exec(s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    (batched, by_rows)
}

/// The windows tried: partitioning and ordering.
const WINDOWS: [&str; 4] =
    ["PARTITION BY P ORDER BY K", "ORDER BY K DESC NULLS FIRST", "PARTITION BY S ORDER BY D", "PARTITION BY P ORDER BY T"];

/// The frames tried, each with the windows whose ordering it fits.
const FRAMES: [&str; 9] = [
    "",
    "ROWS BETWEEN 2 PRECEDING AND 1 FOLLOWING",
    "ROWS UNBOUNDED PRECEDING",
    "ROWS BETWEEN 1 FOLLOWING AND 3 FOLLOWING EXCLUDE CURRENT ROW",
    "RANGE BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING EXCLUDE GROUP",
    "RANGE BETWEEN 5 PRECEDING AND 2 FOLLOWING",
    "GROUPS BETWEEN 1 PRECEDING AND 1 FOLLOWING EXCLUDE TIES",
    "GROUPS BETWEEN 2 PRECEDING AND CURRENT ROW",
    "ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE TIES",
];

/// The functions over a frame.
const FRAMED: [&str; 16] = [
    "COUNT(*)",
    "COUNT(D)",
    "SUM(D)",
    "AVG(D)",
    "MIN(D)",
    "MAX(K)",
    "SUM(F)",
    "AVG(F)",
    "MIN(F)",
    "MAX(S)",
    "SUM(K * 2)",
    "FIRST_VALUE(D)",
    "LAST_VALUE(S)",
    "NTH_VALUE(D, 2)",
    "NTH_VALUE(F, 2) FROM LAST IGNORE NULLS",
    "LAST_VALUE(D) IGNORE NULLS",
];

/// The functions of no frame.
const UNFRAMED: [&str; 10] = [
    "ROW_NUMBER()",
    "RANK()",
    "DENSE_RANK()",
    "PERCENT_RANK()",
    "CUME_DIST()",
    "NTILE(7)",
    "LAG(D)",
    "LEAD(D, 2, 0)",
    "LAG(F, 1) IGNORE NULLS",
    "LEAD(S, 3) IGNORE NULLS",
];

/// `frame` as it fits `window`: a RANGE offset of a date is an interval.
fn fits(window: &str, frame: &str) -> String {
    if frame.contains("RANGE BETWEEN 5") && window.ends_with("BY T") {
        return "RANGE BETWEEN INTERVAL '5' DAY PRECEDING AND INTERVAL '2' DAY FOLLOWING".to_string();
    }
    frame.to_string()
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.10, §7.15 GR5, §9.23 SR5] each
/// window function over batches, of every frame type and exclusion, of
/// tables of rows and of columns, gives the rows and errors the window
/// gives a row at a time.
#[test]
fn windows_over_batches_are_those_of_rows() {
    let (_db, a) = tables();
    for table in ["W", "WC"] {
        for window in WINDOWS {
            for f in UNFRAMED {
                let sql = format!("SELECT I, {f} OVER ({window}) FROM {table}");
                let (batched, by_rows) = both(&a, &sql);
                assert_eq!(batched, by_rows, "{sql}");
            }
            for frame in FRAMES {
                let frame = fits(window, frame);
                for f in FRAMED {
                    let sql = format!("SELECT I, {f} OVER ({window} {frame}) FROM {table}");
                    let (batched, by_rows) = both(&a, &sql);
                    assert_eq!(batched, by_rows, "{sql}");
                }
            }
        }
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.15 GR5b i 1 A] [DESIGN] a RANGE
/// bound whose arithmetic overflows raises the error the window raises a
/// row at a time, over batches too, after giving way.
#[test]
fn a_range_bound_overflowing_raises_its_error_over_batches() {
    let (_db, a) = tables();
    let sql = "SELECT I, SUM(K) OVER (ORDER BY T RANGE BETWEEN INTERVAL '9999' YEAR(4) FOLLOWING AND UNBOUNDED FOLLOWING) FROM W";
    let (batched, by_rows) = both(&a, sql);
    assert_eq!(batched, by_rows);
    assert!(batched.is_err());
}
