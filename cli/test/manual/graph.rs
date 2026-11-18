//! The manual's examples of property graphs (`doc/`), run as printed.

use turborel::db::Database;

use super::Client;

/// Verifies: [DESIGN] RULES.md R-DOC-8, D106: the examples of
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

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D110: the examples of CREATE
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

/// Verifies: [DESIGN] RULES.md R-DOC-8, D111: the chapter of the
/// graph algorithms runs as printed, as ANN.
#[test]
fn the_graph_algorithm_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/20-graph-algorithms.md") > 0, "the chapter shows its examples' output");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D107, D108: the SQL Reference
/// Manual's chapter of property graphs and the example of CREATE
/// ADJACENCY INDEX run as printed, each as ANN in a database of its own;
/// the guide to graphs, which makes ANN's schema itself, does too.
#[test]
fn the_graph_table_examples_run_as_printed() {
    for page in ["sql/19-property-graphs.md", "sql/statements/create-adjacency-index.md"] {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
        let mut ann = Client::of(&db, "ANN");
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
    let db = Database::new();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("data/05-graphs.md") > 0, "the guide shows its examples' output");
}
