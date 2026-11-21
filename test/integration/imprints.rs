//! \[DESIGN\] Column imprints of the column store's frozen groups (RULES.md
//! D511, COLUMNSTORE.IMPRINTS): a table that keeps them, and one that does
//! not, answer as a table of the row store does, of conditions of every
//! kind imprints narrow, of columns in order, skewed and scattered, with
//! nulls; with rows changed; and once the option is changed.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;

use crate::common::{exec, rows};

/// Rows of each table.
const ROWS: i64 = 9000;

/// Queries of a table `{t}`: comparisons, ranges, IN, NOT IN, a LIKE
/// prefix, IS NULL, conditions of two columns and of several conjuncts.
const QUERIES: [&str; 12] = [
    "SELECT K FROM {t} WHERE N = 4321",
    "SELECT COUNT(*) FROM {t} WHERE N BETWEEN 2000 AND 2100",
    "SELECT K, S FROM {t} WHERE V = 7777",
    "SELECT COUNT(*) FROM {t} WHERE V IN (3, 70, 8880)",
    "SELECT COUNT(*) FROM {t} WHERE V NOT IN (42, 43)",
    "SELECT K FROM {t} WHERE S LIKE 's00012%'",
    "SELECT COUNT(*) FROM {t} WHERE S >= 's00500' AND S < 's00510'",
    "SELECT COUNT(*) FROM {t} WHERE V IS NULL",
    "SELECT K FROM {t} WHERE V = N",
    "SELECT K FROM {t} WHERE N < 300 AND V > 100 AND S LIKE 's000%'",
    "SELECT D, COUNT(*) FROM {t} WHERE D BETWEEN DATE '2013-02-01' AND DATE '2013-02-03' GROUP BY D",
    "SELECT COUNT(*) FROM {t} WHERE K > 8990 OR K < 5",
];

/// Schema A of `db`: R of the row store, P of the column store keeping
/// imprints and Q not, each of `ROWS` rows, K a key, N in the rows' order,
/// V 42 in nine rows of ten and else the key, null for every 13th, S a
/// text of a scattered number, D a date in the rows' order; P's and Q's
/// rows frozen.
fn tables(db: &Database) -> Session {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let columns = "K INTEGER PRIMARY KEY, N INTEGER, V INTEGER, S VARCHAR(20), D DATE";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE R ({columns})
           CREATE TABLE P ({columns}) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 4096, COLUMNSTORE.IMPRINTS = 'ON')
           CREATE TABLE Q ({columns}) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 4096, COLUMNSTORE.IMPRINTS = 'OFF')"
    ))
    .unwrap();
    let a = db.session("A");
    for table in ["R", "P", "Q"] {
        for first in (0..ROWS).step_by(1000) {
            let values: Vec<String> = (first..first + 1000)
                .map(|k| {
                    let v = if k % 13 == 0 {
                        "NULL".to_owned()
                    } else if k % 10 == 0 {
                        k.to_string()
                    } else {
                        "42".to_owned()
                    };
                    format!(
                        "({k}, {k}, {v}, 's{:05}', DATE '2013-{:02}-{:02}')",
                        k * 7919 % ROWS,
                        1 + k.checked_div(2800).unwrap(),
                        1 + k.checked_div(100).unwrap() % 28
                    )
                })
                .collect();
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
        assert_eq!(of("P"), of_rows, "{when}, imprints: {q}");
        assert_eq!(of("Q"), of_rows, "{when}, none: {q}");
    }
}

/// Verifies: [DESIGN] RULES.md D511, COLUMNSTORE.IMPRINTS: tables of the
/// column store with imprints and without answer each query as a table of
/// the row store, frozen, with rows changed, and their groups written
/// again; the option is shown as set, changed either way, and a value of
/// neither name is refused.
#[test]
fn tables_with_imprints_answer_as_row_tables() {
    let db = Database::new();
    let a = tables(&db);
    alike(&a, "frozen");
    for table in ["R", "P", "Q"] {
        a.run(&format!("UPDATE {table} SET V = 4321, S = 's00123' WHERE K BETWEEN 600 AND 610")).unwrap();
        a.run(&format!("DELETE FROM {table} WHERE K BETWEEN 3000 AND 3100")).unwrap();
        a.run(&format!("INSERT INTO {table} VALUES (90001, 4321, 7777, 's00505', DATE '2013-02-02')")).unwrap();
    }
    a.commit().unwrap();
    alike(&a, "changed");
    db.maintain().unwrap();
    alike(&a, "written again");
    let shown = |t: &str| {
        rows(
            &a,
            &format!(
                "SELECT OPTION_VALUE FROM INFORMATION_SCHEMA.TABLE_OPTIONS WHERE TABLE_NAME = '{t}' AND OPTION_NAME = 'COLUMNSTORE.IMPRINTS'"
            ),
        )
    };
    assert_eq!((shown("P"), shown("Q")), (Ok(vec!["ON".to_owned()]), Ok(vec!["OFF".to_owned()])));
    for (base, (p, q)) in [(9100, ("OFF", "ON")), (9200, ("ON", "OFF"))] {
        a.run(&format!("ALTER TABLE P SET (COLUMNSTORE.IMPRINTS = '{p}')")).unwrap();
        a.run(&format!("ALTER TABLE Q SET (COLUMNSTORE.IMPRINTS = '{q}')")).unwrap();
        a.commit().unwrap();
        for k in [base, base + 1] {
            for table in ["R", "P", "Q"] {
                a.run(&format!("INSERT INTO {table} VALUES ({k}, {k}, 7777, 's00012x', DATE '2013-03-01')")).unwrap();
            }
            a.commit().unwrap();
            db.maintain().unwrap();
        }
        alike(&a, &format!("P {p}, Q {q}"));
    }
    assert_eq!(exec(&a, "ALTER TABLE P SET (COLUMNSTORE.IMPRINTS = 'MAYBE')").err(), Some(Code::SyntaxRule));
}
