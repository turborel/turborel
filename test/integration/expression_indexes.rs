//! Indexes whose key parts are expressions (RULES.md D92): the
//! expressions admitted and those refused, the plans that read such an
//! index and the rows they find, the same as a scan's, and its entries
//! kept through writes, rollback, ALTER TABLE, a reopening and a failure.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::Code;
use turborel::params::Params;

use crate::common::{Scratch, exec, failure, locking_database, rows, sorted};

/// The rows of table T: ID, NAME, A, B and DOC, a JSON text.
const ROWS: i32 = 300;

/// Schema S, its table T (ID, NAME, A, B, DOC), A and B not null, of
/// `ROWS` rows, and the
/// table's twin P, the same rows and no index.
fn table(db: &Database) -> Session {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S \
         CREATE TABLE T (ID INTEGER NOT NULL, NAME VARCHAR(20), A INTEGER NOT NULL, B INTEGER NOT NULL, DOC VARCHAR(200)) \
         CREATE TABLE P (ID INTEGER NOT NULL, NAME VARCHAR(20), A INTEGER NOT NULL, B INTEGER NOT NULL, DOC VARCHAR(200))",
    )
    .unwrap();
    let s = db.session("S");
    for table in ["T", "P"] {
        let values: Vec<String> = (0..ROWS)
            .map(|n| format!("({n}, 'name{}', {}, {}, '{{\"k\":{}, \"c\":\"c{}\"}}')", n % 50, n % 7, n % 11, n % 40, n % 3))
            .collect();
        s.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    s
}

