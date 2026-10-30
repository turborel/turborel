//! Diagnostics (RULES.md D40): the events a database gives the sink it is
//! given, of its life and of the failures an operator must know of.

use turborel::db::{Database, OpenOptions, Opened};
use turborel::diagnostics::{Event, Extension, Level};
use turborel::error::Code;
use turborel::index::BTreeMethod;
use turborel::value::{Decimal, Value};

use crate::common::{Kept, Scramble, Scratch, arm, gated, locked, os};

/// Verifies: [DESIGN] RULES.md D40, `Diagnostics`: a database kept in a
/// directory tells its sink it was created, of each checkpoint taken, of a
/// setting changed and a plug-in registered, that it closed, and, opened
/// again, that it was reopened; a statement's own failure, a syntax error
/// or a broken constraint, is no event.
#[test]
fn a_database_tells_its_sink_of_its_life() {
    let scratch = Scratch::new("diagnostics");
    let dir = scratch.path("db");
    let kept = Kept::default();
    let db = Database::new();
    db.set_diagnostics(Box::new(kept.clone()));
    assert_eq!(db.open_files(&os(), &dir, &OpenOptions::default()).unwrap(), Opened::Created);
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE)").unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (1)").unwrap();
    assert_eq!(s.run("INSERT INTO T VALUES (1)").err().map(|e| e.code), Some(Code::Constraint));
    assert_eq!(s.run("SELEC K FROM T").err().map(|e| e.code), Some(Code::Syntax));
    s.commit().unwrap();
    db.checkpoint().unwrap();
    db.set_setting("LOCK_WAIT", &Value::Char("WAIT".into())).unwrap();
    db.register_index_method(Box::new(BTreeMethod::in_memory())).unwrap();
    drop(s);
    db.close().unwrap();
    let info = |e: &str| (Level::Info, e.to_owned());
    let registered = |kind, name: &str| info(&format!("{:?}", Event::Registered { kind, name: name.into() }));
    assert_eq!(
        kept.taken(),
        vec![
            // Opening registers the engines and methods that keep their data in the directory.
            registered(Extension::Engine, "columnstore"),
            registered(Extension::Engine, "rowstore"),
            registered(Extension::IndexMethod, "btree"),
            registered(Extension::IndexMethod, "hash"),
            info("Opened(Created)"),
            info("CheckpointTaken(1)"),
            info("SettingChanged { name: \"LOCK_WAIT\", value: Some(Char(\"WAIT\")) }"),
            registered(Extension::IndexMethod, "btree"),
            info("Closed"),
        ]
    );
    let again = Database::new();
    again.set_diagnostics(Box::new(kept.clone()));
    again.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    let reopened = kept.taken();
    assert_eq!(reopened.last(), Some(&info("Opened(Reopened)")), "{reopened:?}");
}

/// Verifies: [DESIGN] RULES.md D40, `Event::of`: a failure of the
/// database's making, here a memory reservation the budget refuses, is a
/// warning given to the sink, with the error the statement returns; and
/// the error keeps its message plain, its citation apart (RULES.md R-ERR-2).
#[test]
fn failures_an_operator_must_know_of_are_events() {
    let kept = Kept::default();
    let db = Database::new();
    db.set_diagnostics(Box::new(kept.clone()));
    db.set_setting("MEMORY_BUDGET", &Value::Exact(Decimal::new(1 << 20, 0))).unwrap();
    kept.taken();
    let refused = db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap_err();
    assert_eq!(refused.code, Code::Resources);
    assert!(!refused.msg.starts_with('['), "{}", refused.msg);
    db.session("A").run("SELECT K FROM NOWHERE").unwrap_err();
    let events = kept.taken();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, Level::Warning);
    assert!(events[0].1.contains("Resources"), "{events:?}");
}

/// Verifies: [DESIGN] RULES.md D36, D40: DIAGNOSTICS.LEVEL says the least
/// level told: INFO, the default, every event; WARNING, warnings and
/// errors; ERROR, errors alone; its words in any case, others refused.
/// What a program tells (`Database::tell`), a server's own events, is
/// filtered alike. The owner sets it in SQL, as the Operator's Guide
/// shows.
#[test]
fn events_below_the_level_are_not_told() {
    let kept = Kept::default();
    let db = Database::new();
    db.set_diagnostics(Box::new(kept.clone()));
    let (info, warning) = (Event::Listening { addresses: vec![] }, Event::StopHurried { sessions: 1 });
    let error = Event::AcceptFailed { failures: 1, message: "no file descriptor left".into() };
    let levels = |kept: &Kept| kept.taken().into_iter().map(|(level, _)| level).collect::<Vec<_>>();
    let set = |word: &str| db.set_setting("DIAGNOSTICS.LEVEL", &Value::Char(word.into()));
    for (word, told) in [
        ("INFO", vec![Level::Info, Level::Warning, Level::Error]),
        ("warning", vec![Level::Warning, Level::Error]),
        ("ERROR", vec![Level::Error]),
    ] {
        set(word).unwrap();
        kept.taken();
        for e in [&info, &warning, &error] {
            db.tell(e);
        }
        assert_eq!(levels(&kept), told, "{word}");
    }
    assert_eq!(set("DEBUG").err().map(|e| e.code), Some(Code::Api));
    set("INFO").unwrap();
    assert_eq!(levels(&kept), [Level::Info], "the change told, at INFO");
    // As the Operator's Guide sets it, by the database's owner.
    db.create_user("ANN", b"x").unwrap();
    db.set_owner("ANN").unwrap();
    db.session("ANN").run("SET DATABASE DIAGNOSTICS.LEVEL = 'WARNING'").unwrap();
    kept.taken();
    db.tell(&info);
    assert_eq!(levels(&kept), Vec::<Level>::new());
}

