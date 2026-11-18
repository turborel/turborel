//! The examples of the statements of embedding models and vector
//! pipelines, run as printed, each page of a database of its own, VECTOR
//! built in.

use turborel::db::Database;

use super::Client;

/// Verifies: [DESIGN] RULES.md R-DOC-8: the examples of CREATE, ALTER
/// and DROP EMBEDDING MODEL, and of CREATE and DROP VECTOR PIPELINE, run
/// as printed, as ANN.
#[test]
fn the_embedding_examples_run_as_printed() {
    let pages = [
        "sql/statements/create-embedding-model.md",
        "sql/statements/alter-embedding-model.md",
        "sql/statements/drop-embedding-model.md",
        "sql/statements/create-vector-pipeline.md",
        "sql/statements/drop-vector-pipeline.md",
    ];
    for page in pages {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
        assert!(Client::of(&db, "ANN").page(page) > 0, "{page} shows its examples' output");
    }
}
