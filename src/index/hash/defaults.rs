//! Unit tests of what an index kind answers by the defaults of `Index`
//! and `IndexMethod` (`index.rs`), of the hash index, which overrides
//! none of them.

use super::*;
use crate::catalog::{IndexDesc, QualifiedName};
use crate::functions::ScalarRef;
use crate::index::NearestQuery;
use crate::memory::MemoryBudget;

/// Verifies: [DESIGN] RULES.md D85, an index kind serves only the queries
/// it says it serves: the hash index measures by no metric, finds no
/// nearest rows, has no adjacency, members or reachability of a graph, no
/// ranking statistics of a score and no envelopes of a spatial key; each
/// asked of it, it answers none rather than failing.
#[test]
fn an_index_of_no_such_answers_gives_none() {
    let method = HashMethod::in_memory();
    let desc = IndexDesc {
        name: "I".to_owned(),
        table: QualifiedName { auth: "U".to_owned(), ident: "T".to_owned() },
        columns: vec![0],
        method: "HASH".to_owned(),
        unique: false,
        options: Vec::new(),
        collations: Vec::new(),
        nulls: false,
        expressions: Vec::new(),
    };
    assert!(method.metric(&desc).is_none());
    let t = DataType::Exact { precision: 9, scale: 0, spelled: crate::value::Spelled::Integer };
    let index = Arc::<dyn Index>::from(method.create(std::slice::from_ref(&t)).unwrap());
    let query = NearestQuery {
        function: ScalarRef(Arc::new(crate::functions::CardinalityOf(t.clone()))),
        column: 0,
        argument: 0,
        value: Value::Null,
        descending: false,
        statistics: None,
        metric: None,
        approximate: None,
    };
    assert!(Arc::clone(&index).nearest(&query, &MemoryBudget::new(usize::MAX)).unwrap().is_none());
    assert!(Arc::clone(&index).adjacency().is_none());
    assert!(Arc::clone(&index).members().is_none());
    assert!(Arc::clone(&index).reachability().is_none());
    assert!(Arc::clone(&index).ranking_statistics().unwrap().is_none());
    assert!(index.envelopes().is_none());
}
