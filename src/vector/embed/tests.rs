//! Unit tests of embedding: chunking (`vector/chunk.rs`), the built-in
//! models, the routines, and the plans of pipelines and auto-embedded
//! columns.

mod chunk;
mod hashed;
mod pipeline;
mod routines;

use std::sync::Arc;

use super::*;
use crate::db::{Database, Session};

/// The rows `sql` gives in `session`, each its values' text joined by
/// commas.
pub(super) fn rows(session: &Session, sql: &str) -> Result<Vec<String>> {
    let mut cursor = session.open_cursor(sql)?;
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch()? {
        out.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    Ok(out)
}

/// A database with VECTOR, the routines of the built-in models, and a
/// schema U.
pub(super) fn database() -> (Database, Arc<Embedders>) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    // Registered routines are called in generated columns and triggers
    // under the setting; built in, they would need none.
    db.set_setting("SQL.FUNCTION_CALLS", &crate::value::Value::Char("ON".to_owned())).unwrap();
    db.register_type(Box::new(crate::vector::VectorType)).unwrap();
    let models = Arc::new(Embedders::built_in());
    for routine in routines(&models) {
        db.register_scalar_function(routine).unwrap();
    }
    (db, models)
}

/// Verifies: [EXTENSION] (§12.1) The registry holds the built-in models,
/// finds a model by its name in any case, and refuses a second model of
/// a name and a name not in upper case.
#[test]
fn the_registry_names_its_models() {
    let models = Embedders::built_in();
    assert_eq!(models.names().unwrap(), [HASH_DENSE, HASH_SPARSE]);
    assert!(models.named("hash_dense").unwrap().is_some());
    assert!(models.named("NONE").unwrap().is_none());
    let again = models.register(Arc::new(HashedEmbedder::dense())).unwrap_err();
    assert_eq!(again.code, Code::Data);
    let lower = HashedEmbedder::new("x", 8, Layout::Dense, 1);
    assert_eq!(lower.name(), "X", "a hashed model's name is made upper case");
    models.register(Arc::new(lower)).unwrap();
    assert!(models.deterministic());
    let shape = models.named(HASH_DENSE).unwrap().unwrap().shape();
    assert_eq!(shape, ModelShape { dims: HASH_DENSE_DIMS, element: Element::Float32, layout: Layout::Dense });
    assert!(shape.data_type().is_ok());
}
