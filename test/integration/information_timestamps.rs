//! The creation and last-altered timestamps of Feature T011 [ISO/IEC
//! 9075-11:2006 draft §5.6, §6.30 Description 13, 14, §6.40 Description
//! 24, 25, §6.58 Description 13; ISO/IEC 9075-4:2006 draft §19.4
//! Description 7, 8]: each the CURRENT_TIMESTAMP of the statement that
//! made what it describes, kept in its descriptor, logged with its
//! definition, and so the same after a checkpoint, a reopening, a crash
//! and a standby's replay, whatever the clock reads then.

use std::path::Path;
use std::sync::Arc;

use turborel::clock::{Clock, FixedClock};
use turborel::db::{Database, OpenOptions, Standby};
use turborel::log::{Log, Lsn, MemoryLog};

use crate::common::{Scratch, after_crash, rows};

/// 2024-02-29 12:34:56.78 UTC, in microseconds from 1970-01-01.
const T0: i64 = 1_709_210_096_780_000;
/// A minute later.
const T1: i64 = T0 + 60_000_000;
/// Two minutes later.
const T2: i64 = T0 + 120_000_000;
/// A day later: what a clock reads as a database is opened again, or a
/// standby replays, which no timestamp takes.
const LATER: i64 = T0 + 86_400_000_000;

/// The routines, method specifications, triggers and modules of `db`,
/// with their timestamps, as user A sees them.
fn stamps(db: &Database) -> Vec<String> {
    let a = db.session("A");
    let queries = [
        "SELECT ROUTINE_SCHEMA, ROUTINE_NAME, ROUTINE_TYPE, CREATED, LAST_ALTERED FROM INFORMATION_SCHEMA.ROUTINES ORDER BY 1, 2, 3",
        "SELECT METHOD_NAME, CREATED, LAST_ALTERED FROM INFORMATION_SCHEMA.METHOD_SPECIFICATIONS ORDER BY 1, 2",
        "SELECT TRIGGER_NAME, CREATED FROM INFORMATION_SCHEMA.TRIGGERS",
        "SELECT MODULE_NAME, MODULE_CREATED, MODULE_LAST_ALTERED FROM INFORMATION_SCHEMA.MODULES",
    ];
    let shown = queries.iter().flat_map(|q| rows(&a, q).unwrap()).collect();
    a.commit().unwrap();
    shown
}

/// Defines, as `clock` moves: at T0, by the operator, schema A of table T
/// and function G; at T1, by A in the displacement +05:30, function F,
/// procedure P, structured type PERSON of a method specification,
/// distinct type MONEY, trigger TR and module SHOP; at T2, PERSON's
/// method's body, its attribute AGE, and schema LIB of function H.
fn define(db: &Database, clock: &FixedClock) {
    clock.set(T0);
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER) CREATE FUNCTION G (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X",
    )
    .unwrap();
    clock.set(T1);
    let a = db.session("A");
    a.run("SET SESSION.TIME_ZONE = '+05:30'").unwrap();
    for sql in [
        "CREATE FUNCTION F (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X + 1",
        "CREATE PROCEDURE P () MODIFIES SQL DATA INSERT INTO T VALUES (1)",
        "CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20)) NOT FINAL METHOD GREETING () RETURNS CHARACTER VARYING(30) CONTAINS SQL",
        "CREATE TYPE MONEY AS DECIMAL(9, 2) FINAL",
        "CREATE TRIGGER TR AFTER INSERT ON T FOR EACH ROW DELETE FROM T WHERE K < 0",
        "CREATE MODULE SHOP FUNCTION COUNTED () RETURNS INTEGER READS SQL DATA RETURN (SELECT COUNT(K) FROM T); END MODULE",
    ] {
        a.run(sql).unwrap();
    }
    a.commit().unwrap();
    clock.set(T2);
    a.run("CREATE METHOD GREETING () RETURNS CHARACTER VARYING(30) FOR PERSON RETURN SELF.NAME").unwrap();
    a.run("ALTER TYPE PERSON ADD ATTRIBUTE AGE INTEGER").unwrap();
    a.direct("CREATE SCHEMA LIB AUTHORIZATION A CREATE FUNCTION H (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X").unwrap();
    a.commit().unwrap();
}

/// A database whose clock stands at `at`, and the clock.
fn clocked(db: Database, at: i64) -> (Database, Arc<FixedClock>) {
    let clock = Arc::new(FixedClock::at(at));
    db.set_clock(Arc::clone(&clock) as Arc<dyn Clock>).unwrap();
    (db, clock)
}

