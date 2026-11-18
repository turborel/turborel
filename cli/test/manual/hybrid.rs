//! The examples of hybrid search, run as printed, of the built-in VECTOR
//! type, its functions and the hybrid index method.

use turborel::db::Database;

use super::Client;

/// Verifies: [DESIGN] RULES.md R-DOC-8, D94, D100: the examples of hybrid
/// search and of CREATE HYBRID VECTOR INDEX run as printed, as ANN, each
/// in a database of its own, the second of a table HELP.
#[test]
fn the_hybrid_search_examples_run_as_printed() {
    let db = Database::new();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/15-hybrid-search.md") > 0, "it shows its examples' output");
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE HELP (ID INTEGER PRIMARY KEY, BODY VARCHAR(200), EMB VECTOR(4))").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/statements/create-hybrid-vector-index.md") > 0, "it shows its examples' output");
}
