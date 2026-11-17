//! Temporal foreign keys [ISO/IEC 9075-2:2023 §4.25.3.3, §11.8]: FOREIGN
//! KEY (c, PERIOD p) REFERENCES t (c, PERIOD q), whose rows' periods are
//! covered by the periods of the referenced rows their values match.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError};
use turborel::value::Value;

use crate::common::{Scratch, rows};

/// The text that cites a row not covered.
const UNCOVERED: &str = "references no row of";

/// Schema A, its table PRICES of the period VALID, keyed by ITEM WITHOUT
/// OVERLAPS of it, of item 1 through January and February 2026 in two
/// rows and item 2 through January, and its table ORDERS of the period
/// HELD, whose items reference PRICES for their period.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    ok(&db.session("A"), &prices());
    db
}

/// The statements `database` runs.
fn prices() -> [&'static str; 3] {
    [
        "CREATE TABLE PRICES (ITEM INTEGER, PRICE INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR VALID (S, E), \
         PRIMARY KEY (ITEM, VALID WITHOUT OVERLAPS))",
        "INSERT INTO PRICES VALUES (1, 10, DATE '2026-01-01', DATE '2026-02-01'), (1, 11, DATE '2026-02-01', DATE '2026-03-01'), \
         (2, 20, DATE '2026-01-01', DATE '2026-02-01')",
        "CREATE TABLE ORDERS (ITEM INTEGER, F DATE NOT NULL, T DATE NOT NULL, PERIOD FOR HELD (F, T), \
         CONSTRAINT PRICED FOREIGN KEY (ITEM, PERIOD HELD) REFERENCES PRICES (ITEM, PERIOD VALID))",
    ]
}

/// Runs each of `sql` in `s`, which must succeed, and commits.
fn ok(s: &Session, sql: &[&str]) {
    for one in sql {
        s.run(one).unwrap_or_else(|e| panic!("{one}: {e}"));
    }
    s.commit().unwrap();
}

/// The error `sql` fails with in `s`, rolled back.
fn refused(s: &Session, sql: &str) -> SqlError {
    let e = s.run(sql).err().unwrap_or_else(|| panic!("{sql} succeeded"));
    s.rollback().unwrap();
    e
}

/// Asserts `sql` fails in `s` with `code`, its message citing `cited`.
fn refused_by(s: &Session, sql: &str, code: Code, cited: &str) {
    let e = refused(s, sql);
    assert_eq!(e.code, code, "{sql}: {e}");
    assert!(e.to_string().contains(cited), "{sql}: {e} does not cite {cited}");
}

/// An order of `item` from `f` to `t`, days of 2026.
fn order(item: &str, f: &str, t: &str) -> String {
    format!("INSERT INTO ORDERS VALUES ({item}, DATE '2026-{f}', DATE '2026-{t}')")
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.25.3.3, §11.8 SR18a i, GR2] a row
/// whose period the periods of the rows of its item cover, in one row or
/// across rows that meet, is kept, and one of a null item (MATCH SIMPLE);
/// a row whose period runs past them, starts before them, is of an item
/// of another period or of none, is refused (23000); and a row's period
/// changed past them too.
#[test]
fn a_rows_period_is_covered_by_the_rows_it_references_11_8() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[&order("1", "01-15", "02-15"), &order("1", "01-01", "03-01"), &order("2", "01-02", "01-03"), &order("NULL", "05-01", "06-01")],
    );
    let early = "INSERT INTO ORDERS VALUES (1, DATE '2025-12-31', DATE '2026-01-10')".to_owned();
    for sql in [order("1", "01-15", "03-15"), early, order("2", "01-15", "02-15"), order("3", "01-01", "01-02")] {
        refused_by(&s, &sql, Code::Constraint, UNCOVERED);
    }
    refused_by(&s, "UPDATE ORDERS SET T = DATE '2026-03-02' WHERE ITEM = 1", Code::Constraint, UNCOVERED);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM ORDERS").unwrap(), ["4"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.8 GR2; §14.9; §15.8 GR9] NO ACTION:
/// a referenced row deleted, its period shortened, or its key changed, is
/// refused while it leaves a row's period not covered (23000), and kept
/// where no row needs it; DELETE FOR PORTION OF a time no row needs is
/// kept, of a time one needs refused.
#[test]
fn a_referenced_row_is_kept_while_a_row_needs_its_period_11_8() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[&order("1", "01-15", "02-15")]);
    refused_by(&s, "DELETE FROM PRICES WHERE ITEM = 1 AND PRICE = 11", Code::Constraint, "no longer there");
    refused_by(&s, "UPDATE PRICES SET E = DATE '2026-02-10' WHERE PRICE = 11", Code::Constraint, "no longer there");
    refused_by(&s, "UPDATE PRICES SET ITEM = 9 WHERE PRICE = 10", Code::Constraint, "no longer there");
    let needed = "DELETE FROM PRICES FOR PORTION OF VALID FROM DATE '2026-02-10' TO DATE '2026-02-12' WHERE ITEM = 1";
    refused_by(&s, needed, Code::Constraint, "no longer there");
    ok(
        &s,
        &[
            "UPDATE PRICES SET E = DATE '2026-02-20' WHERE PRICE = 11",
            "DELETE FROM PRICES FOR PORTION OF VALID FROM DATE '2026-01-01' TO DATE '2026-01-10' WHERE ITEM = 1",
            "DELETE FROM PRICES WHERE ITEM = 2",
        ],
    );
    let left = "SELECT ITEM, S, E FROM PRICES ORDER BY S";
    assert_eq!(rows(&s, left).unwrap(), ["1,2026-01-10,2026-02-01", "1,2026-02-01,2026-02-20"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §15.18 GR9a iv, GR10a iv] ON DELETE and
/// ON UPDATE RESTRICT of a temporal foreign key: a referenced row deleted,
/// or its key changed, while a row's values match it is refused (23001),
/// whatever its period; one of an item no row has is deleted.
#[test]
fn restrict_of_a_temporal_foreign_key_refuses_a_matching_row_15_18() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[
            "ALTER TABLE ORDERS DROP CONSTRAINT PRICED RESTRICT",
            "ALTER TABLE ORDERS ADD CONSTRAINT PRICED FOREIGN KEY (ITEM, PERIOD HELD) REFERENCES PRICES (ITEM, PERIOD VALID) \
             ON DELETE RESTRICT ON UPDATE RESTRICT",
            &order("1", "01-15", "01-20"),
        ],
    );
    let state = |sql: &str| s.run(sql).err().map(|e| (e.sqlstate().as_str().to_owned(), s.rollback().unwrap()).0);
    assert_eq!(state("DELETE FROM PRICES WHERE PRICE = 11").as_deref(), Some("23001"));
    assert_eq!(state("UPDATE PRICES SET ITEM = 9 WHERE PRICE = 10").as_deref(), Some("23001"));
    assert_eq!(state("DELETE FROM PRICES WHERE ITEM = 2"), None);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.8 SR2a, SR2b, SR18a i 4, 5, 6, 8] a