/// The slow statements `kept` heard of since last asked: who ran each,
/// and its text.
fn slow(kept: &Kept) -> Vec<String> {
    let events = kept.taken().into_iter().filter(|(_, e)| e.starts_with("SlowStatement"));
    events
        .map(|(level, e)| {
            assert_eq!(level, Level::Warning);
            e
        })
        .collect()
}

/// Verifies: [DESIGN] RULES.md D40: a statement whose work takes
/// DIAGNOSTICS.SLOW_STATEMENT or longer, here waiting for a lock, is told to
/// the sink as slow, with who ran it and its text, failed or not; one
/// quicker is not, nor any while the setting is 0.
#[test]
fn a_slow_statement_is_told() {
    let (db, holder) = locked();
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone()));
    let waiter = db.session("X");
    waiter.run("SET LOCK_TIMEOUT = 40").unwrap();
    assert!(waiter.run("INSERT INTO L VALUES (2)").is_err(), "waited, then refused");
    assert_eq!(slow(&kept), Vec::<String>::new(), "none while the setting is 0");
    db.set_setting("DIAGNOSTICS.SLOW_STATEMENT", &Value::Exact(Decimal::new(20, 0))).unwrap();
    assert!(waiter.run("INSERT INTO L VALUES (2)").is_err());
    waiter.run("SET LOCK_TIMEOUT = 0").unwrap();
    let told = slow(&kept);
    assert_eq!(told.len(), 1, "the wait, not the SET: {told:?}");
    assert!(told[0].contains(r#"user: "X""#) && told[0].contains("INSERT INTO L VALUES (2)"), "{told:?}");
    holder.commit().unwrap();
}

/// Verifies: [DESIGN] RULES.md D56, D48: while anything of the database is
/// encrypted, a slow statement is told by who ran it and its time alone,
/// its text left out, so no value meant for an encrypted table reaches a
/// log in the clear.
#[test]
fn a_slow_statement_of_an_encrypted_database_is_told_without_its_text() {
    let (db, holder) = locked();
    db.register_encoding(Box::new(Scramble)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE SECRET (V CHARACTER(10)) WITH (ENCODINGS = 'test scramble')").unwrap();
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone()));
    db.set_setting("DIAGNOSTICS.SLOW_STATEMENT", &Value::Exact(Decimal::new(20, 0))).unwrap();
    let waiter = db.session("X");
    waiter.run("SET LOCK_TIMEOUT = 40").unwrap();
    assert!(waiter.run("INSERT INTO L VALUES (2)").is_err(), "waited, then refused");
    let told = slow(&kept);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains(r#"user: "X""#) && told[0].contains("statement: None"), "{told:?}");
    holder.commit().unwrap();
}

/// Verifies: [DESIGN] RULES.md D40: a cursor's work, its opening and its
/// fetches, is timed: one whose fetch reads long is told as slow when it
/// is done with; the time between its fetches is its client's, and not
/// counted.
#[test]
fn a_cursor_is_timed_by_its_work() {
    let (db, gate, heard, go) = gated();
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone()));
    db.set_setting("DIAGNOSTICS.SLOW_STATEMENT", &Value::Exact(Decimal::new(20, 0))).unwrap();
    let s = db.session("X");
    let mut idle = s.open_cursor("SELECT K FROM G").unwrap();
    assert!(idle.fetch().unwrap().is_some());
    std::thread::sleep(std::time::Duration::from_millis(40));
    while idle.fetch().unwrap().is_some() {}
    drop(idle);
    assert_eq!(slow(&kept), Vec::<String>::new(), "its client's time");
    arm(&gate);
    let stalling = std::thread::spawn(move || {
        heard.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(40));
        go.send(()).unwrap();
    });
    let mut reading = s.open_cursor("SELECT K FROM G").unwrap();
    while reading.fetch().unwrap().is_some() {}
    stalling.join().unwrap();
    assert_eq!(slow(&kept), Vec::<String>::new(), "told when done with");
    drop(reading);
    let told = slow(&kept);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("SELECT K FROM G"), "{told:?}");
}

