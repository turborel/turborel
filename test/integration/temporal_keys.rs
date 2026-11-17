//! UNIQUE and PRIMARY KEY WITHOUT OVERLAPS of an application-time period
//! [ISO/IEC 9075-2:2023 §4.25.3.2, §11.7]: two rows of equal values in
//! the constraint's columns are a duplicate only if their periods overlap.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError};
use turborel::value::Value;

use crate::common::{Scratch, rows};

/// The text that cites an overlap.
const OVERLAP: &str = "§11.7 SR6d i";

/// Schema A and its table PRICES of the period VALID, whose PRIMARY KEY is
/// ITEM WITHOUT OVERLAPS of it, of item 1 priced 10 through January 2026.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    ok(
        &db.session("A"),
        &[
            "CREATE TABLE PRICES (ITEM INTEGER, PRICE INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR VALID (S, E), \
             PRIMARY KEY (ITEM, VALID WITHOUT OVERLAPS))",
            "INSERT INTO PRICES VALUES (1, 10, DATE '2026-01-01', DATE '2026-02-01')",
        ],
    );
    db
}

/// Runs each of `sql` in `s`, which must succeed, and commits.
fn ok(s: &Session, sql: &[&str]) {
    for one in sql {
        s.run(one).unwrap_or_else(|e| panic!("{one}: {e}"));
    }
    s.commit().unwrap();
}