/// temporal foreign key writes a PERIOD of both sides, with the columns
/// referenced; no CASCADE, SET NULL or SET DEFAULT; each table has its
/// period; the columns referenced are those of a unique constraint WITHOUT
/// OVERLAPS of that period; and the periods are of one type.
#[test]
fn a_temporal_foreign_key_is_refused_against_its_rules_11_8() {
    let db = database();
    let s = db.session("A");
    let table = |key: &str| format!("CREATE TABLE T (ITEM INTEGER, F DATE NOT NULL, T DATE NOT NULL, PERIOD FOR HELD (F, T), {key})");
    let cases = [
        ("FOREIGN KEY (ITEM, PERIOD HELD) REFERENCES PRICES (ITEM)", "§11.8 SR2a"),
        ("FOREIGN KEY (ITEM) REFERENCES PRICES (ITEM, PERIOD VALID)", "§11.8 SR2a"),
        ("FOREIGN KEY (ITEM, PERIOD HELD) REFERENCES PRICES (ITEM, PERIOD VALID) ON DELETE CASCADE", "§11.8 SR2b"),
        ("FOREIGN KEY (ITEM, PERIOD HELD) REFERENCES PRICES (ITEM, PERIOD VALID) ON UPDATE SET NULL", "§11.8 SR2b"),
        ("FOREIGN KEY (ITEM, PERIOD HELD) REFERENCES PRICES (ITEM, PERIOD OTHER)", "§11.8 SR18a i 4"),
        ("FOREIGN KEY (ITEM, PERIOD OTHER) REFERENCES PRICES (ITEM, PERIOD VALID)", "§11.8 SR18a i 5"),
        ("FOREIGN KEY (ITEM, PERIOD HELD) REFERENCES PRICES (PRICE, PERIOD VALID)", "§11.8 SR18a i 6"),
    ];
    for (key, cited) in cases {
        refused_by(&s, &table(key), Code::SyntaxRule, cited);
    }
    let stamped = "CREATE TABLE T (ITEM INTEGER, F TIMESTAMP NOT NULL, T TIMESTAMP NOT NULL, PERIOD FOR HELD (F, T), \
                   FOREIGN KEY (ITEM, PERIOD HELD) REFERENCES PRICES (ITEM, PERIOD VALID))";
    refused_by(&s, stamped, Code::SyntaxRule, "§11.8 SR18a i 8");
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.25.3.3] of two referencing columns,
/// MATCH FULL refuses a row of one null (23000) and keeps one of both;
/// MATCH PARTIAL covers a row of one null by the periods of every row its
/// value not null matches.
#[test]
fn match_types_take_nulls_of_a_temporal_foreign_key_4_25() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[
            "CREATE TABLE SHELF (ROW_NO INTEGER, BAY INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR STOCKED (S, E), \
             UNIQUE (ROW_NO, BAY, STOCKED WITHOUT OVERLAPS))",
            "INSERT INTO SHELF VALUES (1, 1, DATE '2026-01-01', DATE '2026-02-01'), (1, 2, DATE '2026-02-01', DATE '2026-03-01')",
            "CREATE TABLE FULLY (ROW_NO INTEGER, BAY INTEGER, F DATE NOT NULL, T DATE NOT NULL, PERIOD FOR HELD (F, T), \
             FOREIGN KEY (ROW_NO, BAY, PERIOD HELD) REFERENCES SHELF (ROW_NO, BAY, PERIOD STOCKED) MATCH FULL)",
            "CREATE TABLE PARTLY (ROW_NO INTEGER, BAY INTEGER, F DATE NOT NULL, T DATE NOT NULL, PERIOD FOR HELD (F, T), \
             FOREIGN KEY (ROW_NO, BAY, PERIOD HELD) REFERENCES SHELF (ROW_NO, BAY, PERIOD STOCKED) MATCH PARTIAL)",
            "INSERT INTO FULLY VALUES (NULL, NULL, DATE '2026-05-01', DATE '2026-06-01')",
            "INSERT INTO PARTLY VALUES (1, NULL, DATE '2026-01-15', DATE '2026-02-15')",
        ],
    );
    refused_by(&s, "INSERT INTO FULLY VALUES (1, NULL, DATE '2026-01-02', DATE '2026-01-03')", Code::Constraint, UNCOVERED);
    refused_by(&s, "INSERT INTO PARTLY VALUES (1, NULL, DATE '2026-01-15', DATE '2026-03-15')", Code::Constraint, UNCOVERED);
    refused_by(&s, "DELETE FROM SHELF WHERE BAY = 2", Code::Constraint, "no longer there");
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.25.3.3] with an index method that
/// orders no keys behind unique constraints (TABLES.UNIQUE_INDEX_METHOD of
/// hash), the periods of a key's rows are gathered and sorted: the same
/// rows kept and refused.
#[test]
fn a_temporal_foreign_key_of_a_hash_index_gathers_the_periods_11_8() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db.set_setting("TABLES.UNIQUE_INDEX_METHOD", &Value::Char("hash".into())).unwrap();
    let s = db.session("A");
    ok(&s, &prices());
    ok(&s, &[&order("1", "01-15", "02-15")]);
    refused_by(&s, &order("1", "01-15", "03-15"), Code::Constraint, UNCOVERED);
    refused_by(&s, "DELETE FROM PRICES WHERE PRICE = 10", Code::Constraint, "no longer there");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.8; ISO/IEC 9075:1992 §10.6] a
