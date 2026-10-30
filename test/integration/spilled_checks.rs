//! Spilling of what a data-changing statement did, kept for its
//! constraints' checks at its end, and of the checks of deferred
//! constraints, kept until they are made (RULES.md D38): held past
//! WORK_MEMORY in spill areas, every constraint's outcome as it is in
//! memory. Split from spilling.rs (RULES.md R-SIZE-2).

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{Counted, sorted, spill_bytes};
use crate::spilling::{SMALL, bytes};

/// Rows of each table but P and H: some thousands, each with a long
/// string, many times the smallest work area.
const ROWS: i64 = 3000;

/// What a statement came to: done, or its error's code and message.
type Outcome = Result<(), (Code, String)>;

/// The outcome of running `sql` in `s`.
fn outcome(s: &Session, sql: &str) -> Outcome {
    s.run(sql).map(|_| ()).map_err(|e| (e.code, e.msg))
}

/// Schema S, spilling to a counted provider, its work areas the smallest
/// if `small`, and a session of it: P (K PRIMARY KEY) of rows 0 and 1; C,
/// of `ROWS` rows, half referencing each row of P, deleted with it (ON
/// DELETE CASCADE); H, one row, referencing P's row 1 (NO ACTION); Q, of
/// `ROWS` rows, K its PRIMARY KEY, V UNIQUE and CHECK (V >= 0); and D, of
/// `ROWS` rows, its CHECK DV (V >= 0) initially deferred. C's and D's
/// changes are held in memory, and they have no index, so what spills of
/// them is what a statement keeps for its checks.
fn database(small: bool) -> (Database, Counted, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE P (K INTEGER PRIMARY KEY)
           CREATE TABLE C (K INTEGER, P INTEGER REFERENCES P ON DELETE CASCADE, S CHARACTER(60))
           CREATE TABLE H (P INTEGER REFERENCES P)
           CREATE TABLE Q (K INTEGER PRIMARY KEY, V INTEGER UNIQUE CHECK (V >= 0), S CHARACTER(60))
           CREATE TABLE D (K INTEGER, V INTEGER CONSTRAINT DV CHECK (V >= 0) INITIALLY DEFERRED, S CHARACTER(60))",
    )
    .unwrap();
    let s = db.session("S");
    for table in ["C", "Q", "D"] {
        s.run(&format!("ALTER TABLE {table} SET (TRANSACTIONS.HELD_BYTES = 67108864)")).unwrap();
    }
    s.run("INSERT INTO P VALUES (0), (1)").unwrap();
    s.run("INSERT INTO H VALUES (1)").unwrap();
    for start in (0..ROWS).step_by(500) {
        let row = |k: i64, second: i64| format!("({k}, {second}, '{}')", "s".repeat(50));
        let keyed: Vec<String> = (start..start + 500).map(|k| row(k, k % 2)).collect();
        let valued: Vec<String> = (start..start + 500).map(|k| row(k, k)).collect();
        s.run(&format!("INSERT INTO C VALUES {}", keyed.join(", "))).unwrap();
        s.run(&format!("INSERT INTO Q VALUES {}", valued.join(", "))).unwrap();
        s.run(&format!("INSERT INTO D VALUES {}", valued.join(", "))).unwrap();
    }
    s.commit().unwrap();
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    if small {
        db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    }
    (db, counted, s)
}

/// Verifies: [DESIGN] RULES.md D38 [ISO/IEC 9075:1992 §10.6 GR3, §11.8 GR2,
/// GR5]. Regression test (R-TEST-7): what a statement did, each row's id
/// and the old values of those deleted, was held in memory for the checks
/// at its end. With WORK_MEMORY at its least, a DELETE of one row whose
/// referential action deletes thousands holds them in a spill area, and
/// ends as it does in memory: done, the rows deleted with it; or, a NO
/// ACTION foreign key broken after them, refused, SQLCODE -201, and undone,
/// every row deleted put back.
#[test]
fn a_statements_effects_spill_and_are_checked() {
    let mut ends = Vec::new();
    for small in [false, true] {
        let (_db, counted, s) = database(small);
        let refused = outcome(&s, "DELETE FROM P WHERE K = 1");
        assert_eq!(refused.as_ref().err().map(|e| e.0), Some(Code::Constraint), "H references 1");
        assert_eq!(counted.taken() > 0, small, "the rows deleted with it spilled");
        assert_eq!(sorted(&s, "SELECT COUNT(*) FROM C"), ["3000"], "undone");
        let done = outcome(&s, "DELETE FROM P WHERE K = 0");
        assert_eq!(done, Ok(()));
        assert_eq!(counted.taken() > 0, small, "the rows deleted with it spilled");
        s.commit().unwrap();
        ends.push((refused, sorted(&s, "SELECT P, COUNT(*) FROM C GROUP BY P"), sorted(&s, "SELECT K FROM P")));
    }
    assert_eq!(ends[0], ends[1]);
    assert_eq!(ends[1].1, ["1,1500"]);
}

