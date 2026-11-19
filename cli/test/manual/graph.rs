//! The manual's examples of property graphs (`doc/`), run as printed.

use turborel::db::Database;

use super::Client;

/// Verifies: [DESIGN] RULES.md R-DOC-8, D140: the examples of
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

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D144: the examples of CREATE
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

/// Verifies: [DESIGN] RULES.md R-DOC-8, D145: the chapter of the
/// graph algorithms runs as printed, as ANN.
#[test]
fn the_graph_algorithm_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/21-graph-algorithms.md") > 0, "the chapter shows its examples' output");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D141, D142, D149, D150: the SQL
/// Reference Manual's chapter of property graphs and the examples of
/// CREATE ADJACENCY INDEX, CREATE LABEL INDEX and CREATE REACHABILITY
/// INDEX run as printed, each as ANN in a database of its own;
/// the guide to graphs, which makes ANN's schema itself, does too.
#[test]
fn the_graph_table_examples_run_as_printed() {
    let pages = [
        "sql/20-property-graphs.md",
        "sql/statements/create-adjacency-index.md",
        "sql/statements/create-label-index.md",
        "sql/statements/create-reachability-index.md",
    ];
    for page in pages {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
        let mut ann = Client::of(&db, "ANN");
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
    let db = Database::new();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("data/05-graphs.md") > 0, "the guide shows its examples' output");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D151: the examples of
/// GRAPH_STATISTICS of the Information Schema's chapter run as printed,
/// as ANN, the statistics of its tables gathered after the first, as the
/// chapter says the maintenance has.
#[test]
fn the_graph_statistics_examples_run_as_printed() {
    let page = "sql/07-information-schema.md";
    let text = super::text_of(page);
    let section = text.split("## Graph statistics").nth(1).and_then(|s| s.split("The standard:").next()).unwrap();
    let examples = super::examples_in(section);
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let (first, rest) = examples.split_at(1);
    ann.run(page, first, None);
    db.analyze("ANN.CITY").unwrap();
    db.analyze("ANN.ROAD").unwrap();
    assert_eq!(ann.run(page, rest, None), 2, "{page} shows its examples' output");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8: the chapter of the graph
/// compatibility syntax runs as printed, as ANN.
#[test]
fn the_graph_compatibility_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/22-graph-compatibility.md") > 0, "the chapter shows its examples' output");
}
