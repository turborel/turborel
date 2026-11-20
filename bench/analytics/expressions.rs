//! How fast expressions evaluated a vector at a time run (RULES.md D362,
//! D363): set functions of arithmetic of decimals and integers, grouped
//! and filtered as a pricing summary report is, a revenue sum filtered by
//! ranges, filters comparing expressions of several columns, an OR of
//! them, CASE, ABS and CHAR_LENGTH, of F and FC. Run alone with `ONLY_VAR`
//! set to `expressions`.

use crate::harness::{Bench, Case};
use crate::{ROWS, fetched};
use turborel::db::Session;

/// Measures the expressions of table `table`, their cases' names ending
/// in `label`.
pub fn workloads(bench: &Bench, s: &Session, (table, label): (&str, &str)) {
    for (name, sql) in [
        (
            "pricing summary of decimal arithmetic, 10 groups",
            "SELECT G, SUM(E), SUM(E * (1 - D * 0.01)), SUM(E * (1 - D * 0.01) * (1 + H * 0.001)), AVG(E), COUNT(*) \
             FROM {t} WHERE A < 900 GROUP BY G",
        ),
        ("revenue of ranges", "SELECT SUM(E * D) FROM {t} WHERE A >= 100 AND A < 600 AND D BETWEEN 2 AND 4 AND H < 50"),
        ("sum of integer arithmetic", "SELECT SUM(A * 3 + B - C * 2) FROM {t}"),
        ("filter of expressions of columns", "SELECT COUNT(*) FROM {t} WHERE A * 2 + D > C - B"),
        ("OR of expressions", "SELECT COUNT(*) FROM {t} WHERE A + H < 100 OR C * 2 > 390000"),
        ("CASE of three branches", "SELECT SUM(CASE WHEN D < 3 THEN E WHEN D < 5 THEN E * 2 ELSE 0 END) FROM {t}"),
        ("ABS of a difference", "SELECT SUM(ABS(A - 500)) FROM {t}"),
        ("filter of CHAR_LENGTH", "SELECT COUNT(*) FROM {t} WHERE CHAR_LENGTH(S) > 8"),
    ] {
        let sql = sql.replace("{t}", table);
        bench.measure(Case::new(format!("{name}{label}"), "rows"), || {
            fetched(s, &sql);
            ROWS
        });
    }
}
