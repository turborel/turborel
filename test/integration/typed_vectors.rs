//! \[DESIGN\] Exact numbers a batch holds as integers, and character
//! strings it holds as text (RULES.md D31, `storage::Ints`, `Texts`):
//! judged and fed to set functions so, they answer as row by row, of a
//! table of the column store whose rows are frozen, as a table of the row
//! store answers.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;

use crate::common::{both_ways, exec, rows, sorted};

/// The rows written: K, a key; D, a DECIMAL(9, 2) of either sign; I, a
/// SMALLINT, one of 5; N, a NUMERIC(18), near its limits for some rows; V,
/// a VARCHAR(12), of 40 values and some not ASCII, a trailing space on
/// every third; each but K null for every 7th row.
const ROWS: i64 = 10_000;

/// Tables R, of the row store, and C, of the column store, alike: written,
/// C's rows frozen, then some changed and deleted in each.
fn both(db: &Database) -> Session {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER PRIMARY KEY, D DECIMAL(9, 2), I SMALLINT, N NUMERIC(18), V VARCHAR(12))
           CREATE TABLE C (K INTEGER PRIMARY KEY, D DECIMAL(9, 2), I SMALLINT, N NUMERIC(18), V VARCHAR(12))
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        for first in (0..ROWS).step_by(500) {
            let values: Vec<String> = (first..first + 500)
                .map(|k: i64| {
                    if k % 7 == 0 {
                        return format!("({k}, NULL, NULL, NULL, NULL)");
                    }
                    let d = format!("{}.{:02}", (k % 300) - 150, k % 100);
                    let n = match k % 11 {
                        0 => "999999999999999999".to_owned(),
                        1 => "-999999999999999999".to_owned(),
                        _ => (k * 1_000_003).to_string(),
                    };
                    let v = if k % 17 == 0 { format!("é{}", k % 5) } else { format!("v{}{}", k % 40, if k % 3 == 0 { " " } else { "" }) };
                    format!("({k}, {d}, {}, {n}, '{v}')", k % 5)
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
const QUERIES: [&str; 40] = [
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
    "SELECT I, SUM(D * 2 - I), MIN(N / 1000), MIN(D + 0.005) FROM {t} GROUP BY I",
    "SELECT I, D, COUNT(*) FROM {t} WHERE K < 900 GROUP BY I, D",
    "SELECT COUNT(*) FROM {t} WHERE D < 1.5 AND D >= -2 AND I <= 3 AND I <> 2 AND N > -5",
    "SELECT COUNT(*) FROM {t} WHERE I IN (1.0E0, 2) OR D = 2.5E0",
    "SELECT SUM(-D), SUM(D - I * 0.5), SUM(I * D * 0.001), MAX(N * 2) FROM {t} WHERE N BETWEEN -100000000000000000 AND 100000000000000000",
    "SELECT SUM(D * 0.0000000000000001), SUM(K / 2), MIN(I - D) FROM {t} WHERE K < 3000",
    "SELECT I, SUM(-D + 1), MIN(D * I) FROM {t} GROUP BY I",
    "SELECT I, SUM(3), COUNT(K / 2), MAX(D * 2.5E0) FROM {t} GROUP BY I",
    "SELECT COUNT(*), SUM(D) FROM {t} WHERE D * 2 > 3 AND I < 4",
    "SELECT COUNT(*) FROM {t} WHERE D - 5 < 0 AND K + 1 > 100 AND D <> I",
    "SELECT COUNT(*), MIN(K) FROM {t} WHERE D * D > 1.5 AND -D <= 0.25",
    "SELECT COUNT(*) FROM {t} WHERE K / 2 > 1000 AND D * 2 > 0",
    "SELECT COUNT(*), SUM(D) FROM {t} WHERE D > I AND -N < K",
    "SELECT COUNT(*), MIN(K) FROM {t} WHERE -D >= I AND V LIKE 'v1%' AND I <> -N",
    "SELECT COUNT(*) FROM {t} WHERE V LIKE 'v1%' AND V NOT LIKE '%5'",
    "SELECT COUNT(*), MIN(K) FROM {t} WHERE V LIKE '%1 ' OR V LIKE 'é%'",
    "SELECT COUNT(*) FROM {t} WHERE V = 'v3' OR V = 'v6 '",
    "SELECT COUNT(*) FROM {t} WHERE V < 'v2' AND V >= 'v10' AND V <> 'v12'",
    "SELECT COUNT(*) FROM {t} WHERE V > 'é' OR V <= 'v'",
    "SELECT COUNT(*) FROM {t} WHERE V IN ('v1', 'v2 ', 'é3') AND V NOT IN ('v9', CAST(NULL AS VARCHAR(3)))",
    "SELECT COUNT(*) FROM {t} WHERE V NOT IN ('v1', 'v2') AND V IS NOT NULL OR V IS NULL AND K < 100",
    // Not V itself, of which 'v0' and 'v0 ' are one group, nor MIN(V):
    // which of equal strings either gives is the first row's, and a table
    // of the column store gives its rows in another order by batch.
    "SELECT COUNT(*), MIN(K), MAX(K) FROM {t} GROUP BY V",
    "SELECT COUNT(*) FROM (SELECT DISTINCT V FROM {t} WHERE K > 50) AS X",
    "SELECT D, SUM(CASE WHEN I > 2 THEN I * 10 ELSE 0 END), COUNT(*) FROM {t} WHERE K < 2000 GROUP BY D",
];

/// Verifies: [DESIGN] RULES.md D31 [X3.135-1986 §5.8 GR1, §5.11 GR4,
/// GR5, §5.13, §5.14, §5.15]: exact numbers judged and fed to set
/// functions as integers, and character strings as text, give what they
/// give row by row (EXECUTOR.BATCHES OFF), and what a table of the row
/// store gives: numbers compared with constants of more and fewer digits
/// after the point than their scale, of either sign, one that no value
/// equals, by every comparison; IN and NOT IN lists, one holding a null,
/// one an approximate number; numbers near the limits of 18 digits; set
/// functions grouped and not, of expressions, negations, differences and
/// products of scales up to 18, one with a quotient evaluated of values;
/// a running sum, or a product, past 18 digits raising either way; filters
/// that can raise of comparisons of expressions, one of a quotient, one
/// raising; filters that cannot, of columns and negations compared, the
/// changed rows' values judged a row at a time; of a constant, of an approximate number, of a CASE a row at a
/// time; groups past the work area; strings, some not ASCII, some with a
/// trailing space, LIKE patterns, compared padded, IN and NOT IN, grouped,
/// distinct, counted; nulls among them all; rows changed and deleted after
/// they were frozen.
#[test]
fn numbers_and_strings_as_vectors_answer_as_row_by_row() {
    let db = Database::new();
    let a = both(&db);
    for query in QUERIES {
        let column = query.replace("{t}", "C");
        let (batched, row) = both_ways(&a, &column);
        assert_eq!(batched, row, "{column}");
        assert_eq!(batched, sorted(&a, &query.replace("{t}", "R")), "{column}, as the row store");
    }
    assert_eq!(sorted(&a, "SELECT COUNT(*) FROM C WHERE D = 1.515"), vec!["0"], "no value of scale 2 is 1.515");
    // Groups of integers past the work area, written and made after.
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    let spilled = "SELECT K, N, SUM(D), MAX(I) FROM C GROUP BY K, N";
    let (batched, row) = both_ways(&a, spilled);
    assert_eq!(batched, row, "{spilled}");
    assert_eq!(batched, sorted(&a, &spilled.replace(" C ", " R ")), "{spilled}, as the row store");
    exec(&a, "SET MEMORY.WORK_BYTES = DEFAULT").unwrap();
    // [X3.135-1986 §5.8 GR4e] a running sum past 18 digits raises, of
    // integers as of decimals, though the sum of all is within them.
    let raising = [
        "SELECT COUNT(*) FROM {t} WHERE N * 1000 > 0 AND I = 1",
        "SELECT SUM(N) FROM {t}",
        "SELECT SUM(N) FROM {t} GROUP BY I",
        "SELECT SUM(N * 1000) FROM {t}",
        "SELECT I, MAX(N * 10) FROM {t} GROUP BY I",
    ];
    for (table, query) in ["R", "C"].iter().flat_map(|t| raising.iter().map(move |q| (*t, *q))) {
        let sum = query.replace("{t}", table);
        assert_eq!(rows(&a, &sum).err(), Some(Code::Data), "{sum}");
        exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
        assert_eq!(rows(&a, &sum).err(), Some(Code::Data), "{sum}, row by row");
        exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    }
    assert_ne!(sorted(&a, "SELECT COUNT(*) FROM C WHERE D = 1.510"), vec!["0"], "1.51 is 1.510");
}
