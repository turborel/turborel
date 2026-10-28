//! How fast analytic queries run on the row store (RULES.md D31), the
//! baseline of a columnar layout: a table of 16 columns and 200,000 rows
//! counted, summed by one column and by twelve, filtered to 1% by a column
//! in the order the rows were written and by one scattered, grouped, and
//! two of its columns read whole; and one row fetched by its key, the
//! transactional work beside them. Rates are of the table's rows read,
//! but the key's, of queries. Run with `cargo xtask bench`, on the harness
//! (RULES.md R-BENCH-1).

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;

use harness::{Bench, Case, scatter};
use turborel::db::{Database, Session};

/// Rows in the table F.
const ROWS: u64 = 200_000;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 500;
/// Keys fetched in each run of the lookups.
const LOOKUPS: u64 = 1_000;

fn main() {
    let bench = Bench::new("analytics");
    let db = database();
    let s = db.session("B");
    measure(&bench, &s, ("F", ""));
    measure(&bench, &s, ("FC", " (COLUMNS)"));
}

/// Measures each workload on table `table`, its cases' names ending in
/// `label`.
fn measure(bench: &Bench, s: &Session, (table, label): (&str, &str)) {
    let scan = |sql: &str| {
        let sql = sql.replace("{t}", table);
        move || {
            fetched(s, &sql);
            ROWS
        }
    };
    let case = |name: &str, unit: &'static str| Case::new(format!("{name}{label}"), unit);
    bench.measure(case("COUNT(*) of 200,000 rows of 16 columns", "rows"), scan("SELECT COUNT(*) FROM {t}"));
    bench.measure(case("SUM of 1 of 16 columns", "rows"), scan("SELECT SUM(A) FROM {t}"));
    bench.measure(
        case("SUMs of 12 of 16 columns", "rows"),
        scan("SELECT SUM(A), SUM(B), SUM(C), SUM(D), SUM(E), SUM(H), SUM(I), SUM(J), SUM(L), SUM(M), SUM(N), SUM(O) FROM {t}"),
    );
    bench.measure(
        case("COUNT of 1% by a column in the rows' order", "rows"),
        scan("SELECT COUNT(*) FROM {t} WHERE B BETWEEN 100000 AND 101999"),
    );
    bench.measure(case("COUNT of 1% by a scattered column", "rows"), scan("SELECT COUNT(*) FROM {t} WHERE C < 2000"));
    bench.measure(case("GROUP BY 10 groups, COUNT, SUM and AVG", "rows"), scan("SELECT G, COUNT(*), SUM(A), AVG(E) FROM {t} GROUP BY G"));
    bench.measure(case("fetch 2 of 16 columns of every row", "rows"), scan("SELECT A, B FROM {t}"));
    bench.measure(case("fetch one row of 16 columns by its key", "queries"), || {
        for n in 0..LOOKUPS {
            fetched(s, &format!("SELECT * FROM {table} WHERE K = {}", scatter(n, ROWS)));
        }
        LOOKUPS
    });
}

/// The rows of `sql`, each fetched, counted.
fn fetched(s: &Session, sql: &str) -> u64 {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut rows = 0;
    while cursor.fetch().unwrap().is_some() {
        rows += 1;
    }
    s.commit().unwrap();
    rows
}

/// Schema B with table F of `ROWS` rows: K its key; G one of 10 groups; B
/// in the order the rows are written, C scattered; the others numbers of
/// a few distributions, E exact with a fraction, and S text.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE F (K INTEGER PRIMARY KEY, G INTEGER NOT NULL, A INTEGER, B INTEGER, C INTEGER,
             D INTEGER, E DECIMAL(9,2), H INTEGER, I INTEGER, J INTEGER, L INTEGER, M INTEGER,
             N INTEGER, O INTEGER, P INTEGER, S CHAR(16))
           CREATE TABLE FC (K INTEGER PRIMARY KEY, G INTEGER NOT NULL, A INTEGER, B INTEGER, C INTEGER,
             D INTEGER, E DECIMAL(9,2), H INTEGER, I INTEGER, J INTEGER, L INTEGER, M INTEGER,
             N INTEGER, O INTEGER, P INTEGER, S CHAR(16)) WITH (LAYOUT = 'COLUMNS')",
    )
    .unwrap();
    let s = db.session("B");
    for first in (0..ROWS).step_by(PER_INSERT as usize) {
        let rows: Vec<String> = (first..first + PER_INSERT)
            .map(|k| {
                let c = scatter(k, ROWS);
                format!(
                    "({k}, {}, {}, {k}, {c}, {}, {}.{:02}, {}, {}, {}, {}, {}, {}, {}, {}, 's {}')",
                    k % 10,
                    k % 1_000,
                    k % 7,
                    k % 5_000,
                    k % 100,
                    k / 3,
                    k % 13,
                    c % 997,
                    k * 2,
                    k % 50,
                    c / 7,
                    k % 3,
                    k + 1,
                    k % 977
                )
            })
            .collect();
        for table in ["F", "FC"] {
            s.run(&format!("INSERT INTO {table} VALUES {}", rows.join(", "))).unwrap();
        }
    }
    s.commit().unwrap();
    db
}