/// Verifies: [DESIGN] RULES.md D38 [X3.135-1986 §6.4 GR2, §8.12; ISO/IEC
/// 9075:1992 §11.6 GR3, §11.7 GR2]: with WORK_MEMORY at its least, an
/// UPDATE, an INSERT or a DELETE of thousands of rows, its effects and
/// their checks spilled, ends as it does in memory: a key passing through
/// duplicates and ending unique is done; a duplicate of a PRIMARY KEY or a
/// UNIQUE, or a CHECK false, refused with the same condition, and undone.
#[test]
fn a_statements_checks_spill_with_the_same_outcomes() {
    let statements = [
        "UPDATE Q SET V = V + 1",
        "UPDATE Q SET K = K + 1",
        "UPDATE Q SET V = 7",
        "UPDATE Q SET V = 0 - V",
        "UPDATE Q SET K = K / 2",
        "INSERT INTO Q SELECT K + 10000, V, S FROM Q",
        "INSERT INTO Q SELECT K + 10000, V + 10000, S FROM Q",
        "DELETE FROM Q WHERE K > 1000",
    ];
    let mut ends = Vec::new();
    for small in [false, true] {
        let (_db, counted, s) = database(small);
        let outcomes: Vec<Outcome> = statements.iter().map(|sql| outcome(&s, sql)).collect();
        assert_eq!(counted.taken() > 0, small);
        s.commit().unwrap();
        ends.push((outcomes, sorted(&s, "SELECT K, V FROM Q")));
    }
    let refused: Vec<bool> = ends[1].0.iter().map(Result::is_err).collect();
    assert_eq!(refused, [false, false, true, true, true, true, false, false]);
    assert!(ends[1].0.iter().flatten().count() == 4 && ends[1].0.iter().filter_map(|o| o.as_ref().err()).all(|e| e.0 == Code::Constraint));
    assert_eq!(ends[0], ends[1]);
    assert_eq!(ends[1].1.len(), 1000);
}

/// Verifies: [DESIGN] RULES.md D38 [ISO/IEC 9075:1992 §10.6 GR3, §14.2
/// GR3, §14.3 GR4, GR5a]. Regression test (R-TEST-7): the checks of a
/// deferred constraint waited in memory until made. With WORK_MEMORY at
/// its least, those of an UPDATE of thousands of rows wait in a spill
/// area, held while the transaction runs and freed as it commits, rolls
/// back, or they are made; and each ends as in memory: SET CONSTRAINTS
/// IMMEDIATE refused while one is false, the checks kept, and done once
/// all hold; an immediate check refusing the statement; COMMIT rolling
/// back, SQLSTATE 40002, while one is false.
#[test]
fn deferred_checks_spill_until_made() {
    let mut ends = Vec::new();
    for small in [false, true] {
        let (db, _counted, s) = database(small);
        assert_eq!(spill_bytes(&db), 0);
        let mut outcomes = Vec::new();
        let mut held = Vec::new();
        for sql in [
            "UPDATE D SET V = V + 1",
            "COMMIT",
            "UPDATE D SET V = 0 - V",
            "SET CONSTRAINTS ALL IMMEDIATE",
            "UPDATE D SET V = 0 - V",
            "SET CONSTRAINTS ALL IMMEDIATE",
            "UPDATE D SET V = 0 - V",
            "SET CONSTRAINTS ALL DEFERRED",
            "UPDATE D SET V = 0 - V",
            "ROLLBACK",
            "UPDATE D SET V = 0 - V",
            "COMMIT",
        ] {
            outcomes.push(outcome(&s, sql));
            held.push(spill_bytes(&db) > 0);
        }
        // Held from each deferring UPDATE until the checks are made, the
        // transaction ends, or ever only in memory.
        let waiting = [true, false, true, true, true, false, false, false, true, false, true, false];
        assert_eq!(held, waiting.map(|w| w && small), "small: {small}");
        ends.push((outcomes, sorted(&s, "SELECT V FROM D WHERE V < 3")));
    }
    let codes: Vec<Option<Code>> = ends[1].0.iter().map(|o| o.as_ref().err().map(|e| e.0)).collect();
    let (constraint, commit) = (Some(Code::Constraint), Some(Code::CommitFailed));
    assert_eq!(codes, [None, None, None, constraint, None, None, constraint, None, None, None, None, commit]);
    assert_eq!(ends[0], ends[1]);
    assert_eq!(ends[1].1, ["1", "2"], "the first UPDATE committed, the last rolled back");
}