/// The physical plan of `sql`, as EXPLAIN shows it.
fn plan(s: &Session, sql: &str) -> String {
    let shown = s.explain(sql, &Params::new()).unwrap();
    shown.split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// The rows of `sql` of T, and of the same query of P, in order.
fn both(s: &Session, sql: &str) -> (Vec<String>, Vec<String>) {
    (rows(s, sql).unwrap(), rows(s, &sql.replace(" T ", " P ")).unwrap())
}

/// Verifies: [EXTENSION] RULES.md D92: an index of UPPER(NAME),
/// of A + B and of JSON_VALUE ... NULL ON ERROR, by btree and by hash, is
/// probed by `=`, a range and IN of an expression of the same form, finds
/// the rows a scan finds, and EXPLAIN shows the expression it probes.
#[test]
fn an_expression_is_probed_by_its_index() {
    let db = Database::new();
    let s = table(&db);
    s.run("CREATE INDEX T_UP ON T ((UPPER(NAME)))").unwrap();
    s.run("CREATE INDEX T_SUM ON T ((A + B), ID)").unwrap();
    s.run("CREATE INDEX T_K ON T ((JSON_VALUE(DOC, 'lax $.k' RETURNING INTEGER NULL ON ERROR))) USING hash").unwrap();
    let probes = [
        ("SELECT ID FROM T WHERE UPPER(NAME) = 'NAME7' ORDER BY ID", "T_UP"),
        ("SELECT ID FROM T WHERE UPPER(NAME) IN ('NAME7', 'NAME9') ORDER BY ID", "T_UP"),
        ("SELECT ID FROM T WHERE A + B = 3 ORDER BY ID", "T_SUM"),
        ("SELECT ID FROM T WHERE A + B BETWEEN 14 AND 15 ORDER BY ID", "T_SUM"),
        ("SELECT ID FROM T WHERE JSON_VALUE(DOC, 'lax $.k' RETURNING INTEGER NULL ON ERROR) = 12 ORDER BY ID", "T_K"),
    ];
    for (sql, index) in probes {
        let shown = plan(&s, sql);
        assert!(shown.contains(index), "{sql}: {shown}");
        let (found, scanned) = both(&s, sql);
        assert!(!found.is_empty(), "{sql}");
        assert_eq!(found, scanned, "{sql}");
    }
    assert!(plan(&s, "SELECT ID FROM T WHERE A + B = 3").contains("T_SUM [btree]: (T.A + T.B) = 3"), "the probe shows the expression");
    assert!(!plan(&s, "SELECT ID FROM T WHERE B + A = 3").contains("T_SUM"), "another form is another expression");
    assert!(!plan(&s, "SELECT ID FROM T WHERE LOWER(NAME) = 'name7'").contains("T_UP"));
}

/// Verifies: [EXTENSION] RULES.md D92: a btree index of an
/// expression never null gives ORDER BY of it its order, and of a GROUP BY
/// of it, so no sort is planned, in either direction, the rows those of a
/// sort.
#[test]
fn an_expression_index_gives_its_order() {
    let db = Database::new();
    let s = table(&db);
    s.run("CREATE INDEX T_SUM ON T ((A + B), ID)").unwrap();
    let ordered = [
        "SELECT A + B FROM T ORDER BY A + B FETCH FIRST 5 ROWS ONLY",
        "SELECT ID FROM T ORDER BY A + B DESC, ID DESC FETCH FIRST 5 ROWS ONLY",
        "SELECT S, COUNT(*) FROM (SELECT A + B AS S FROM T WHERE A + B BETWEEN 3 AND 4) AS D GROUP BY S ORDER BY S",
    ];
    for sql in ordered {
        let shown = plan(&s, sql);
        assert!(shown.contains("T_SUM") && !shown.contains("Sort"), "{sql}: {shown}");
        let (found, scanned) = both(&s, sql);
        assert_eq!(found, scanned, "{sql}");
    }
}

/// Verifies: [EXTENSION] RULES.md D92: INSERT, UPDATE of an input
/// column and of another, DELETE and ROLLBACK keep the index's entries,
/// so a probe finds what a scan of the same rows finds, in the row store,
/// which keeps versions, and in an engine that keeps one, where an UPDATE
/// of no input column leaves the entry; and INDEXES shows the key part as
/// written, in parentheses.
#[test]
fn writes_keep_an_expression_index() {
    for db in [Database::new(), locking_database()] {
        written(&db);
    }
}

/// `writes_keep_an_expression_index` of `db`.
fn written(db: &Database) {
    let s = table(db);
    s.run("CREATE INDEX T_UP ON T ((UPPER(NAME)), ID)").unwrap();
    let writes = [
        "INSERT INTO T VALUES (1000, 'Zed', 1, 1, NULL)",
        "UPDATE T SET NAME = 'zed' WHERE ID < 10",
        "UPDATE T SET B = B + 1 WHERE ID < 20",
        "DELETE FROM T WHERE ID BETWEEN 5 AND 7",
    ];
    for w in writes {
        s.run(w).unwrap();
        s.run(&w.replace(" T ", " P ")).unwrap();
    }
    let sql = "SELECT ID, B FROM T WHERE UPPER(NAME) = 'ZED' ORDER BY ID";
    assert!(plan(&s, sql).contains("T_UP"));
    let (found, scanned) = both(&s, sql);
    assert_eq!(found.len(), 8);
    assert_eq!(found, scanned);
    s.rollback().unwrap();
    assert_eq!(rows(&s, sql).unwrap().len(), 0, "rolled back");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE UPPER(NAME) = 'NAME3'").unwrap().len(), 6);
    let listed = "SELECT COLUMN_NAMES FROM INFORMATION_SCHEMA.INDEXES WHERE INDEX_NAME = 'T_UP'";
    assert_eq!(rows(&s, listed).unwrap().iter().map(|r| r.replace(' ', "")).collect::<Vec<_>>(), ["(UPPER(NAME)),ID"]);
}

/// Verifies: [EXTENSION] RULES.md D92, D15: an expression that can
/// raise an exception condition, or is not deterministic, is no key part:
/// a quotient, a product that can overflow, a CAST, CURRENT_DATE, JSON_VALUE
/// ERROR ON ERROR or of a path with datetime(), and a subquery are each
/// refused with SQLCODE -103; so is a method other than btree and hash of
/// an expression, and a column the table lacks.
#[test]
fn an_expression_that_can_raise_is_refused() {
    let db = Database::new();
    let s = table(&db);
    let refused = [
        "CREATE INDEX X ON T ((A / B))",
        "CREATE INDEX X ON T ((A * B * A * B))",
        "CREATE INDEX X ON T ((CAST(NAME AS INTEGER)))",
        "CREATE INDEX X ON T ((CURRENT_DATE))",
        "CREATE INDEX X ON T ((JSON_VALUE(DOC, 'lax $.k' RETURNING INTEGER ERROR ON ERROR)))",
        "CREATE INDEX X ON T ((JSON_VALUE(DOC, 'lax $.k.datetime()' NULL ON ERROR)))",
        "CREATE INDEX X ON T (((SELECT 1 FROM P FETCH FIRST 1 ROW ONLY)))",
        "CREATE INDEX X ON T ((UPPER(NAME))) USING rtree",
        "CREATE INDEX X ON T ((UPPER(NOPE)))",
    ];
    for sql in refused {
        assert_eq!(failure(s.run(sql)).0, Code::SyntaxRule, "{sql}");
        s.rollback().unwrap();
    }
    assert_eq!(rows(&s, "SELECT INDEX_NAME FROM INFORMATION_SCHEMA.INDEXES WHERE INDEX_NAME = 'X'").unwrap().len(), 0);
    s.rollback().unwrap();
    s.run("CREATE INDEX X ON T ((A + B + 1), (-A))").unwrap();
}

/// Verifies: [EXTENSION] RULES.md D92, D31: a column an index's
/// expression reads is not renamed (SQLCODE -103); dropped, it takes the
/// index with it; another column dropped moves the expression's input,
/// read where it is now.
#[test]
fn altering_an_input_column_keeps_or_drops_the_index() {
    let db = Database::new();
    let s = table(&db);
    s.run("CREATE INDEX T_SUM ON T ((A + B))").unwrap();
    s.run("CREATE INDEX T_UP ON T ((UPPER(NAME)))").unwrap();
    s.commit().unwrap();
    assert_eq!(exec(&s, "ALTER TABLE T RENAME COLUMN A TO AA").err(), Some(Code::SyntaxRule));
    s.rollback().unwrap();
    s.run("ALTER TABLE T DROP COLUMN ID RESTRICT").unwrap();
    let sql = "SELECT NAME FROM T WHERE A + B = 3 ORDER BY NAME";
    assert!(plan(&s, sql).contains("T_SUM"));
    assert_eq!(rows(&s, sql).unwrap(), rows(&s, "SELECT NAME FROM P WHERE A + B = 3 ORDER BY NAME").unwrap());
    s.run("ALTER TABLE T DROP COLUMN NAME CASCADE").unwrap();
    let left = sorted(&s, "SELECT INDEX_NAME FROM INFORMATION_SCHEMA.INDEXES WHERE TABLE_NAME = 'T'");
    assert_eq!(left, ["T_SUM"]);
}

/// Verifies: [EXTENSION] RULES.md D92, D25: an expression index is
/// kept as written and bound again as the database opens, from a
/// checkpoint after it closes and from its log alone, as after a failure,
/// its entries those of the rows written before.
#[test]
fn an_expression_index_survives_reopening() {
    for closed in [true, false] {
        let scratch = Scratch::new(if closed { "expression-index-closed" } else { "expression-index-failed" });
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let db = Database::new();
        db.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        let s = table(&db);
        s.run("CREATE INDEX T_UP ON T ((UPPER(NAME)))").unwrap();
        s.run("INSERT INTO T VALUES (1000, 'Zed', 1, 1, NULL)").unwrap();
        s.commit().unwrap();
        if closed {
            db.close().unwrap();
        }
        drop(s);
        drop(db);
        let db = Database::new();
        db.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        let s = db.session("S");
        let sql = "SELECT ID FROM T WHERE UPPER(NAME) = 'ZED'";
        assert!(plan(&s, sql).contains("T_UP"), "{}", plan(&s, sql));
        assert_eq!(rows(&s, sql).unwrap(), ["1000"]);
        assert_eq!(rows(&s, "SELECT ID FROM T WHERE UPPER(NAME) = 'NAME3'").unwrap().len(), 6);
        s.commit().unwrap();
        db.close().unwrap();
    }
}

/// Verifies: [EXTENSION] RULES.md D92, D49: an expression index on
/// a temporary table holds each session's own rows, probed as any is.
#[test]
fn a_temporary_table_takes_an_expression_index() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE GLOBAL TEMPORARY TABLE G (K INTEGER, V VARCHAR(9)) ON COMMIT PRESERVE ROWS").unwrap();
    let (a, b) = (db.session("S"), db.session("S"));
    for k in 0..100 {
        a.run(&format!("INSERT INTO G VALUES ({k}, 'v{}')", k % 10)).unwrap();
    }
    a.commit().unwrap();
    a.run("CREATE INDEX G_UP ON G ((UPPER(V)))").unwrap();
    b.run("INSERT INTO G VALUES (1, 'v3')").unwrap();
    let sql = "SELECT K FROM G WHERE UPPER(V) = 'V3'";
    assert!(plan(&a, sql).contains("G_UP"), "{}", plan(&a, sql));
    assert_eq!(sorted(&a, sql).len(), 10);
    assert_eq!(sorted(&b, sql), ["1"]);
}
