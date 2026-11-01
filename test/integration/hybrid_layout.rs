//! \[EXTENSION\] The column store's hybrid layout (RULES.md D31,
//! COLUMNSTORE.LAYOUT = 'PAX'): each page of a frozen group a range of its
//! rows, a chunk of each column on it. A table so laid out answers as a
//! table of the row store does, read whole, in part, by key, in order and
//! filtered; changes its layout either way, its groups written again;
//! and is recovered as it was.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;

use crate::common::{Scratch, exec, os, rows, sorted};

/// Rows of each table.
const ROWS: i64 = 6000;

/// Queries of a table `{t}`: whole and of some columns, by key, filtered,
/// grouped, and in order.
const QUERIES: [&str; 9] = [
    "SELECT * FROM {t}",
    "SELECT K, N FROM {t} WHERE G = 3",
    "SELECT * FROM {t} WHERE K = 4321",
    "SELECT S, K FROM {t} WHERE K BETWEEN 2000 AND 2100",
    "SELECT G, COUNT(*), SUM(N), MAX(S) FROM {t} GROUP BY G",
    "SELECT COUNT(*) FROM {t} WHERE S IS NULL OR N > 400",
    "SELECT N, K FROM {t} WHERE N >= 590 ORDER BY N",
    "SELECT K, W FROM {t} WHERE K < 20",
    "SELECT COUNT(W), MIN(W) FROM {t}",
];

/// Schema A of `db`: R, of the row store, and P, of the column store in
/// groups of 2048 laid out as PAX, of `ROWS` rows each, K a key, G one of
/// 7, S a text null for every 11th, N in the rows' order, W a long text
/// for every 50th; P's rows frozen.
fn both(db: &Database) -> Session {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let columns = "K INTEGER PRIMARY KEY, G INTEGER, S VARCHAR(20), N INTEGER, W VARCHAR(900)";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE R ({columns})
           CREATE TABLE P ({columns}) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048, COLUMNSTORE.LAYOUT = 'PAX')"
    ))
    .unwrap();
    let a = db.session("A");
    for table in ["R", "P"] {
        for first in (0..ROWS).step_by(500) {
            let values: Vec<String> = (first..first + 500)
                .map(|k| {
                    let s = if k % 11 == 0 { "NULL".to_owned() } else { format!("'s{}'", k % 97) };
                    let w =
                        if k % 50 == 0 { format!("'{}'", "w".repeat(usize::try_from(800 + k % 7).unwrap())) } else { "NULL".to_owned() };
                    format!("({k}, {}, {s}, {}, {w})", k % 7, k.checked_div(10).unwrap())
                })
                .collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    a
}

/// Checks that each query reads of P what it reads of R.
fn alike(s: &Session, when: &str) {
    for q in QUERIES {
        assert_eq!(sorted(s, &q.replace("{t}", "P")), sorted(s, &q.replace("{t}", "R")), "{when}: {q}");
    }
    let ordered = QUERIES[6];
    assert_eq!(rows(s, &ordered.replace("{t}", "P")), rows(s, &ordered.replace("{t}", "R")), "{when}: in order");
}

/// Verifies: [EXTENSION] RULES.md D31, D41, COLUMNSTORE.LAYOUT: a table of
/// the column store laid out as PAX answers as a table of the row store,
/// frozen, with rows changed, deleted and added, and its groups written
/// again; once its layout is COLUMNS, and PAX again, its groups are written
/// so and it answers alike; the option is shown as set, and a layout of
/// neither name is refused.
#[test]
fn a_hybrid_table_answers_as_a_row_table() {
    let db = Database::new();
    let a = both(&db);
    alike(&a, "frozen");
    for table in ["R", "P"] {
        a.run(&format!("UPDATE {table} SET N = -N WHERE K < 700")).unwrap();
        a.run(&format!("DELETE FROM {table} WHERE K BETWEEN 3000 AND 3100")).unwrap();
        a.run(&format!("INSERT INTO {table} VALUES (90001, 1, 'new', 1, NULL)")).unwrap();
    }
    a.commit().unwrap();
    alike(&a, "changed");
    db.maintain().unwrap();
    alike(&a, "written again");
    let layout = "SELECT OPTION_VALUE FROM INFORMATION_SCHEMA.TABLE_OPTIONS WHERE TABLE_NAME = 'P' AND OPTION_NAME = 'COLUMNSTORE.LAYOUT'";
    assert_eq!(rows(&a, layout), Ok(vec!["PAX".to_owned()]));
    for to in ["COLUMNS", "PAX"] {
        a.run(&format!("ALTER TABLE P SET (COLUMNSTORE.LAYOUT = '{to}')")).unwrap();
        a.commit().unwrap();
        alike(&a, &format!("{to}, groups not yet written again"));
        db.maintain().unwrap();
        alike(&a, &format!("{to}, groups written again"));
    }
    assert_eq!(exec(&a, "ALTER TABLE P SET (COLUMNSTORE.LAYOUT = 'ROWS')").err(), Some(Code::SyntaxRule));
}

/// The database in `dir`.
fn open(dir: &Path) -> Database {
    let db = Database::new();
    db.open_files(&os(), dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D25, D31: a table laid out as PAX in a
/// database kept in a directory answers alike once the database is opened
/// again, whether it stopped without closing or closed.
#[test]
fn a_hybrid_table_is_recovered() {
    let scratch = Scratch::new("hybrid-layout");
    let dir = scratch.path("db");
    let db = open(&dir);
    let a = both(&db);
    alike(&a, "frozen");
    drop((a, db));
    for round in 0..2 {
        let db = open(&dir);
        db.set_job_policy(JobPolicy::Manual).unwrap();
        let a = db.session("A");
        alike(&a, &format!("round {round}"));
        if round == 0 {
            drop(a);
            db.close().unwrap();
        }
    }
}
