//! How fast ORDER BY and top-N run (RULES.md D364, D365): of T, a row-store
//! table of 200,000 rows, and of TC, a column-store one of the same rows,
//! sorted by an integer column, by a decimal one, by a string of a long
//! shared start, by it under a collation, by three keys, one of nulls, by
//! the first 10 and the first 10,000 rows by an integer column, and by
//! every column past a work area of 1 MiB, so spilled. Each is run on the
//! statement's thread alone and on the executor's workers. Run alone with
//! `ONLY_VAR` set to `sorts` (RULES.md R-BENCH-1).

use super::harness::{Bench, Case, scatter};
use turborel::db::{Database, Session};

/// Rows in the tables T and TC.
const ROWS: u64 = 200_000;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 500;

/// Measures each sort of T and of TC, serial and in parallel, each on a
/// session of its own, so each runs under the default work area but the
/// case past 1 MiB.
pub fn workloads(bench: &Bench) {
    let db = database();
    for (degree, threads) in [("1", ", serial"), ("0", "")] {
        for (table, label) in [("T", ""), ("TC", " (columnstore)")] {
            let s = db.session("B");
            s.run(&format!("SET EXECUTOR.DEGREE = {degree}")).unwrap();
            measured(bench, &s, table, &format!("{threads}{label}"));
        }
    }
}

/// Measures the sorts of `table`, their cases' names ending in `label`.
fn measured(bench: &Bench, s: &Session, table: &str, label: &str) {
    for (name, sql) in [
        ("ORDER BY an integer column, 2 columns", "SELECT C, K FROM {t} ORDER BY C"),
        ("ORDER BY a decimal column, 2 columns", "SELECT E, K FROM {t} ORDER BY E"),
        ("ORDER BY a string of a long shared start", "SELECT S, K FROM {t} ORDER BY S"),
        ("ORDER BY a string under a collation", "SELECT S, K FROM {t} ORDER BY S COLLATE UNICODE"),
        ("ORDER BY three keys, nulls first", "SELECT G, N, C FROM {t} ORDER BY G DESC, N NULLS FIRST, C"),
        ("first 10 by an integer column", "SELECT C, K FROM {t} ORDER BY C FETCH FIRST 10 ROWS ONLY"),
        ("first 10,000 by an integer column", "SELECT C, K FROM {t} ORDER BY C FETCH FIRST 10000 ROWS ONLY"),
        ("first 10 of every column by the key", "SELECT * FROM {t} ORDER BY K DESC FETCH FIRST 10 ROWS ONLY"),
        ("first 10 of every column by an integer column", "SELECT * FROM {t} ORDER BY C FETCH FIRST 10 ROWS ONLY"),
        ("ORDER BY an integer column, others projected", "SELECT K, S FROM {t} ORDER BY C"),
    ] {
        let sql = sql.replace("{t}", table);
        bench.measure(Case::new(format!("{name}{label}"), "rows"), || {
            super::fetched(s, &sql);
            ROWS
        });
    }
    s.run("SET MEMORY.WORK_BYTES = 1048576").unwrap();
    let sql = format!("SELECT * FROM {table} ORDER BY C");
    bench.measure(Case::new(format!("ORDER BY of 10 columns past 1 MiB{label}"), "rows"), || {
        super::fetched(s, &sql);
        ROWS
    });
}

/// Schema B with T, in the row store, and TC, in the column store, each
/// of `ROWS` rows: K its key, C scattered, E a decimal of few values, S a
/// string whose first 12 characters every row shares, G one of 10
/// groups, N one of 100 or null each 7th row, and four more columns.
fn database() -> Database {
    let db = Database::new();
    let columns = "K INTEGER NOT NULL, C INTEGER, E DECIMAL(12,2), S VARCHAR(40), G INTEGER, N INTEGER, \
                   P INTEGER, Q INTEGER, R DOUBLE PRECISION, V VARCHAR(12)";
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION B CREATE TABLE T ({columns}) CREATE TABLE TC ({columns}) USING columnstore")).unwrap();
    let s = db.session("B");
    for first in (0..ROWS).step_by(PER_INSERT as usize) {
        let rows: Vec<String> = (first..first + PER_INSERT)
            .map(|k| {
                let c = scatter(k, ROWS);
                let n = if k % 7 == 0 { "NULL".to_owned() } else { (c % 100).to_string() };
                format!(
                    "({k}, {c}, {}.{:02}, 'customer#0000{}', {}, {n}, {}, {}, {}.5, 'v{}')",
                    c % 5_000,
                    k % 100,
                    c % 50_000,
                    k % 10,
                    k % 1_000,
                    c / 3,
                    k % 777,
                    k % 97
                )
            })
            .collect();
        for table in ["T", "TC"] {
            s.run(&format!("INSERT INTO {table} VALUES {}", rows.join(", "))).unwrap();
        }
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}
