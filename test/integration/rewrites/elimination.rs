//! Joins that cannot change the rows removed (RULES.md D375,
//! `optimizer/elimination.rs`).

use turborel::db::{Database, Session};
use turborel::optimizer::ELIMINATE_JOINS;
use turborel::threads::JobPolicy;

use super::{COLUMNS, alike, left};

/// The tables of schema H, each name after `{P}`: DEPT keyed by ID; EMP
/// referencing it by DEPT, NOT NULL, and by BOSS, nullable; LATE
/// referencing it by a deferrable constraint; U of a nullable unique key;
/// DD of a deferrable one.
const TABLES: [&str; 5] = [
    "CREATE TABLE {P}DEPT (ID INTEGER PRIMARY KEY, NAME VARCHAR(10), CODE INTEGER NOT NULL UNIQUE)",
    "CREATE TABLE {P}EMP (ID INTEGER PRIMARY KEY, DEPT INTEGER NOT NULL REFERENCES {P}DEPT, BOSS INTEGER REFERENCES {P}DEPT, PAY INTEGER)",
    "CREATE TABLE {P}LATE (ID INTEGER, DEPT INTEGER NOT NULL REFERENCES {P}DEPT DEFERRABLE)",
    "CREATE TABLE {P}U (K INTEGER UNIQUE, V INTEGER)",
    "CREATE TABLE {P}DD (K INTEGER NOT NULL, V INTEGER, UNIQUE (K) DEFERRABLE)",
];

/// What makes a table of the column store.
const COLUMN_STORE: &str = " USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)";

/// Fills the tables of `prefix`: EMP's rows each of a department, some
/// of none's boss, and an employee of no department's code.
fn fill(s: &Session, prefix: &str) {
    let insert = |table: &str, rows: &str| s.run(&format!("INSERT INTO {prefix}{table} VALUES {rows}")).unwrap();
    insert("DEPT", "(1, 'a', 10), (2, 'b', 20), (3, NULL, 30), (4, 'd', 40)");
    insert("EMP", "(1, 1, NULL, 100), (2, 1, 2, 0), (3, 2, NULL, 300), (4, 3, 1, 400), (5, 3, 3, NULL)");
    insert("LATE", "(1, 1), (2, 2), (3, 2)");
    insert("U", "(1, 1), (NULL, 2), (NULL, 3), (2, 4)");
    insert("DD", "(1, 1), (2, 2)");
}

/// Schema H in the row store, and with the prefix `COLUMNS` in the
/// column store, filled alike.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let rows_of: Vec<String> = TABLES.iter().map(|t| t.replace("{P}", "")).collect();
    let columns_of: Vec<String> = TABLES.iter().map(|t| format!("{}{COLUMN_STORE}", t.replace("{P}", COLUMNS))).collect();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION H {} {}", rows_of.join(" "), columns_of.join(" "))).unwrap();
    let s = db.session("H");
    fill(&s, "");
    fill(&s, COLUMNS);
    s.commit().unwrap();
    db.maintain().unwrap();
    (db, s)
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR1c, GR5]; [ISO/IEC 9075-2:2023
/// §4.25.3.2, §4.25.3.3]; [DESIGN] RULES.md D375: a LEFT or RIGHT join to
/// a table on a unique key NOT NULL, unread above, and an inner join on a
/// referential constraint NOT DEFERRABLE of NOT NULL columns to the
/// referenced table unfiltered and unread, are removed, the rows alike;
/// so through a view, of both stores.
#[test]
fn joins_on_keys_are_removed() {
    let (_db, s) = tables();
    for sql in [
        "SELECT E.ID, E.PAY FROM {EMP} E LEFT JOIN {DEPT} D ON E.DEPT = D.ID",
        "SELECT E.ID FROM {DEPT} D RIGHT JOIN {EMP} E ON D.ID = E.BOSS",
        "SELECT E.ID FROM {EMP} E LEFT JOIN {DEPT} D ON E.PAY / 10 = D.CODE AND D.NAME IS NOT NULL",
        "SELECT E.ID, E.PAY FROM {EMP} E JOIN {DEPT} D ON E.DEPT = D.ID",
        "SELECT E.ID, E.PAY FROM {EMP} E, {DEPT} D WHERE D.ID = E.DEPT AND E.PAY > 100",
        "SELECT E.ID, L.ID FROM {EMP} E JOIN {DEPT} D ON E.DEPT = D.ID JOIN {LATE} L ON L.ID = E.ID",
        "SELECT DISTINCT E.DEPT FROM {EMP} E LEFT JOIN {DEPT} D ON E.DEPT = D.ID",
        "SELECT COUNT(*) FROM (SELECT E.ID FROM {EMP} E JOIN {DEPT} D ON E.DEPT = D.ID) X",
    ] {
        alike(&s, ELIMINATE_JOINS, sql);
    }
    for prefix in ["", COLUMNS] {
        s.run(&format!(
            "CREATE VIEW {prefix}STAFF AS SELECT E.ID, E.PAY, D.NAME FROM {prefix}EMP E LEFT JOIN {prefix}DEPT D ON E.DEPT = D.ID"
        ))
        .unwrap();
    }
    s.commit().unwrap();
    alike(&s, ELIMINATE_JOINS, "SELECT ID, PAY FROM {STAFF} WHERE PAY IS NOT NULL");
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.25.2, §4.25.3.2]; [DESIGN] RULES.md
/// D375: a join is kept on a nullable unique key, a deferrable one, a
/// deferrable or nullable referential constraint, a column of the joined
/// table read above, a referenced table filtered, a FULL join, and an ON
/// that can raise, the rows and errors alike with the rule and without.
#[test]
fn joins_that_could_change_rows_are_kept() {
    let (_db, s) = tables();
    for sql in [
        "SELECT E.ID FROM {EMP} E LEFT JOIN {U} U ON E.DEPT = U.K",
        "SELECT E.ID FROM {EMP} E LEFT JOIN {DD} X ON E.DEPT = X.K",
        "SELECT L.ID FROM {LATE} L JOIN {DEPT} D ON L.DEPT = D.ID",
        "SELECT E.ID FROM {EMP} E JOIN {DEPT} D ON E.BOSS = D.ID",
        "SELECT E.ID, D.NAME FROM {EMP} E LEFT JOIN {DEPT} D ON E.DEPT = D.ID",
        "SELECT E.ID, D.NAME FROM {EMP} E JOIN {DEPT} D ON E.DEPT = D.ID",
        "SELECT E.ID FROM {EMP} E JOIN {DEPT} D ON E.DEPT = D.ID AND D.NAME = 'a'",
        "SELECT E.ID FROM {EMP} E FULL JOIN {DEPT} D ON E.DEPT = D.ID",
        "SELECT E.ID FROM {EMP} E LEFT JOIN {DEPT} D ON E.DEPT = D.ID AND 100 / E.PAY > D.ID",
        "SELECT E.ID FROM {EMP} E LEFT JOIN {DEPT} D ON E.DEPT = D.ID OR E.BOSS = D.ID",
    ] {
        left(&s, ELIMINATE_JOINS, sql);
    }
}
