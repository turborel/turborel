//! ALTER COLUMN … SET NOT NULL and DROP NOT NULL [ISO/IEC 9075-2:2023
//! §11.15, §11.16, Feature F383] and SET DATA TYPE [§11.19, Feature F382]
//! (RULES.md D399 to D401): the rows checked or copied, in either engine,
//! the refusals each Syntax Rule makes, what depends on the column, and
//! the change undone by ROLLBACK, kept by a reopen and recovered after a
//! crash.

use turborel::db::{Database, Opened};
use turborel::error::Code;

use crate::common::{Scratch, alter_failed, committed_each, query, reopened_after_crash, schema_a};

/// The engines a table is made in.
const ENGINES: [&str; 2] = ["ROWSTORE", "COLUMNSTORE"];

/// Verifies: [ISO/IEC 9075-2:2023 §11.15 GR1, GR1b to GR1d] SET NOT NULL
/// of a column whose rows hold a null fails with SQLSTATE 23000 and
/// changes nothing; once none does, it gives the column a NOT NULL
/// constraint of a name of its own, which the Information Schema shows
/// and an INSERT of a null breaks; SET NOT NULL again leaves it so [GR1];
/// in both engines.
#[test]
fn set_not_null_checks_the_rows_15() {
    for engine in ENGINES {
        let db = schema_a();
        let s = db.session("A");
        committed_each(
            &s,
            &[&format!("CREATE TABLE T (K INTEGER, V VARCHAR(5)) USING {engine}"), "INSERT INTO T VALUES (1, 'a'), (2, NULL)"],
        );
        let e = alter_failed(&s, "ALTER TABLE T ALTER COLUMN V SET NOT NULL", "§6.3");
        assert_eq!((e.code, e.sqlstate().class()), (Code::Constraint, "23"), "{e}");
        assert_eq!(query(&s, "SELECT IS_NULLABLE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' AND COLUMN_NAME = 'V'"), ["YES"]);
        committed_each(
            &s,
            &["UPDATE T SET V = 'b' WHERE K = 2", "ALTER TABLE T ALTER V SET NOT NULL", "ALTER TABLE T ALTER V SET NOT NULL"],
        );
        assert_eq!(query(&s, "SELECT IS_NULLABLE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' AND COLUMN_NAME = 'V'"), ["NO"]);
        let checks = "SELECT CHECK_CLAUSE FROM INFORMATION_SCHEMA.CHECK_CONSTRAINTS WHERE CONSTRAINT_SCHEMA = 'A'";
        assert_eq!(query(&s, checks), ["V IS NOT NULL"], "{engine}: one constraint");
        let e = s.run("INSERT INTO T VALUES (3, NULL)").unwrap_err();
        assert_eq!(e.code, Code::Constraint, "{e}");
        s.rollback().unwrap();
        assert_eq!(query(&s, "SELECT K, V FROM T ORDER BY K"), ["1,a", "2,b"]);
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.15 GR1d] SET NOT NULL checks the
/// rows every snapshot sees once it commits: a null another transaction
/// committed before it is found, and one another transaction has written
/// but not committed holds the table, so the ALTER is refused until it
/// ends.
#[test]
fn set_not_null_sees_the_rows_of_other_transactions() {
    let db = schema_a();
    let (a, b) = (db.session("A"), db.session("A"));
    committed_each(&a, &["CREATE TABLE T (K INTEGER, V INTEGER)", "INSERT INTO T VALUES (1, 1)"]);
    assert_eq!(query(&a, "SELECT COUNT(*) FROM T"), ["1"]);
    committed_each(&b, &["INSERT INTO T VALUES (2, NULL)"]);
    alter_failed(&a, "ALTER TABLE T ALTER V SET NOT NULL", "NOT NULL");
    committed_each(&b, &["DELETE FROM T WHERE K = 2"]);
    b.run("INSERT INTO T VALUES (3, NULL)").unwrap();
    let e = a.run("ALTER TABLE T ALTER V SET NOT NULL").unwrap_err();
    a.rollback().unwrap();
    assert!(matches!(e.code, Code::Conflict | Code::Constraint), "{e}");
    b.rollback().unwrap();
    committed_each(&a, &["ALTER TABLE T ALTER V SET NOT NULL"]);
    assert_eq!(b.run("INSERT INTO T VALUES (4, NULL)").unwrap_err().code, Code::Constraint);
    b.rollback().unwrap();
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.16 GR1, SR2 to SR4; §11.12 SR2, SR3,
/// SR6] DROP NOT NULL drops the column's NOT NULL constraint, so a null
/// may be inserted, and of a column NULL already does nothing; it is
/// refused of a column of the PRIMARY KEY, a period's start, a
/// system-time period's column, an identity column, or no column; and
/// of any column of a system-versioned table.
#[test]
fn drop_not_null_and_its_refusals_16() {
    let db = schema_a();
    let s = db.session("A");
    committed_each(
        &s,
        &[
            "CREATE TABLE T (ID INTEGER PRIMARY KEY, V INTEGER NOT NULL, N INTEGER, \
             G INTEGER GENERATED ALWAYS AS IDENTITY, F DATE NOT NULL, U DATE NOT NULL, PERIOD FOR P (F, U))",
            "CREATE TABLE S (K INTEGER, B TIMESTAMP(6) GENERATED ALWAYS AS ROW START, \
             E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, PERIOD FOR SYSTEM_TIME (B, E))",
            "CREATE TABLE SV (K INTEGER, B TIMESTAMP(6) GENERATED ALWAYS AS ROW START, \
             E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, PERIOD FOR SYSTEM_TIME (B, E)) WITH SYSTEM VERSIONING",
        ],
    );
    committed_each(&s, &["ALTER TABLE T ALTER COLUMN V DROP NOT NULL", "ALTER TABLE T ALTER N DROP NOT NULL"]);
    committed_each(&s, &["INSERT INTO T (ID, V, F, U) VALUES (1, NULL, DATE '2026-01-01', DATE '2026-02-01')"]);
    assert_eq!(query(&s, "SELECT ID, V FROM T"), ["1,NULL"]);
    alter_failed(&s, "ALTER TABLE T ALTER ID DROP NOT NULL", "§11.16 SR4");
    alter_failed(&s, "ALTER TABLE T ALTER F DROP NOT NULL", "§11.16 SR3");
    alter_failed(&s, "ALTER TABLE S ALTER E DROP NOT NULL", "§11.16 SR2");
    alter_failed(&s, "ALTER TABLE SV ALTER K DROP NOT NULL", "§11.12 SR2");
    alter_failed(&s, "ALTER TABLE T ALTER G DROP NOT NULL", "§11.12 SR6");
    alter_failed(&s, "ALTER TABLE T ALTER G SET NOT NULL", "§11.12 SR6");
    alter_failed(&s, "ALTER TABLE T ALTER Z DROP NOT NULL", "§11.12 SR3");
    let e = alter_failed(&s, "ALTER TABLE T ALTER V DROP NULL", "expected");
    assert_eq!(e.code, Code::Syntax);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.16 GR1c] DROP NOT NULL drops the
/// constraint as DROP CONSTRAINT … CASCADE does: a view whose grouping
/// depends on the key the NOT NULL makes of a UNIQUE goes with it.
#[test]
fn drop_not_null_cascades_to_what_depends_on_it() {
    let db = schema_a();
    let s = db.session("A");
    committed_each(
        &s,
        &["CREATE TABLE T (K INTEGER NOT NULL, N INTEGER, CONSTRAINT U_K UNIQUE (K))", "CREATE VIEW V AS SELECT K, N FROM T GROUP BY K"],
    );
    committed_each(&s, &["ALTER TABLE T ALTER K DROP NOT NULL"]);
    assert_eq!(crate::common::rows(&s, "SELECT * FROM V"), Err(Code::SyntaxRule));
    s.rollback().unwrap();
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.19 GR1, SR15a, SR15c] SET DATA TYPE
/// widens a column in either engine: its rows copied, each value assigned
/// to the new type, a fixed-length string padded to its new length; its
/// index made again, finding the rows and keeping the column unique; a
/// CHECK of the column alone bound again; and the Information Schema
/// shows the new type. A varying string given a fixed length is padded
/// to it, and keeps the padding given a varying length again.
#[test]
fn set_data_type_widens_a_column_19() {
    for engine in ENGINES {
        let db = schema_a();
        let s = db.session("A");
        committed_each(
            &s,
            &[
                &format!("CREATE TABLE T (K INTEGER UNIQUE, C CHAR(3), V VARCHAR(4) CHECK (V <> 'bad'), D DECIMAL(5,2)) USING {engine}"),
                "INSERT INTO T VALUES (1, 'ab', 'four', 123.45), (2, 'cd', 'x', -1.5)",
            ],
        );
        committed_each(&s, &["CREATE INDEX T_V ON T (V)"]);
        committed_each(
            &s,
            &[
                "ALTER TABLE T ALTER COLUMN K SET DATA TYPE BIGINT",
                "ALTER TABLE T ALTER C SET DATA TYPE CHAR(5)",
                "ALTER TABLE T ALTER V SET DATA TYPE VARCHAR(10)",
                "ALTER TABLE T ALTER D SET DATA TYPE DECIMAL(8,3)",
            ],
        );
        assert_eq!(query(&s, "SELECT K, '<' || C || '>', V, D FROM T ORDER BY K"), ["1,<ab   >,four,123.450", "2,<cd   >,x,-1.500"]);
        committed_each(&s, &["ALTER TABLE T ALTER V SET DATA TYPE CHAR(12)"]);
        assert_eq!(query(&s, "SELECT CHAR_LENGTH(V) FROM T ORDER BY K"), ["12", "12"], "{engine}: each value assigned, so padded");
        committed_each(&s, &["ALTER TABLE T ALTER V SET DATA TYPE VARCHAR(12)"]);
        committed_each(&s, &["INSERT INTO T VALUES (9000000000, 'abcde', 'ten chars!', 12345.678)"]);
        assert_eq!(query(&s, "SELECT K FROM T WHERE V = 'ten chars!'"), ["9000000000"]);
        assert_eq!(s.run("INSERT INTO T VALUES (1, 'z', 'z', 0)").unwrap_err().code, Code::Constraint, "{engine}: K stays unique");
        s.rollback().unwrap();
        assert_eq!(s.run("INSERT INTO T VALUES (5, 'z', 'bad', 0)").unwrap_err().code, Code::Constraint, "{engine}: the CHECK holds");
        s.rollback().unwrap();
        let types = "SELECT DATA_TYPE, CHARACTER_MAXIMUM_LENGTH, NUMERIC_PRECISION, NUMERIC_SCALE FROM INFORMATION_SCHEMA.COLUMNS \
                     WHERE TABLE_NAME = 'T' ORDER BY ORDINAL_POSITION";
        let shown = query(&s, types);
        assert_eq!(shown.get(1).map(String::as_str), Some("CHARACTER,5,NULL,NULL"), "{shown:?}");
        assert_eq!(shown.get(3).map(String::as_str), Some("DECIMAL,NULL,8,3"), "{shown:?}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.19 SR13 to SR15; §11.12 SR3, SR6]
/// SET DATA TYPE is refused of a type shorter or less precise, of another
/// kind, or not predefined; of an identity column, a period's column, a
/// column of a domain (\[DESIGN\]) or no column; and of a name that is no
/// type.
#[test]
fn set_data_type_refuses_what_narrows_19() {
    let db = schema_a();
    let s = db.session("A");
    committed_each(
        &s,
        &[
            "CREATE DOMAIN POSITIVE AS INTEGER CHECK (VALUE > 0)",
            "CREATE TABLE T (K INTEGER GENERATED ALWAYS AS IDENTITY, V VARCHAR(5), D DECIMAL(5,2), M POSITIVE, \
             F DATE NOT NULL, U DATE NOT NULL, B BOOLEAN, PERIOD FOR P (F, U))",
        ],
    );
    alter_failed(&s, "ALTER TABLE T ALTER V SET DATA TYPE VARCHAR(4)", "§11.19 SR15a");
    alter_failed(&s, "ALTER TABLE T ALTER V SET DATA TYPE INTEGER", "§11.19 SR15a");
    alter_failed(&s, "ALTER TABLE T ALTER D SET DATA TYPE DECIMAL(5,3)", "§11.19 SR15c");
    alter_failed(&s, "ALTER TABLE T ALTER D SET DATA TYPE DOUBLE PRECISION", "§11.19 SR15c");
    alter_failed(&s, "ALTER TABLE T ALTER B SET DATA TYPE CHAR(5)", "§11.19 SR15j");
    alter_failed(&s, "ALTER TABLE T ALTER V SET DATA TYPE VARCHAR(9) ARRAY[3]", "§11.19 SR13");
    alter_failed(&s, "ALTER TABLE T ALTER V SET DATA TYPE POSITIVE", "§11.19 SR13");
    alter_failed(&s, "ALTER TABLE T ALTER K SET DATA TYPE BIGINT", "§11.12 SR6");
    alter_failed(&s, "ALTER TABLE T ALTER F SET DATA TYPE TIMESTAMP", "§11.19 SR5");
    alter_failed(&s, "ALTER TABLE T ALTER M SET DATA TYPE BIGINT", "domain");
    alter_failed(&s, "ALTER TABLE T ALTER Z SET DATA TYPE BIGINT", "§11.12 SR3");
    committed_each(&s, &["ALTER TABLE T ALTER V SET DATA TYPE VARCHAR(5)"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.19 SR6 to SR10] SET DATA TYPE is
/// refused of a column a view's query, a CHECK that reads another column
/// too, an assertion, a trigger's column list or action, a routine's body
/// or a generation expression references; a view that reads other columns
/// of the table alone keeps it allowed, and still reads them.
#[test]
fn set_data_type_refuses_what_references_the_column() {
    let db = schema_a();
    let s = db.session("A");
    committed_each(
        &s,
        &[
            "CREATE TABLE T (A INTEGER, B INTEGER, C INTEGER, D INTEGER, E INTEGER, F INTEGER, G INTEGER GENERATED ALWAYS AS (F + 1), \
             H INTEGER, CHECK (A < B))",
            "CREATE TABLE LOG (N INTEGER)",
            "CREATE VIEW V AS SELECT C, H FROM T",
            "CREATE ASSERTION SMALL CHECK (NOT EXISTS (SELECT * FROM T WHERE D > 100))",
            "CREATE TRIGGER TR AFTER UPDATE OF E ON T FOR EACH ROW INSERT INTO LOG VALUES (1)",
            "CREATE FUNCTION TOP_H () RETURNS INTEGER READS SQL DATA RETURN (SELECT MAX(H) FROM T)",
        ],
    );
    alter_failed(&s, "ALTER TABLE T ALTER A SET DATA TYPE BIGINT", "§11.19 SR6");
    alter_failed(&s, "ALTER TABLE T ALTER D SET DATA TYPE BIGINT", "§11.19 SR6");
    alter_failed(&s, "ALTER TABLE T ALTER E SET DATA TYPE BIGINT", "§11.19 SR7");
    alter_failed(&s, "ALTER TABLE T ALTER C SET DATA TYPE BIGINT", "§11.19 SR9");
    alter_failed(&s, "ALTER TABLE T ALTER F SET DATA TYPE BIGINT", "§11.19 SR10");
    committed_each(&s, &["DROP VIEW V CASCADE"]);
    alter_failed(&s, "ALTER TABLE T ALTER H SET DATA TYPE BIGINT", "§11.19 SR8");
    committed_each(
        &s,
        &["CREATE VIEW W AS SELECT B FROM T", "INSERT INTO T (A, B, F) VALUES (1, 2, 3)", "ALTER TABLE T ALTER G SET DATA TYPE BIGINT"],
    );
    assert_eq!(query(&s, "SELECT B FROM W"), ["2"]);
    assert_eq!(query(&s, "SELECT G FROM T"), ["4"]);
}

/// Verifies: [ISO/IEC 9075:1992 §4.28; ISO/IEC 9075-2:2023 §11.15,
/// §11.16, §11.19] Each form within a transaction is undone by ROLLBACK:
/// the column's type and nullability and the rows are as they were.
#[test]
fn each_form_is_undone_by_rollback() {
    for engine in ENGINES {
        let db = schema_a();
        let s = db.session("A");
        committed_each(
            &s,
            &[&format!("CREATE TABLE T (K INTEGER NOT NULL, V VARCHAR(3)) USING {engine}"), "INSERT INTO T VALUES (1, 'abc')"],
        );
        for sql in
            ["ALTER TABLE T ALTER V SET DATA TYPE VARCHAR(9)", "ALTER TABLE T ALTER V SET NOT NULL", "ALTER TABLE T ALTER K DROP NOT NULL"]
        {
            s.run(sql).unwrap();
        }
        s.run("INSERT INTO T VALUES (NULL, 'abcdefg')").unwrap();
        s.rollback().unwrap();
        let shown =
            "SELECT IS_NULLABLE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' ORDER BY ORDINAL_POSITION";
        assert_eq!(query(&s, shown), ["NO,NULL", "YES,3"], "{engine}");
        assert_eq!(query(&s, "SELECT K, V FROM T"), ["1,abc"]);
    }
}

/// The state each form leaves, read in `db`.
fn altered_state(db: &Database) -> Vec<String> {
    let s = db.session("A");
    let mut state = query(
        &s,
        "SELECT IS_NULLABLE, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' ORDER BY ORDINAL_POSITION",
    );
    state.extend(query(&s, "SELECT K, V FROM T ORDER BY K"));
    state.extend(query(&s, "SELECT K FROM T WHERE V = 'two'"));
    state
}

/// What `altered_state` reads of the forms done.
const ALTERED: [&str; 5] = ["YES,INTEGER,NULL", "NO,CHARACTER VARYING,12", "1,one", "2,two", "2"];

/// Runs each form on table T of schema A of `db`, made in `engine`.
fn alter_each(db: &Database, engine: &str) {
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    committed_each(&s, &[&format!("CREATE TABLE T (K INTEGER NOT NULL, V VARCHAR(5)) USING {engine}")]);
    committed_each(&s, &["CREATE INDEX T_V ON T (V)"]);
    committed_each(&s, &["INSERT INTO T VALUES (1, 'one'), (2, 'two')"]);
    committed_each(
        &s,
        &["ALTER TABLE T ALTER V SET DATA TYPE VARCHAR(12)", "ALTER TABLE T ALTER V SET NOT NULL", "ALTER TABLE T ALTER K DROP NOT NULL"],
    );
}

/// Verifies: [DESIGN] RULES.md D25, D33, D399 to D401: the forms, logged
/// as the ALTER TABLE records they are, are kept by a database reopened,
/// from its log and from a checkpoint's image, in either engine.
#[test]
fn the_forms_are_kept_by_a_reopen() {
    for engine in ENGINES {
        let scratch = Scratch::new("alter-columns-reopen");
        let dir = scratch.path("db");
        {
            let db = Database::open(&dir).unwrap();
            alter_each(&db, engine);
            assert_eq!(altered_state(&db), ALTERED);
        }
        let db = Database::open(&dir).unwrap();
        assert_eq!(altered_state(&db), ALTERED, "{engine}: from the log");
        db.checkpoint().unwrap();
        drop(db);
        let db = Database::open(&dir).unwrap();
        assert_eq!(altered_state(&db), ALTERED, "{engine}: from the image");
    }
}

/// Verifies: [DESIGN] RULES.md D25, D401: after a crash, the forms
/// committed are recovered, the rows SET DATA TYPE copied among them; one
/// whose transaction was open at the crash left the table as it was.
#[test]
fn the_forms_are_recovered_after_a_crash() {
    for engine in ENGINES {
        let scratch = Scratch::new("alter-columns-crash");
        let dir = scratch.path("db");
        let (crashed, _) = reopened_after_crash(&dir);
        alter_each(&crashed, engine);
        let open = crashed.session("A");
        open.run("ALTER TABLE T ALTER V SET DATA TYPE VARCHAR(20)").unwrap();
        for _ in 0..2 {
            let (db, opened) = reopened_after_crash(&dir);
            assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
            assert_eq!(altered_state(&db), ALTERED, "{engine}");
        }
        drop((open, crashed));
    }
}
