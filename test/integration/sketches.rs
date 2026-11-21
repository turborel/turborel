//! \[DESIGN\] Column sketches of the column store's frozen groups (RULES.md
//! D525, COLUMNSTORE.SKETCH): a table that keeps them of its texts, and one
//! that does not, answer as a table of the row store does, of equalities,
//! ranges, LIKE prefixes, IN and NOT IN of texts of many values, one value
//! frequent among them, some ending in spaces, nulls among them, and of a
//! column of another collation; with rows changed; and once the option is
//! changed.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;

use crate::common::{exec, rows};

/// Rows of each table.
const ROWS: i64 = 12_000;

/// Queries of a table `{t}`.
const QUERIES: [&str; 14] = [
    "SELECT K FROM {t} WHERE U = 'http://site42.example/page4242'",
    "SELECT COUNT(*) FROM {t} WHERE U = 'http://often.example/'",
    "SELECT COUNT(*) FROM {t} WHERE U = 'http://often.example/  '",
    "SELECT COUNT(*) FROM {t} WHERE U LIKE 'http://site7.example/%'",
    "SELECT COUNT(*) FROM {t} WHERE U LIKE 'http://site1%'",
    "SELECT COUNT(*) FROM {t} WHERE U >= 'http://site3' AND U < 'http://site4'",
    "SELECT COUNT(*) FROM {t} WHERE U > 'http://site9'",
    "SELECT COUNT(*) FROM {t} WHERE U <= 'http://often.example/'",
    "SELECT COUNT(*) FROM {t} WHERE U IN ('http://site5.example/page5', 'http://often.example/', 'none')",
    "SELECT COUNT(*) FROM {t} WHERE U NOT IN ('http://often.example/', 'http://site6.example/page6')",
    "SELECT COUNT(*) FROM {t} WHERE U IS NULL OR U < 'http://site2'",
    "SELECT COUNT(*) FROM {t} WHERE U <> 'http://often.example/' AND K < 6000",
    "SELECT COUNT(*) FROM {t} WHERE C = 'HTTP://OFTEN.EXAMPLE/'",
    "SELECT COUNT(*) FROM {t} WHERE C >= 'HTTP://SITE3' AND C < 'http://site4'",
];

/// The values of row `k`: K the key, U a web address of many, every fifth
/// one address, every 23rd null, every 13th ending in spaces, and C the
/// same of a collation ignoring case.
fn row(k: i64) -> String {
    let u = if k % 23 == 0 {
        "NULL".to_owned()
    } else if k % 5 == 0 {
        "'http://often.example/'".to_owned()
    } else {
        let pad = if k % 13 == 0 { "  " } else { "" };
        format!("'http://site{}.example/page{}{pad}'", k * 7919 % 500, k * 104_729 % 100_000)
    };
    format!("({k}, {u}, {u})")
}

/// Schema A of `db`: R of the row store, P of the column store keeping
/// sketches of U and C and Q not, each of `ROWS` rows; P's and Q's rows
/// frozen.
fn tables(db: &Database) -> Session {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let columns = "K INTEGER PRIMARY KEY, U VARCHAR(60), C VARCHAR(60) COLLATE CASE_INSENSITIVE";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE R ({columns})
           CREATE TABLE P ({columns}) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 4096, COLUMNSTORE.SKETCH = 'U, C')
           CREATE TABLE Q ({columns}) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 4096)"
    ))
    .unwrap();
    let a = db.session("A");
    for table in ["R", "P", "Q"] {
        for first in (0..ROWS).step_by(1000) {
            let values: Vec<String> = (first..first + 1000).map(row).collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    a
}

/// Checks that each query reads of P and Q what it reads of R.
fn alike(s: &Session, when: &str) {
    for q in QUERIES {
        let of = |t: &str| {
            let mut found = rows(s, &q.replace("{t}", t)).unwrap_or_else(|e| panic!("{when}, {t}: {q}: {e:?}"));
            found.sort();
            found
        };
        let of_rows = of("R");
        assert_eq!(of("P"), of_rows, "{when}, sketches: {q}");
        assert_eq!(of("Q"), of_rows, "{when}, none: {q}");
    }
}

/// Verifies: [DESIGN] RULES.md D525, COLUMNSTORE.SKETCH: tables of the
/// column store with sketches and without answer each query as a table of
/// the row store, frozen, with rows changed, and their groups written
/// again; the option is shown as set, its groups written again once it is
/// changed, and a column not the table's refused.
#[test]
fn tables_with_sketches_answer_as_row_tables() {
    let db = Database::new();
    let a = tables(&db);
    alike(&a, "frozen");
    for table in ["R", "P", "Q"] {
        a.run(&format!("UPDATE {table} SET U = 'http://often.example/', C = U WHERE K BETWEEN 600 AND 640")).unwrap();
        a.run(&format!("DELETE FROM {table} WHERE K BETWEEN 3000 AND 3100")).unwrap();
        a.run(&format!("INSERT INTO {table} VALUES (90001, 'http://site42.example/page4242', 'x')")).unwrap();
    }
    a.commit().unwrap();
    alike(&a, "changed");
    db.maintain().unwrap();
    alike(&a, "written again");
    let shown = |t: &str| {
        rows(
            &a,
            &format!(
                "SELECT OPTION_VALUE FROM INFORMATION_SCHEMA.TABLE_OPTIONS WHERE TABLE_NAME = '{t}' AND OPTION_NAME = 'COLUMNSTORE.SKETCH'"
            ),
        )
    };
    assert_eq!(shown("P"), Ok(vec!["U, C".to_owned()]));
    a.run("ALTER TABLE P SET (COLUMNSTORE.SKETCH = DEFAULT)").unwrap();
    a.run("ALTER TABLE Q SET (COLUMNSTORE.SKETCH = 'U')").unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    alike(&a, "the option changed");
    assert_eq!(exec(&a, "ALTER TABLE P SET (COLUMNSTORE.SKETCH = 'NOPE')").err(), Some(Code::SyntaxRule));
}
