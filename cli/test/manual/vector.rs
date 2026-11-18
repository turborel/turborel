//! The examples of CREATE VECTOR INDEX, run as printed, of the vector
//! methods every database has built in.

use turborel::db::Database;

use super::Client;

/// Verifies: [DESIGN] RULES.md R-DOC-8, D94, D93: the examples of
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

/// Verifies: [DESIGN] RULES.md R-DOC-8, D93: the examples of the Vectors
/// chapter, and of CREATE then DROP VECTOR DISTANCE FUNCTION, each in a
/// database of its own whose schema ANN they run in, and the vector guide,
/// which makes its schema itself, run as printed, as ANN.
#[test]
fn the_vector_examples_run_as_printed() {
    let pages: [&[&str]; 2] =
        [&["sql/14-vectors.md"], &["sql/statements/create-vector-distance-function.md", "sql/statements/drop-vector-distance-function.md"]];
    for pages in pages {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
        let mut ann = Client::of(&db, "ANN");
        for page in pages {
            assert!(ann.page(page) > 0, "{page} shows its examples' output");
        }
    }
    let db = Database::new();
    assert!(Client::of(&db, "ANN").page("data/04-vectors.md") > 0, "the guide shows its examples' output");
}