/// Verifies: [DESIGN] RULES.md D40, D56: a log segment that cannot be begun
/// is told to the database's sink, as an error, while the statements that
/// append to the log go on; the sink the database has as it fails, set
/// after it opened.
#[test]
fn a_failed_rotation_of_the_log_is_told() {
    let scratch = Scratch::new("diagnostics-rotation");
    let (fs, faults) = crate::faults::FaultyFs::with_faults();
    let db = Database::new();
    db.set_setting("LOG.SEGMENT_SIZE", &Value::Exact(Decimal::new(4096, 0))).unwrap();
    db.open_files(&fs, &scratch.path("db"), &OpenOptions::default()).unwrap();
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone()));
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, S CHARACTER(200))").unwrap();
    faults.inject(crate::faults::Rule::fail(crate::faults::Op::Open).on(&format!("log{}segment-", std::path::MAIN_SEPARATOR)));
    let a = db.session("A");
    for k in 0..100 {
        a.run(&format!("INSERT INTO T VALUES ({k}, 'row {k}')")).unwrap();
        a.commit().unwrap();
    }
    faults.clear();
    let told: Vec<(Level, String)> = kept.taken().into_iter().filter(|(_, e)| e.contains("could not begin a segment")).collect();
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].0, Level::Error);
    assert_eq!(crate::common::count(&a, "SELECT K FROM T"), 100);
}

/// A database in `scratch` over `fs`, its checkpoints run by the program
/// (JOBS.POLICY MANUAL), one due after each row change; schema A's table
/// T; and a sink keeping its events.
fn overdue_after_one(scratch: &Scratch, fs: &std::sync::Arc<dyn turborel::disk::io::FileSystem>) -> (Database, Kept) {
    let db = Database::new();
    db.open_files(fs, &scratch.path("db"), &OpenOptions::default()).unwrap();
    db.set_job_policy(turborel::threads::JobPolicy::Manual).unwrap();
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &Value::Exact(Decimal::new(1, 0))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone()));
    (db, kept)
}

/// The events of checkpoints overdue `kept` has, with their levels.
fn overdue(kept: &Kept) -> Vec<(Level, String)> {
    kept.taken().into_iter().filter(|(_, e)| e.starts_with("CheckpointOverdue")).collect()
}

/// Verifies: [DESIGN] RULES.md D25, D40, D55: with no checkpoint taken,
/// the log grown past `OVERDUE` times CHECKPOINT.AFTER_CHANGES is told as
/// a warning, as a transaction ends, with the job policy that leaves them
/// to the program; and told at most once a minute.
#[test]
fn checkpoints_not_taken_are_told_overdue() {
    let scratch = Scratch::new("diagnostics-overdue-policy");
    let (db, kept) = overdue_after_one(&scratch, &os());
    let a = db.session("A");
    let below = turborel::db::OVERDUE - 1;
    a.run(&format!("INSERT INTO T SELECT * FROM (VALUES {}) AS V (K)", vec!["(1)"; usize::try_from(below).unwrap()].join(", "))).unwrap();
    a.commit().unwrap();
    assert_eq!(overdue(&kept), vec![], "not yet");
    a.run("INSERT INTO T VALUES (2)").unwrap();
    a.commit().unwrap();
    let told = overdue(&kept);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].0, Level::Warning);
    assert!(told[0].1.contains(&format!("changes: {}", turborel::db::OVERDUE)) && told[0].1.contains("NotTaken(Manual)"), "{told:?}");
    a.run("INSERT INTO T VALUES (3)").unwrap();
    a.commit().unwrap();
    assert_eq!(overdue(&kept), vec![], "once a minute");
}

/// Verifies: [DESIGN] RULES.md D25, D40: checkpoints refused, each failing,
/// are told overdue with the failure that refuses them.
#[test]
fn checkpoints_refused_are_told_overdue_with_why() {
    let scratch = Scratch::new("diagnostics-overdue-refused");
    let (fs, faults) = crate::faults::FaultyFs::with_faults();
    let (db, kept) = overdue_after_one(&scratch, &fs);
    faults.inject(crate::faults::Rule::fail(crate::faults::Op::Write).on("catalog"));
    let a = db.session("A");
    for k in 0..turborel::db::OVERDUE {
        a.run(&format!("INSERT INTO T VALUES ({k})")).unwrap();
        a.commit().unwrap();
        assert_eq!(db.run_due().err().map(|e| e.code), Some(Code::Io));
    }
    faults.clear();
    let told = overdue(&kept);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].1.contains("Refused") && told[0].1.contains("injected"), "{told:?}");
}

/// Verifies: [DESIGN] RULES.md D25, D40: checkpoints taken while an open
/// transaction keeps the log from its first record are told overdue,
/// naming the transaction by how long ago it logged it, once the log
/// keeps `OVERDUE` times CHECKPOINT.AFTER_CHANGES.
#[test]
fn a_transaction_keeping_the_log_is_told() {
    let scratch = Scratch::new("diagnostics-overdue-transaction");
    let (db, kept) = overdue_after_one(&scratch, &os());
    let idle = db.session("A");
    idle.run("INSERT INTO T VALUES (0)").unwrap();
    let a = db.session("A");
    for k in 1..turborel::db::OVERDUE {
        a.run(&format!("INSERT INTO T VALUES ({k})")).unwrap();
        a.commit().unwrap();
        db.run_due().unwrap();
    }
    let told = overdue(&kept);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].1.contains("Transaction("), "{told:?}");
    idle.rollback().unwrap();
}
