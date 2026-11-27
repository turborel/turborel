//! Window set functions over a column store's frozen integers with nulls
//! [ISO/IEC 9075:1992 §6.5 GR1b]. A chunk's encoding leaves a null's place
//! holding a number of no meaning (a frame's least, the run's, a
//! dictionary's first entry). SUM, AVG and COUNT over a frame leave each
//! null out, as the same rows of the row store give them (RULES.md D1289).

use turborel::db::{Database, Session};

use crate::common::rows;

/// The rows of each table: `I` in order, `F` numbers far from zero of many
/// values, `R` runs of one number, `D` four numbers far apart, each with
/// nulls among them, the first row's among them.
fn values(first: i64) -> String {
    let far = [1_000_000_007_i64, 5, 9_000_000_000_000, -77];
    let each = (first..first + 2048).map(|k| {
        let null_or = |every: i64, v: i64| if k % every == 0 { "NULL".to_string() } else { v.to_string() };
        let framed = null_or(7, 1000 + (k * 7919) % 5000);
        let run = null_or(11, 500 + k.checked_div(100).unwrap());
        let entry = null_or(5, far[usize::try_from(k * 31 % 4).unwrap()]);
        format!("({k}, {framed}, {run}, {entry})")
    });
    each.collect::<Vec<_>>().join(", ")
}

/// `PLAIN`, the rows of the row store, and `FROZEN`, the same of the column
/// store, frozen.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE PLAIN (I INTEGER, F INTEGER, R INTEGER, D NUMERIC(18, 0))
           CREATE TABLE FROZEN (I INTEGER, F INTEGER, R INTEGER, D NUMERIC(18, 0))
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["PLAIN", "FROZEN"] {
        for first in [0, 2048] {
            a.run(&format!("INSERT INTO {table} VALUES {}", values(first))).unwrap();
        }
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// Verifies: [ISO/IEC 9075:1992 §6.5 GR1b]
/// nulls are left out of SUM, AVG and COUNT over a window's frame, of a
/// column store's frozen column in each encoding that leaves a null's
/// place holding a number, a frame of reference, runs and a dictionary:
/// over the whole partition, which asks for no order, and over frames of
/// rows in a key's order, each gives what the row store's same rows give
/// (RULES.md D1289).
#[test]
fn window_sums_leave_out_the_nulls_of_frozen_columns() {
    let (_db, a) = tables();
    let shown = "SELECT COLUMN_NAME, ENCODING FROM INFORMATION_SCHEMA.COLUMN_STORAGE WHERE TABLE_NAME = 'FROZEN' \
                 AND COLUMN_NAME <> 'I' GROUP BY COLUMN_NAME, ENCODING ORDER BY COLUMN_NAME";
    assert_eq!(rows(&a, shown).unwrap(), ["D,DICTIONARY", "F,FRAME", "R,RUNS"], "each encoding is read");
    let frames = ["()", "(ORDER BY I ROWS BETWEEN 5 PRECEDING AND CURRENT ROW)", "(PARTITION BY R ORDER BY I)"];
    for column in ["F", "R", "D"] {
        for frame in frames {
            let of = |table: &str| {
                let sql = format!(
                    "SELECT I, SUM({column}) OVER {frame}, AVG({column}) OVER {frame}, COUNT({column}) OVER {frame} FROM {table} ORDER BY I"
                );
                rows(&a, &sql).unwrap()
            };
            let (frozen, plain) = (of("FROZEN"), of("PLAIN"));
            assert_eq!(frozen.len(), 4096, "{column} {frame}");
            assert_eq!(frozen, plain, "{column} OVER {frame}");
        }
    }
}
