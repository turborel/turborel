//! \[EXTENSION\] The column store's decoding (RULES.md D352, D353): a
//! table of the column store whose chunks are frames of reference of odd
//! widths, dictionaries of numbers and of texts, runs, decimals and texts
//! coded by a table of symbols reads every value as a table of the row
//! store holds it, whole and by the rows its tree shadows.

use turborel::db::Database;
use turborel::threads::JobPolicy;

use crate::common::sorted;

/// The rows of each table.
const ROWS: i64 = 6000;

/// Row `k`'s values: a frame of 19 bits, one of 61, a dictionary of 40
/// numbers far apart, runs, a decimal of two digits or an exception, a
/// CHARACTER text of 25 values and a VARCHAR text of each row; every
/// 13th value but K's null.
fn row(k: i64) -> String {
    let null = |v: String| if k % 13 == 6 { "NULL".to_owned() } else { v };
    let p = match k % 101 {
        1 => "-0.0E0".to_owned(),
        2 => "1.7976931348623157E308".to_owned(),
        3 => "-4.9E-324".to_owned(),
        4 => "3.3333333333333335E-1".to_owned(),
        _ => format!("{}.{:02}E0", k * 37 % 5000 - 2500, k % 100),
    };
    format!(
        "({k}, {}, {}, {}, {}, {}, {}, {})",
        null((k * 7919 % 500_000).to_string()),
        null((k * 104_729 % 1_000_000_007 * 1_000_000_007 - 4_611_686_018_427_387_904).to_string()),
        null((k % 40 * 1_000_000_000_039 - 4_500_000_000_000_000_000_i64).to_string()),
        null(k.checked_div(600).unwrap().to_string()),
        null(p),
        null(format!("'n{}'", k % 25)),
        null(format!("'customer{k}@mail{}.example.com'", k % 7)),
    )
}

/// The queries each table answers alike, `{t}` its name.
const QUERIES: [&str; 6] = [
    "SELECT * FROM {t}",
    "SELECT SUM(A), SUM(D), SUM(R), MIN(B), MAX(B), COUNT(P) FROM {t}",
    "SELECT K, P FROM {t} WHERE P > 0.0E0",
    "SELECT K, T, U FROM {t} WHERE T = 'n7' OR U LIKE 'customer1%'",
    "SELECT T, COUNT(*), SUM(A) FROM {t} GROUP BY T",
    "SELECT K, D, R FROM {t} WHERE D > 0 AND R < 5",
];

/// Verifies: [EXTENSION] RULES.md D352, D353, decoding: a table of the
/// column store, frozen, of integers in frames of odd widths, near the
/// extremes, in a dictionary and in runs, of approximate numbers as
/// decimals and exceptions (-0, the greatest, the least denormal, a
/// third), of CHARACTER and VARCHAR texts, nulls among them all, answers
/// as a table of the row store, whole and once updates leave some of its
/// rows to its tree, so the rest are read by position [X3.135-1986 §5.8,
/// §5.11 GR4, §8.6].
#[test]
fn every_encoding_decodes_as_written() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let columns = "K INTEGER PRIMARY KEY, A INTEGER, B BIGINT, D BIGINT, R INTEGER, P DOUBLE PRECISION, T CHARACTER(6), U VARCHAR(60)";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE RD ({columns})
           CREATE TABLE CD ({columns}) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 4096)"
    ))
    .unwrap();
    let a = db.session("A");
    for table in ["RD", "CD"] {
        for first in (0..ROWS).step_by(500) {
            let values: Vec<String> = (first..first + 500).map(row).collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    for q in QUERIES {
        assert_eq!(sorted(&a, &q.replace("{t}", "CD")), sorted(&a, &q.replace("{t}", "RD")), "{q}");
    }
    for table in ["RD", "CD"] {
        a.run(&format!("UPDATE {table} SET A = A + 1 WHERE K / 97 * 97 = K")).unwrap();
    }
    a.commit().unwrap();
    for q in QUERIES {
        assert_eq!(sorted(&a, &q.replace("{t}", "CD")), sorted(&a, &q.replace("{t}", "RD")), "shadowed: {q}");
    }
}
