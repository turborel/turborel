//! The examples of SQL/XML, run as printed: the XML type, its functions
//! and predicates, CREATE and DROP XML SCHEMA, and CREATE, ALTER and DROP
//! XQUERY MODULE, and the guide to XML data.

use turborel::db::Database;

use super::Client;

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.8, §6.16] [DESIGN] RULES.md
/// R-DOC-8, D106: the examples of XML values run as printed, as ANN.
#[test]
fn the_xml_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/19-xml.md") > 0, "it shows its examples' output");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D106: the guide to XML data runs
/// as printed, as ANN.
#[test]
fn the_xml_guide_runs_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("data/05-xml.md") > 0, "the guide shows its examples' output");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §4.2.6] [DESIGN]
/// RULES.md R-DOC-8, D106: the examples of CREATE XML SCHEMA and of DROP
/// XML SCHEMA run as printed, as ANN, each page in a database of its own.
#[test]
fn the_xml_schema_examples_run_as_printed() {
    for page in ["sql/statements/create-xml-schema.md", "sql/statements/drop-xml-schema.md"] {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
        let mut ann = Client::of(&db, "ANN");
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
}

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D106 [XQuery §5.2.5]: the
/// examples of CREATE, ALTER and DROP XQUERY MODULE run as printed, as
/// ANN, each page in a database of its own.
#[test]
fn the_xquery_module_examples_run_as_printed() {
    for page in ["sql/statements/create-xquery-module.md", "sql/statements/alter-xquery-module.md", "sql/statements/drop-xquery-module.md"]
    {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
        let mut ann = Client::of(&db, "ANN");
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
}
