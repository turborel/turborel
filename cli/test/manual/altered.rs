//! The examples of ALTER TABLE's column nullability, data type and table
//! RENAME TO (`doc/sql/statements/alter-table.md`), run as printed.

use turborel::db::Database;

use super::{Client, Example, examples};

/// Verifies: [ISO/IEC 9075-2:2023 §11.15, §11.16, §11.19; EXTENSION]
/// RULES.md R-DOC-8, D399 to D402: the examples of SET NOT NULL, SET DATA
/// TYPE, their refusals and RENAME TO run as printed, as ANN: those of
/// table STOCK.
#[test]
fn the_altered_column_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let page = "sql/statements/alter-table.md";
    let stock: Vec<Example> =
        examples(page).into_iter().filter(|e| matches!(e, Example::Sql { sql, .. } if sql.contains("STOCK"))).collect();
    assert_eq!(ann.run(page, &stock, None), 3, "it shows its examples' output");
}
