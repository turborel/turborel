//! The adjacency index's options, kept edges, budget, temporary tables,
//! joins, refusals and Information Schema views (RULES.md D41, D49,
//! D107).

use turborel::db::Database;
use turborel::error::Code;

use super::{agree, change, compacted, graph, probed};
use crate::common::{exec, expected, rows};

/// Verifies: [EXTENSION] RULES.md D38, D41: of ADJACENCY.CSR_BYTES too
/// small for a compacted adjacency, 0 or a few bytes, maintenance builds
/// none, and lookups read the tree alone, still the scan's; set in place
/// by ALTER INDEX, a compacted adjacency built is dropped, and lookups
/// stay the scan's.
#[test]
fn a_budget_too_small_reads_the_tree() {
    for bytes in ["0", "4096"] {
        let db = Database::new();
        let s = graph(&db, 17, &format!(", ADJACENCY.CSR_BYTES = {bytes}"));
        db.maintain().unwrap();
        agree(&s, bytes);
        change(&s);
        exec(&s, "COMMIT WORK").unwrap();
        db.maintain().unwrap();
        agree(&s, bytes);
        assert_eq!(compacted(&db, &s), 0, "{bytes}");
    }
    let db = Database::new();
    let s = graph(&db, 19, "");
    db.maintain().unwrap();
    assert_eq!(compacted(&db, &s), 1);
    exec(&s, "COMMIT WORK").unwrap();
    exec(&s, "ALTER INDEX E_ADJ SET (ADJACENCY.CSR_BYTES = 0)").unwrap();
    assert_eq!(compacted(&db, &s), 0, "dropped in place");
    agree(&s, "dropped in place");
    let set = "SELECT OPTION_VALUE, CHANGED_BY FROM INFORMATION_SCHEMA.INDEX_OPTIONS WHERE OPTION_NAME = 'ADJACENCY.CSR_BYTES'";
    assert_eq!(rows(&s, set), expected(&["0,IN PLACE"]));
}

/// Verifies: [EXTENSION] RULES.md D41: an index of DIRECTION OUT or IN,
/// or of two source columns, finds what a scan finds; ALTER INDEX of its
/// layout makes it anew; options out of range, or a layout with no
/// destination column, are refused.
#[test]
fn layouts_find_alike() {
    for with in [", ADJACENCY.DIRECTION = 'IN'", ", ADJACENCY.DIRECTION = 'out'"] {
        let db = Database::new();
        let s = graph(&db, 23, with);
        db.maintain().unwrap();
        agree(&s, with);
        change(&s);
        agree(&s, with);
    }
    let db = Database::new();
    let s = graph(&db, 29, "");
    exec(&s, "COMMIT WORK").unwrap();
    exec(&s, "ALTER INDEX E_ADJ SET (ADJACENCY.SOURCE_COLUMNS = 2, ADJACENCY.INCLUDE = 0)").unwrap();
    agree(&s, "two source columns");
    let refused = [
        ("ALTER INDEX E_ADJ SET (ADJACENCY.SOURCE_COLUMNS = 0)", Code::SyntaxRule),
        ("ALTER INDEX E_ADJ SET (ADJACENCY.DIRECTION = 'SIDEWAYS')", Code::SyntaxRule),
        ("ALTER INDEX E_ADJ SET (ADJACENCY.INCLUDE = 1)", Code::Unsupported),
        ("CREATE INDEX E_ONE ON E (SRC) USING adjacency", Code::Unsupported),
    ];
    for (sql, code) in refused {
        exec(&s, "ROLLBACK WORK").unwrap();
        assert_eq!(exec(&s, sql).map(drop), Err(code), "{sql}");
    }
    agree(&s, "after the refusals");
}

