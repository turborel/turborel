//! The examples of CREATE VECTOR INDEX, run as printed, of the vector
//! methods every database has built in.

use turborel::db::Database;

use super::Client;

/// Verifies: [DESIGN] RULES.md R-DOC-8, D-APPROX, D88: the examples of
/// CREATE VECTOR INDEX run as printed, as ANN, of the tables DOCS and
/// UNIT and the built-in methods `hnsw`, `diskann` and `ivf`.
#[test]
fn the_vector_index_examples_run_as_printed() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION ANN \
         CREATE TABLE DOCS (ID INTEGER, EMBEDDING VECTOR(3, FLOAT32)) \
         CREATE TABLE UNIT (ID INTEGER, V VECTOR(2, FLOAT32))",
    )
    .unwrap();
    let s = db.session("ANN");
    s.run("INSERT INTO DOCS VALUES (1, '[1,0,0]'), (2, '[0.9,0.1,0]'), (3, '[0,1,0]'), (4, '[0,0,1]')").unwrap();
    s.run("INSERT INTO UNIT VALUES (1, '[1,0]'), (2, '[0.6,0.8]'), (3, '[0,1]')").unwrap();
    s.commit().unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/statements/create-vector-index.md") > 0, "it shows its examples' output");
}
