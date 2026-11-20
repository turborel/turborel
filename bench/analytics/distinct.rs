//! How fast DISTINCT set functions run (RULES.md D340-D342): COUNT with
//! DISTINCT of 10, 977 and 200,000 values of F and FC, of 997 values in
//! each of 10 groups, of two arguments in one query, and with functions
//! without DISTINCT beside it. Run alone with `ONLY_VAR` set to
//! `distinct`.

use crate::harness::{Bench, Case};
use crate::{ROWS, fetched};
use turborel::db::Session;

/// Measures the DISTINCT set functions of table `table`, their cases'
/// names ending in `label`.
pub fn workloads(bench: &Bench, s: &Session, (table, label): (&str, &str)) {
    for (name, sql) in [
        ("COUNT(DISTINCT) of 10 values", "SELECT COUNT(DISTINCT G) FROM {t}"),
        ("COUNT(DISTINCT) of 977 strings", "SELECT COUNT(DISTINCT S) FROM {t}"),
        ("COUNT(DISTINCT) of 200,000 values", "SELECT COUNT(DISTINCT C) FROM {t}"),
        ("COUNT(DISTINCT) of 997 values in each of 10 groups", "SELECT G, COUNT(DISTINCT I) FROM {t} GROUP BY G"),
        ("COUNT(DISTINCT) of two arguments", "SELECT COUNT(DISTINCT A), COUNT(DISTINCT D) FROM {t}"),
        ("COUNT(DISTINCT), SUM and COUNT(*) in 10 groups", "SELECT G, COUNT(DISTINCT C), SUM(E), COUNT(*) FROM {t} GROUP BY G"),
    ] {
        let sql = sql.replace("{t}", table);
        bench.measure(Case::new(format!("{name}{label}"), "rows"), || {
            fetched(s, &sql);
            ROWS
        });
    }
}