/// Verifies: [EXTENSION] an included column must be NOT NULL, as a row
/// with a null in its key has no entry; a column with a collation, indexed
/// by its sort key, is refused, as a step gives a vertex's own key.
#[test]
fn indexes_it_cannot_keep_are_refused() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION G CREATE TABLE E (SRC INTEGER, DST INTEGER, AMOUNT INTEGER, \
         NAME VARCHAR(10) COLLATE CASE_INSENSITIVE)",
    )
    .unwrap();
    let s = db.session("G");
    assert!(exec(&s, "CREATE INDEX E_ADJ ON E (SRC, DST, AMOUNT) USING adjacency WITH (ADJACENCY.INCLUDE = 1)").is_err());
    assert!(exec(&s, "CREATE INDEX E_ADJ ON E (NAME, DST) USING adjacency").is_err());
    exec(&s, "CREATE INDEX E_ADJ ON E (SRC, DST) USING adjacency").unwrap();
    exec(&s, "INSERT INTO E VALUES (1, NULL, 1, 'a'), (1, 2, NULL, 'b')").unwrap();
    assert_eq!(rows(&s, "SELECT AMOUNT FROM E WHERE SRC = 1 ORDER BY AMOUNT"), expected(&["1", "NULL"]), "a null endpoint, read");
}

/// Verifies: [EXTENSION] RULES.md D49: a temporary table's adjacency
/// index holds each session's own rows, and its lookups find them alone.
#[test]
fn a_temporary_table_has_an_adjacency_index_of_its_own_rows() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION G CREATE GLOBAL TEMPORARY TABLE T (ID INTEGER, SRC INTEGER, DST INTEGER) ON COMMIT PRESERVE ROWS",
    )
    .unwrap();
    let (one, two) = (db.session("G"), db.session("G"));
    exec(&one, "CREATE INDEX T_ADJ ON T (SRC, DST) USING adjacency").unwrap();
    exec(&one, "INSERT INTO T VALUES (1, 5, 6), (2, 5, 7), (3, 6, 5)").unwrap();
    exec(&two, "INSERT INTO T VALUES (4, 5, 9)").unwrap();
    assert_eq!(rows(&one, "SELECT ID FROM T WHERE SRC = 5 ORDER BY ID"), expected(&["1", "2"]));
    assert_eq!(rows(&two, "SELECT ID FROM T WHERE SRC = 5 ORDER BY ID"), expected(&["4"]));
    exec(&one, "DELETE FROM T WHERE ID = 1").unwrap();
    assert_eq!(rows(&one, "SELECT DST FROM T WHERE SRC = 5"), expected(&["7"]));
}

/// Verifies: [EXTENSION] RULES.md D35: the planner looks each row of a
/// selective table up in the adjacency index for a join equating its
/// column with the source, the rows those of the scan's join.
#[test]
fn the_planner_joins_through_it() {
    let db = Database::new();
    let s = graph(&db, 31, "");
    exec(&s, "CREATE TABLE V (ID INTEGER NOT NULL, NAME VARCHAR(10) NOT NULL)").unwrap();
    let names: Vec<String> = (0..100).map(|id| format!("({id}, 'v{id}')")).collect();
    exec(&s, &format!("INSERT INTO V VALUES {}", names.join(", "))).unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    db.maintain().unwrap();
    let join = "SELECT V.NAME, {}.DST, {}.AMOUNT FROM V JOIN {} ON {}.SRC = V.ID WHERE V.NAME IN ('v5', 'v42') ORDER BY 1, 2, 3";
    assert!(probed(&s, join), "{}", s.physical_plan(&join.replace("{}", "E"), &turborel::params::Params::new()).unwrap());
    assert_eq!(rows(&s, &join.replace("{}", "E")), rows(&s, &join.replace("{}", "P")));
}

/// Verifies: [EXTENSION] INDEXES shows an adjacency index by its method,
/// and ADJACENCY_INDEXES with the options laying out its key.
#[test]
fn the_information_schema_shows_it() {
    let db = Database::new();
    let s = graph(&db, 37, ", ADJACENCY.DIRECTION = 'OUT'");
    let index = "SELECT METHOD_NAME, COLUMN_NAMES FROM INFORMATION_SCHEMA.INDEXES WHERE INDEX_NAME = 'E_ADJ'";
    assert_eq!(rows(&s, index), expected(&["adjacency,SRC, DST, AMOUNT"]));
    let adjacency = "SELECT TABLE_NAME, INDEX_NAME, SOURCE_COLUMNS, INCLUDED_COLUMNS, DIRECTION, CSR_BYTES \
                     FROM INFORMATION_SCHEMA.ADJACENCY_INDEXES";
    assert_eq!(rows(&s, adjacency), expected(&["E,E_ADJ,1,1,OUT,268435456"]));
}
