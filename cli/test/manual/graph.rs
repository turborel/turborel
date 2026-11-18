//! The manual's examples of property graphs (`doc/`), run as printed.

use turborel::db::Database;

use super::Client;

/// Verifies: [DESIGN] RULES.md R-DOC-8, D-GRAPH-CATALOG: the examples of
/// CREATE PROPERTY GRAPH, then those of ALTER PROPERTY GRAPH and of DROP
/// PROPERTY GRAPH, run as printed, as ANN.
#[test]
fn the_property_graph_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let pages =
        ["sql/statements/create-property-graph.md", "sql/statements/alter-property-graph.md", "sql/statements/drop-property-graph.md"];
    for page in pages {
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
}

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D-GQL: the examples of CREATE
/// GRAPH TYPE, CREATE GRAPH, EXECUTE GQL, DROP GRAPH and DROP GRAPH TYPE,
/// run as printed, as ANN.
#[test]
fn the_gql_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let pages = [
        "sql/statements/create-graph-type.md",
        "sql/statements/create-graph.md",
        "sql/statements/execute-gql.md",
        "sql/statements/drop-graph.md",
        "sql/statements/drop-graph-type.md",
    ];
    for page in pages {
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
}
