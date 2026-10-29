//! \[DESIGN\] Exact numbers a batch holds as integers (RULES.md D31,
//! `storage::Ints`): judged and fed to set functions as integers, they
//! answer as row by row, of a table of the column store whose rows are
//! frozen, as a table of the row store answers.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{both_ways, exec, rows, sorted};

/// The rows written: K, a key; D, a DECIMAL(9, 2) of either sign; I, a
/// SMALLINT, one of 5; N, a NUMERIC(18), near its limits for some rows;
/// each but K null for every 7th row.
const ROWS: i64 = 5000;

/// Tables R, of the row store, and C, of the column store, alike: written,
/// C's rows frozen, then some changed and deleted in each.
fn both(db: &Database) -> Session {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER PRIMARY KEY, D DECIMAL(9, 2), I SMALLINT, N NUMERIC(18))
           CREATE TABLE C (K INTEGER PRIMARY KEY, D DECIMAL(9, 2), I SMALLINT, N NUMERIC(18))
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        for first in (0..ROWS).step_by(500) {
            let values: Vec<String> = (first..first + 500)
                .map(|k: i64| {
                    if k % 7 == 0 {
                        return format!("({k}, NULL, NULL, NULL)");
                    }
                    let d = format!("{}.{:02}", (k % 300) - 150, k % 100);
                    let n = match k % 11 {
                        0 => "999999999999999999".to_owned(),
                        1 => "-999999999999999999".to_owned(),
                        _ => (k * 1_000_003).to_string(),
                    };
                    format!("({k}, {d}, {}, {n})", k % 5)
                })
                .collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
        db.maintain().unwrap();
        a.run(&format!("UPDATE {table} SET D = D + 0.01, I = 9 WHERE K BETWEEN 100 AND 130")).unwrap();
        a.run(&format!("DELETE FROM {table} WHERE K BETWEEN 3000 AND 3050")).unwrap();
        a.commit().unwrap();
    }
    a
}

/// Queries of exact numbers: `{t}` the table, R or C.
const QUERIES: [&str; 16] = [
    "SELECT COUNT(*) FROM {t} WHERE D = 1.51",
    "SELECT COUNT(*) FROM {t} WHERE D = 1.510 OR D = -1.49",
    "SELECT COUNT(*) FROM {t} WHERE D = 1.515",
    "SELECT COUNT(*), SUM(D) FROM {t} WHERE D > -0.01 AND D <= 12.345",
    "SELECT COUNT(*) FROM {t} WHERE D <> 3 AND D >= -100",
    "SELECT COUNT(*) FROM {t} WHERE I IN (1, 2.0, 3.5)",
    "SELECT COUNT(*) FROM {t} WHERE I NOT IN (1, 2)",
    "SELECT COUNT(*) FROM {t} WHERE I NOT IN (1, CAST(NULL AS SMALLINT))",
    "SELECT COUNT(*) FROM {t} WHERE N > 999999999999999998",
    "SELECT COUNT(*) FROM {t} WHERE N = -999999999999999999 OR N < -999999999999999999",
    "SELECT COUNT(*), COUNT(D) FROM {t} WHERE D IS NULL OR I IS NOT NULL",
    "SELECT SUM(D), AVG(D), MIN(D), MAX(D), COUNT(D) FROM {t}",
    "SELECT SUM(N), MIN(N), MAX(N), AVG(I) FROM {t} WHERE N BETWEEN -1000000000000 AND 1000000000000",
    "SELECT I, SUM(D), MAX(N), COUNT(*) FROM {t} GROUP BY I",
    "SELECT SUM(D * 2), MIN(-D), MAX(N / 2) FROM {t} WHERE I = 9 OR K < 50",
    "SELECT K, D FROM {t} WHERE D BETWEEN 2.5 AND 2.75",
];

/// Verifies: [DESIGN] RULES.md D31 [X3.135-1986 §5.8 GR1, §5.11 GR4,
/// §5.13, §5.15]: exact numbers judged and fed to set functions as
/// integers give what they give row by row (EXECUTOR.BATCHES OFF), and
/// what a table of the row store gives: compared with constants of more
/// and fewer digits after the point than their scale, of either sign,
/// one that no value equals; IN and NOT IN lists of either, one holding
/// a null; numbers near the limits of 18 digits; nulls among them; set
/// functions grouped and not, of expressions; a running sum past 18
/// digits raising either way; rows changed and deleted after they were
/// frozen.
#[test]
fn exact_numbers_as_integers_answer_as_row_by_row() {
    let db = Database::new();
    let a = both(&db);
    for query in QUERIES {
        let column = query.replace("{t}", "C");
        let (batched, row) = both_ways(&a, &column);
        assert_eq!(batched, row, "{column}");
        assert_eq!(batched, sorted(&a, &query.replace("{t}", "R")), "{column}, as the row store");
    }
    assert_eq!(sorted(&a, "SELECT COUNT(*) FROM C WHERE D = 1.515"), vec!["0"], "no value of scale 2 is 1.515");
    // [X3.135-1986 §5.8 GR4e] a running sum past 18 digits raises, of
    // integers as of decimals, though the sum of all is within them.
    for table in ["R", "C"] {
        let sum = format!("SELECT SUM(N) FROM {table}");
        assert_eq!(rows(&a, &sum).err(), Some(Code::Data), "{sum}");
        exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
        assert_eq!(rows(&a, &sum).err(), Some(Code::Data), "{sum}, row by row");
        exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    }
    assert_ne!(sorted(&a, "SELECT COUNT(*) FROM C WHERE D = 1.510"), vec!["0"], "1.51 is 1.510");
}