/// The database in directory `dir`, opened as after a crash, its clock
/// standing at `at`.
fn opened(dir: &Path, at: i64) -> (Database, Arc<FixedClock>) {
    let (db, clock) = clocked(Database::new(), at);
    db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    (db, clock)
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.40 Description 24, 25, §6.30
/// Description 13, 14, §6.58 Description 13, §5.6; ISO/IEC 9075-4:2006
/// draft §19.4 Description 7, 8; ISO/IEC 9075-2:2023 §11.60 GR4, GR5]
/// (Feature T011): CREATED is the CURRENT_TIMESTAMP of the statement that
/// made the routine, method specification, trigger or module, in its
/// session's displacement, of TIME_STAMP's two digits of the second's
/// fraction, and equal to CURRENT_TIMESTAMP in its own transaction;
/// LAST_ALTERED is CREATED, as nothing has altered them. A routine a
/// type's definition makes, or an attribute added to it, is of that
/// statement's; a method's body, of CREATE METHOD's, while its
/// specification keeps the type's; a schema's routines, of its
/// definition's, the operator's in the database's displacement. The
/// short name views show the same.
#[test]
fn created_is_the_current_timestamp_of_the_definition_t011() {
    let (db, clock) = clocked(Database::new(), T0);
    clock.set(T1);
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE U (K INTEGER)").unwrap();
    let b = db.session("B");
    b.run("CREATE FUNCTION E () RETURNS INTEGER CONTAINS SQL RETURN 1").unwrap();
    let same = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.ROUTINES \
                WHERE ROUTINE_NAME = 'E' AND CREATED = CURRENT_TIMESTAMP AND LAST_ALTERED = CREATED";
    assert_eq!(rows(&b, same).unwrap(), ["1"], "in its own transaction");
    b.commit().unwrap();

    define(&db, &clock);
    let (t0, t1, t2) = ("2024-02-29 12:34:56.78+00:00", "2024-02-29 18:05:56.78+05:30", "2024-02-29 18:06:56.78+05:30");
    let routines = [
        ("A,AGE,INSTANCE METHOD", t2),
        ("A,AGE,INSTANCE METHOD", t2),
        ("A,COUNTED,FUNCTION", t1),
        ("A,F,FUNCTION", t1),
        ("A,G,FUNCTION", t0),
        ("A,GREETING,INSTANCE METHOD", t2),
        ("A,MONEY,FUNCTION", t1),
        ("A,NAME,INSTANCE METHOD", t1),
        ("A,NAME,INSTANCE METHOD", t1),
        ("A,NUMERIC,FUNCTION", t1),
        ("A,P,PROCEDURE", t1),
        ("A,PERSON,FUNCTION", t1),
        ("LIB,H,FUNCTION", t2),
    ];
    let specifications = [("AGE", t2), ("AGE", t2), ("GREETING", t1), ("NAME", t1), ("NAME", t1)];
    let expected: Vec<String> = routines
        .iter()
        .chain(&specifications)
        .map(|(what, at)| format!("{what},{at},{at}"))
        .chain([format!("TR,{t1}"), format!("SHOP,{t1},{t1}")])
        .collect();
    assert_eq!(stamps(&db), expected);
    let a = db.session("A");
    let short = [
        ("SELECT CREATED, LAST_ALTERED FROM INFORMATION_SCHEMA.ROUTINES_S WHERE ROUTINE_NAME = 'GREETING'", format!("{t2},{t2}")),
        ("SELECT CREATED, LAST_ALTERED FROM INFORMATION_SCHEMA.METHOD_SPECS WHERE METHOD_NAME = 'GREETING'", format!("{t1},{t1}")),
        ("SELECT CREATED FROM INFORMATION_SCHEMA.TRIGGERS_S", t1.to_owned()),
        ("SELECT CREATED, ALTERED FROM INFORMATION_SCHEMA.MODULES_S", format!("{t1},{t1}")),
    ];
    for (query, shown) in short {
        assert_eq!(rows(&a, query).unwrap(), [shown], "{query}");
    }
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.40 Description 24, 25, §6.30
/// Description 13, 14, §6.58 Description 13; ISO/IEC 9075-4:2006 draft
/// §19.4 Description 7, 8] (Feature T011); [DESIGN] RULES.md D25, D37:
/// the timestamps are kept with the definitions: after a crash, by the
/// definitions' records replayed, and after a checkpoint and a clean
/// close, by the catalog's image, the same as they were made, though the
/// clock reads a day later each time.
#[test]
fn timestamps_survive_recovery_and_checkpoints_t011() {
    let scratch = Scratch::new("information-timestamps");
    let dir = scratch.path("db");
    let made = {
        let (db, clock) = opened(&dir, T0);
        define(&db, &clock);
        stamps(&db)
        // Left without a checkpoint or a close: a crash.
    };
    {
        let (db, _) = opened(&dir, LATER);
        assert_eq!(stamps(&db), made, "replayed from the log");
        db.checkpoint().unwrap();
        db.close().unwrap();
    }
    let (db, _) = opened(&dir, LATER + 1);
    assert_eq!(stamps(&db), made, "read from the checkpoint's image");
    db.close().unwrap();
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.40 Description 24, 25]
/// (Feature T011); [DESIGN] RULES.md D57: a standby replaying its
/// primary's log shows the timestamps its primary does, its own clock
/// reading a day later.
#[test]
fn a_standby_shows_its_primarys_timestamps_t011() {
    let log = Arc::new(MemoryLog::new());
    let (primary, clock) = clocked(Database::new(), T0);
    primary.set_log(Arc::clone(&log) as Arc<dyn Log>).unwrap();
    define(&primary, &clock);
    let (standby, _) = clocked(Database::new(), LATER);
    standby.set_log(Arc::new(MemoryLog::new())).unwrap();
    Standby::replay(&standby, &mut log.read_from(Lsn(0))).unwrap();
    assert_eq!(stamps(&standby), stamps(&primary));
}