/// temporal foreign key DEFERRABLE INITIALLY DEFERRED takes a row before
/// the rows that cover it, judged as the transaction commits.
#[test]
fn a_deferred_temporal_foreign_key_is_judged_at_commit_11_8() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[
            "CREATE TABLE LATER (ITEM INTEGER, F DATE NOT NULL, T DATE NOT NULL, PERIOD FOR HELD (F, T), \
             FOREIGN KEY (ITEM, PERIOD HELD) REFERENCES PRICES (ITEM, PERIOD VALID) DEFERRABLE INITIALLY DEFERRED)",
            "INSERT INTO LATER VALUES (3, DATE '2026-01-01', DATE '2026-01-05')",
            "INSERT INTO PRICES VALUES (3, 30, DATE '2026-01-01', DATE '2026-02-01')",
        ],
    );
    s.run("INSERT INTO LATER VALUES (4, DATE '2026-01-01', DATE '2026-01-05')").unwrap();
    assert_eq!(s.commit().err().map(|e| e.code), Some(Code::CommitFailed));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.28 SR3d ii, GR2f iii] DROP PERIOD
/// RESTRICT is refused while a temporal foreign key is of the period, of
/// the table that references or the one referenced; CASCADE drops it, and
/// the referenced table's key with it.
#[test]
fn drop_period_drops_a_temporal_foreign_key_only_with_cascade_11_28() {
    let db = database();
    let s = db.session("A");
    refused_by(&s, "ALTER TABLE ORDERS DROP PERIOD FOR HELD RESTRICT", Code::SyntaxRule, "§11.28 SR3d ii");
    refused_by(&s, "ALTER TABLE PRICES DROP PERIOD FOR VALID RESTRICT", Code::SyntaxRule, "§11.28 SR3d ii");
    ok(&s, &["ALTER TABLE ORDERS DROP PERIOD FOR HELD CASCADE", "INSERT INTO ORDERS VALUES (7, DATE '2026-01-01', DATE '2026-01-02')"]);
    ok(&s, &["ALTER TABLE PRICES DROP PERIOD FOR VALID CASCADE", "INSERT INTO PRICES VALUES (1, 0, DATE '2026-01-01', DATE '2026-01-02')"]);
    let left = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE CONSTRAINT_TYPE IN ('FOREIGN KEY', 'PRIMARY KEY')";
    assert_eq!(rows(&s, left).unwrap(), ["0"]);
}

/// Verifies: [DESIGN] RULES.md D25: a temporal foreign key, and RESTRICT,
/// are kept with the definitions, after stopping without a checkpoint, by
/// the logged definition, and after a checkpoint, by the snapshot.
#[test]
fn a_temporal_foreign_key_is_kept_across_reopening_11_8() {
    let scratch = Scratch::new("temporal-references-reopen");
    let dir = scratch.path("db");
    {
        let db = Database::open(&dir).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
        let s = db.session("A");
        ok(&s, &prices());
        ok(
            &s,
            &[
                "ALTER TABLE ORDERS DROP CONSTRAINT PRICED RESTRICT",
                "ALTER TABLE ORDERS ADD CONSTRAINT PRICED FOREIGN KEY (ITEM, PERIOD HELD) REFERENCES PRICES (ITEM, PERIOD VALID) \
                 ON DELETE RESTRICT",
                &order("1", "01-15", "02-15"),
            ],
        );
    }
    let check = |db: &Database| {
        let s = db.session("A");
        refused_by(&s, &order("1", "01-15", "03-15"), Code::Constraint, UNCOVERED);
        let e = refused(&s, "DELETE FROM PRICES WHERE PRICE = 11");
        assert_eq!(e.sqlstate().as_str(), "23001", "{e}");
    };
    {
        let db = Database::open(&dir).unwrap();
        check(&db);
        db.checkpoint().unwrap();
        db.close().unwrap();
    }
    let db = Database::open(&dir).unwrap();
    check(&db);
    db.close().unwrap();
}
