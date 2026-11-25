//! INFORMATION_SCHEMA.COLUMN_STORAGE (RULES.md D1165): what each column of
//! each base table takes, by encoding, of either store.

use turborel::db::Database;
use turborel::threads::JobPolicy;

use crate::common::rows;

/// A database whose owner is OWNER, its maintenance run only as a test
/// asks: C's column store table G, of 4,096 rows in groups of 2,048, its
/// key K, a text S of three values and a text Z of one; and C's row store
/// table R of 100 rows.
fn owned() -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION C \
           CREATE TABLE G (K INTEGER, S VARCHAR(10), Z VARCHAR(10)) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048) \
           CREATE TABLE R (K INTEGER, S VARCHAR(10))",
    )
    .unwrap();
    let c = db.session("C");
    for first in [0, 2048] {
        let values: Vec<String> = (first..first + 2048).map(|k| format!("({k}, 's{}', 'same')", k % 3)).collect();
        c.run(&format!("INSERT INTO G VALUES {}", values.join(", "))).unwrap();
    }
    let values: Vec<String> = (0..100).map(|k| format!("({k}, 'r{k}')")).collect();
    c.run(&format!("INSERT INTO R VALUES {}", values.join(", "))).unwrap();
    c.commit().unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.1]; [DESIGN] RULES.md
/// D1165: a column store's frozen columns are shown by the encoding each
/// chunk got, their chunks, rows, bytes and the pages they take, each
/// column's rows all the table's; its rows still in its tree are ROW, of
/// no chunk or page, their bytes not known before statistics are gathered;
/// TABLE_STATISTICS counts the table's bytes, not estimated.
#[test]
fn a_column_stores_chunks_are_shown_by_encoding() {
    let db = owned();
    let owner = db.session("OWNER");
    let encodings = "SELECT COLUMN_NAME, ENCODING, CHUNKS IS NULL, ROW_COUNT, BYTES IS NULL, PAGE_BYTES IS NULL \
                     FROM INFORMATION_SCHEMA.COLUMN_STORAGE WHERE TABLE_NAME = 'G' ORDER BY ORDINAL_POSITION, ENCODING";
    assert_eq!(
        rows(&owner, encodings).unwrap(),
        ["K,ROW,TRUE,4096,TRUE,TRUE", "S,ROW,TRUE,4096,TRUE,TRUE", "Z,ROW,TRUE,4096,TRUE,TRUE"],
        "in the tree, no width measured"
    );
    db.maintain().unwrap();
    let each = "SELECT COLUMN_NAME, SUM(ROW_COUNT), SUM(CHUNKS) >= 2, MIN(BYTES) > 0, SUM(BYTES) <= SUM(PAGE_BYTES), COUNT(*) \
                FROM INFORMATION_SCHEMA.COLUMN_STORAGE WHERE TABLE_NAME = 'G' AND ENCODING <> 'ROW' GROUP BY COLUMN_NAME ORDER BY COLUMN_NAME";
    assert_eq!(rows(&owner, each).unwrap(), ["K,4096,TRUE,TRUE,TRUE,1", "S,4096,TRUE,TRUE,TRUE,1", "Z,4096,TRUE,TRUE,TRUE,1"], "frozen");
    let kept = "SELECT COLUMN_NAME, ENCODING FROM INFORMATION_SCHEMA.COLUMN_STORAGE WHERE TABLE_NAME = 'G' ORDER BY ORDINAL_POSITION";
    let shown = rows(&owner, kept).unwrap();
    assert_eq!(shown.len(), 3, "{shown:?}");
    assert_eq!(shown[2], "Z,CONSTANT", "one value");
    assert!(["S,DICTIONARY", "S,FSST"].contains(&shown[1].as_str()), "{shown:?}");
    assert!(!shown[0].ends_with(",ROW") && !shown[0].ends_with(",NULL"), "{shown:?}");
    let table = "SELECT BYTES_ESTIMATED, BYTES >= (SELECT SUM(PAGE_BYTES) FROM INFORMATION_SCHEMA.COLUMN_STORAGE WHERE TABLE_NAME = 'G') \
                 FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_NAME = 'G'";
    assert_eq!(rows(&owner, table).unwrap(), ["NO,TRUE"], "its chunks' pages among the table's");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.1, §21.2.9]; [DESIGN]
/// RULES.md D1165: a row store's columns are ROW, their rows counted and
/// no bytes their own, gathered statistics or not, the tree's the
/// table's; a user sees the columns it holds a privilege on, none of a
/// table it holds none on.
#[test]
fn a_row_stores_columns_are_shown_by_their_rows_to_whom_may_see_them() {
    let db = owned();
    let owner = db.session("OWNER");
    let sql = "SELECT COLUMN_NAME, ENCODING, ROW_COUNT, BYTES FROM INFORMATION_SCHEMA.COLUMN_STORAGE WHERE TABLE_NAME = 'R' ORDER BY ORDINAL_POSITION";
    assert_eq!(rows(&owner, sql).unwrap(), ["K,ROW,100,NULL", "S,ROW,100,NULL"], "no bytes of a column of a tree");
    db.analyze("C.R").unwrap();
    assert_eq!(rows(&owner, sql).unwrap(), ["K,ROW,100,NULL", "S,ROW,100,NULL"], "nor once its statistics are gathered");
    let table = "SELECT BYTES > 0 FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_NAME = 'R'";
    assert_eq!(rows(&owner, table).unwrap(), ["TRUE"], "the tree's bytes the table's");
    db.session("C").run("GRANT UPDATE (S) ON R TO P").unwrap();
    db.session("C").commit().unwrap();
    assert_eq!(rows(&db.session("P"), sql).unwrap(), ["S,ROW,100,NULL"], "its column's alone");
    let any = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.COLUMN_STORAGE";
    assert_eq!(rows(&db.session("NOBODY"), any).unwrap(), ["0"], "none of a table it holds no privilege on");
}