/// The rows of PRICES, in order of item and start.
fn prices(s: &Session) -> Vec<String> {
    rows(s, "SELECT ITEM, PRICE, S, E FROM PRICES ORDER BY ITEM, S").unwrap_or_else(|c| panic!("{c:?}"))
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

/// Verifies: [ISO/IEC 9075-2:2023 §4.25.3.2, §11.7 SR6d i] rows of one
/// key whose periods meet or are apart are kept, of another key whatever
/// their periods; a row whose period overlaps, contains, is within or
/// equals another's of its key is refused (SQLSTATE 23000); an UPDATE
/// moving a period or a key onto another's is refused too.
#[test]
fn rows_of_a_key_are_refused_only_if_their_periods_overlap_11_7() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[
            "INSERT INTO PRICES VALUES (1, 11, DATE '2026-02-01', DATE '2026-03-01')",
            "INSERT INTO PRICES VALUES (1, 12, DATE '2026-04-01', DATE '2026-05-01')",
            "INSERT INTO PRICES VALUES (2, 20, DATE '2026-01-15', DATE '2026-02-15')",
        ],
    );
    for (s_at, e_at) in
        [("2025-12-01", "2026-01-02"), ("2026-01-10", "2026-01-20"), ("2025-01-01", "2027-01-01"), ("2026-01-01", "2026-02-01")]
    {
        let sql = format!("INSERT INTO PRICES VALUES (1, 0, DATE '{s_at}', DATE '{e_at}')");
        refused_by(&s, &sql, Code::Constraint, OVERLAP);
    }
    refused_by(&s, "UPDATE PRICES SET E = DATE '2026-04-02' WHERE PRICE = 11", Code::Constraint, OVERLAP);
    refused_by(&s, "UPDATE PRICES SET S = DATE '2026-01-31' WHERE PRICE = 11", Code::Constraint, OVERLAP);
    refused_by(&s, "UPDATE PRICES SET ITEM = 1 WHERE ITEM = 2", Code::Constraint, OVERLAP);
    ok(&s, &["UPDATE PRICES SET E = DATE '2026-04-01' WHERE PRICE = 11"]);
    assert_eq!(
        prices(&s),
        ["1,10,2026-01-01,2026-02-01", "1,11,2026-02-01,2026-04-01", "1,12,2026-04-01,2026-05-01", "2,20,2026-01-15,2026-02-15"]
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.7 SR6d i; §14.14 SR7a vi; §15.14
/// GR12] UPDATE FOR PORTION OF splits a row of a key WITHOUT OVERLAPS
/// into parts that do not overlap, which the constraint keeps; a row
/// whose overlap with another the statement leaves, written in one
/// statement, is refused as the statement ends.
#[test]
fn a_portion_splits_a_row_into_parts_that_do_not_overlap_11_7() {
    let db = database();
    let s = db.session("A");
    ok(&s, &["UPDATE PRICES FOR PORTION OF VALID FROM DATE '2026-01-10' TO DATE '2026-01-20' SET PRICE = 15"]);
    assert_eq!(prices(&s), ["1,10,2026-01-01,2026-01-10", "1,15,2026-01-10,2026-01-20", "1,10,2026-01-20,2026-02-01"]);
    // Two rows of the key inserted at once, overlapping each other.
    let both = "INSERT INTO PRICES VALUES (3, 0, DATE '2026-01-01', DATE '2026-01-10'), (3, 0, DATE '2026-01-05', DATE '2026-01-06')";
    refused_by(&s, both, Code::Constraint, OVERLAP);
    // One shifted past the other within one statement.
    ok(&s, &["UPDATE PRICES SET S = S + INTERVAL '1' YEAR, E = E + INTERVAL '1' YEAR"]);
    assert_eq!(prices(&s), ["1,10,2027-01-01,2027-01-10", "1,15,2027-01-10,2027-01-20", "1,10,2027-01-20,2027-02-01"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.25.3.2, §11.7 SR5, SR6d i] of a
/// UNIQUE WITHOUT OVERLAPS, NULLS DISTINCT, a row with a null is a
/// duplicate of none; of NULLS NOT DISTINCT, a null equals a null, so
/// rows of nulls whose periods overlap are refused, and kept apart.
#[test]
fn a_null_is_distinct_as_the_unique_null_treatment_says_11_7() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[
            "CREATE TABLE ROOMS (ROOM INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR BOOKED (S, E), \
             UNIQUE (ROOM, BOOKED WITHOUT OVERLAPS))",
            "CREATE TABLE DESKS (DESK INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR BOOKED (S, E), \
             UNIQUE NULLS NOT DISTINCT (DESK, BOOKED WITHOUT OVERLAPS))",
            "INSERT INTO ROOMS VALUES (NULL, DATE '2026-01-01', DATE '2026-02-01'), (NULL, DATE '2026-01-01', DATE '2026-02-01')",
            "INSERT INTO DESKS VALUES (NULL, DATE '2026-01-01', DATE '2026-02-01'), (NULL, DATE '2026-02-01', DATE '2026-03-01')",
        ],
    );
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM ROOMS").unwrap(), ["2"]);
    let sql = "INSERT INTO DESKS VALUES (NULL, DATE '2026-01-15', DATE '2026-02-15')";
    refused_by(&s, sql, Code::Constraint, OVERLAP);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.7 SR6d i; ISO/IEC 9075:1992
/// §10.6] a DEFERRABLE INITIALLY DEFERRED key WITHOUT OVERLAPS lets rows
/// overlap within the transaction, judged as it commits: kept once moved
/// apart, refused while they overlap (SQLSTATE 40002).
#[test]
fn a_deferred_key_is_judged_as_the_transaction_commits_11_7() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[
            "CREATE TABLE ROOMS (ROOM INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR BOOKED (S, E), \
             UNIQUE (ROOM, BOOKED WITHOUT OVERLAPS) DEFERRABLE INITIALLY DEFERRED)",
            "INSERT INTO ROOMS VALUES (1, DATE '2026-01-01', DATE '2026-02-01')",
            "INSERT INTO ROOMS VALUES (1, DATE '2026-01-15', DATE '2026-02-15')",
            "UPDATE ROOMS SET S = DATE '2026-02-01' WHERE E = DATE '2026-02-15'",
        ],
    );
    s.run("INSERT INTO ROOMS VALUES (1, DATE '2026-01-20', DATE '2026-01-25')").unwrap();
    let e = s.commit().err().unwrap_or_else(|| panic!("the commit succeeded"));
    assert_eq!(e.code, Code::CommitFailed, "{e}");
    assert!(e.to_string().contains(OVERLAP), "{e}");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM ROOMS").unwrap(), ["2"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.7 SR6d i; ISO/IEC 9075:1992
/// §11.16] ALTER TABLE ADD CONSTRAINT of a key WITHOUT OVERLAPS judges
/// the rows there: refused while two of a key overlap, kept once apart;
/// DROP CONSTRAINT then lets them overlap again.
#[test]
fn a_key_added_judges_the_rows_there_11_7() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[
            "CREATE TABLE ROOMS (ROOM INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR BOOKED (S, E))",
            "INSERT INTO ROOMS VALUES (1, DATE '2026-01-01', DATE '2026-02-01'), (1, DATE '2026-01-15', DATE '2026-02-15')",
        ],
    );
    let add = "ALTER TABLE ROOMS ADD CONSTRAINT FREE UNIQUE (ROOM, BOOKED WITHOUT OVERLAPS)";
    refused_by(&s, add, Code::Constraint, OVERLAP);
    ok(&s, &["UPDATE ROOMS SET S = DATE '2026-02-01' WHERE E = DATE '2026-02-15'", add]);
    refused_by(&s, "INSERT INTO ROOMS VALUES (1, DATE '2026-01-20', DATE '2026-01-25')", Code::Constraint, OVERLAP);
    ok(&s, &["ALTER TABLE ROOMS DROP CONSTRAINT FREE RESTRICT", "INSERT INTO ROOMS VALUES (1, DATE '2026-01-20', DATE '2026-01-25')"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.7 SR6d i] with an index method that
/// finds no rows by their first columns behind unique constraints
/// (TABLES.UNIQUE_INDEX_METHOD of hash), the table is read whole for rows
/// that overlap: the same rows kept, of a key or of other keys, and
/// refused.
#[test]
fn a_key_of_a_hash_index_reads_the_table_for_an_overlap_11_7() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db.set_setting("TABLES.UNIQUE_INDEX_METHOD", &Value::Char("hash".into())).unwrap();
    let s = db.session("A");
    ok(
        &s,
        &[
            "CREATE TABLE ROOMS (ROOM INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR BOOKED (S, E), \
             PRIMARY KEY (ROOM, BOOKED WITHOUT OVERLAPS))",
            "INSERT INTO ROOMS VALUES (1, DATE '2026-01-01', DATE '2026-02-01'), (1, DATE '2026-02-01', DATE '2026-03-01')",
            "INSERT INTO ROOMS VALUES (2, DATE '2026-01-01', DATE '2026-02-01')",
            "INSERT INTO ROOMS VALUES (3, DATE '2026-01-15', DATE '2026-02-15')",
        ],
    );
    refused_by(&s, "INSERT INTO ROOMS VALUES (1, DATE '2026-01-31', DATE '2026-02-02')", Code::Constraint, OVERLAP);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM ROOMS").unwrap(), ["4"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.7 SR3b, SR6c; §11.8 SR4a, SR4b] a
/// key WITHOUT OVERLAPS of a period the table has not is refused, by
/// CREATE TABLE and ALTER TABLE; WITHOUT OVERLAPS needs a column before
/// it; no two unique constraints are of the same columns, whatever their
/// period; a foreign key references a PRIMARY KEY WITHOUT OVERLAPS by no
/// column list, nor its columns alone.
#[test]
fn a_key_without_overlaps_is_refused_against_its_rules_11_7() {
    let db = database();
    let s = db.session("A");
    let none = "CREATE TABLE ROOMS (ROOM INTEGER, S DATE NOT NULL, E DATE NOT NULL, UNIQUE (ROOM, BOOKED WITHOUT OVERLAPS))";
    refused_by(&s, none, Code::SyntaxRule, "§11.7 SR3b");
    refused_by(&s, "ALTER TABLE PRICES ADD UNIQUE (PRICE, OTHER WITHOUT OVERLAPS)", Code::SyntaxRule, "§11.7 SR3b");
    let alone = "CREATE TABLE ROOMS (S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR BOOKED (S, E), UNIQUE (BOOKED WITHOUT OVERLAPS))";
    assert_eq!(refused(&s, alone).code, Code::Syntax);
    refused_by(&s, "ALTER TABLE PRICES ADD UNIQUE (ITEM)", Code::SyntaxRule, "§11.7 SR7");
    let implicit = "CREATE TABLE SALES (ITEM INTEGER REFERENCES PRICES)";
    refused_by(&s, implicit, Code::SyntaxRule, "§11.8 SR4b");
    let columns = "CREATE TABLE SALES (ITEM INTEGER REFERENCES PRICES (ITEM))";
    refused_by(&s, columns, Code::SyntaxRule, "§11.8 SR2a");
    let key = "CREATE TABLE SALES (ITEM INTEGER, S DATE, E DATE, FOREIGN KEY (ITEM, S, E) REFERENCES PRICES (ITEM, S, E))";
    refused_by(&s, key, Code::SyntaxRule, "§11.8 SR2a");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.28 SR3d ii, GR2f iii] DROP PERIOD
/// RESTRICT is refused while a key is WITHOUT OVERLAPS of the period;
/// CASCADE drops the key with it, its rows then free to overlap.
#[test]
fn drop_period_drops_a_key_without_overlaps_of_it_only_with_cascade_11_28() {
    let db = database();
    let s = db.session("A");
    refused_by(&s, "ALTER TABLE PRICES DROP PERIOD FOR VALID RESTRICT", Code::SyntaxRule, "§11.28 SR3d ii");
    ok(
        &s,
        &[
            "ALTER TABLE PRICES DROP PERIOD FOR VALID CASCADE",
            "INSERT INTO PRICES VALUES (1, 11, DATE '2026-01-15', DATE '2026-02-15')",
            "INSERT INTO PRICES VALUES (1, 12, DATE '2026-01-01', DATE '2026-02-01')",
        ],
    );
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM PRICES").unwrap(), ["3"]);
}

/// Verifies: [DESIGN] RULES.md D25: a key WITHOUT OVERLAPS is kept with
/// the definitions, its period with it, after stopping without a
/// checkpoint, by its logged definition, and after a checkpoint, by the
/// snapshot: still refusing a row that overlaps.
#[test]
fn a_key_without_overlaps_is_kept_across_reopening_11_7() {
    let scratch = Scratch::new("temporal-keys-reopen");
    let dir = scratch.path("db");
    {
        let db = Database::open(&dir).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
        ok(
            &db.session("A"),
            &[
                "CREATE TABLE ROOMS (ROOM INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR BOOKED (S, E), \
                 UNIQUE NULLS NOT DISTINCT (ROOM, BOOKED WITHOUT OVERLAPS))",
                "INSERT INTO ROOMS VALUES (1, DATE '2026-01-01', DATE '2026-02-01')",
            ],
        );
    }
    let check = |db: &Database| {
        let s = db.session("A");
        refused_by(&s, "INSERT INTO ROOMS VALUES (1, DATE '2026-01-15', DATE '2026-02-15')", Code::Constraint, OVERLAP);
        s.run("INSERT INTO ROOMS VALUES (1, DATE '2026-02-01', DATE '2026-02-15')").unwrap();
        s.rollback().unwrap();
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
