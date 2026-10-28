//! \[EXTENSION\] Deferrable constraints [ISO/IEC 9075:1992 §10.6], SET
//! CONSTRAINTS [§14.2], and the checks COMMIT makes [§14.3 GR4, GR5a].

use turborel::db::{Completion, Database, Session};
use turborel::error::Code;

use crate::common::{exec, sorted};

/// The SQLSTATE of running `sql` in `s`: "00000" if it succeeds.
fn state(s: &Session, sql: &str) -> String {
    match s.run(sql) {
        Ok(done) => done.sqlstate().as_str().to_owned(),
        Err(e) => e.sqlstate().as_str().to_owned(),
    }
}

/// Schema D: P (K PRIMARY KEY); C, whose FOREIGN KEY C_P is initially
/// deferred; T, whose CHECK POS is deferrable but initially immediate; U,
/// whose UNIQUE UK, N, whose NOT NULL NN, and Q, whose PRIMARY KEY QK, are
/// initially deferred.
fn deferring() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION D
           CREATE TABLE P (K INTEGER PRIMARY KEY)
           CREATE TABLE C (K INTEGER, CONSTRAINT C_P FOREIGN KEY (K) REFERENCES P INITIALLY DEFERRED)
           CREATE TABLE T (A INTEGER CONSTRAINT POS CHECK (A > 0) DEFERRABLE, B INTEGER CONSTRAINT FIRM CHECK (B > 0))
           CREATE TABLE U (K INTEGER CONSTRAINT UK UNIQUE DEFERRABLE INITIALLY DEFERRED, I INTEGER)
           CREATE TABLE N (K INTEGER CONSTRAINT NN NOT NULL INITIALLY DEFERRED)
           CREATE TABLE Q (K INTEGER, CONSTRAINT QK PRIMARY KEY (K) INITIALLY DEFERRED)",
    )
    .unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §10.6 GR2-GR4; §14.3 GR4,
/// GR5a]: an initially deferred constraint is checked at COMMIT, not at
/// each statement: a FOREIGN KEY, a UNIQUE, a NOT NULL, a PRIMARY KEY's
/// uniqueness and its columns not null, each may be broken until then;
/// COMMIT then fails with SQLSTATE 40002 and the transaction rolled back.
#[test]
fn deferred_constraints_wait_for_commit_10_6() {
    let db = deferring();
    let s = db.session("D");
    s.run("INSERT INTO C VALUES (1)").unwrap();
    s.run("INSERT INTO P VALUES (1)").unwrap();
    s.run("INSERT INTO U VALUES (1, 1)").unwrap();
    s.run("INSERT INTO U VALUES (1, 2)").unwrap();
    s.run("UPDATE U SET K = 2 WHERE I = 2").unwrap();
    s.run("INSERT INTO N VALUES (NULL)").unwrap();
    s.run("UPDATE N SET K = 5").unwrap();
    s.run("INSERT INTO Q VALUES (NULL)").unwrap();
    s.run("UPDATE Q SET K = 1").unwrap();
    assert_eq!(state(&s, "COMMIT"), "00000");
    assert_eq!(sorted(&s, "SELECT K FROM U"), vec!["1", "2"]);
    for (sql, what) in [
        ("INSERT INTO C VALUES (2)", "no P is 2"),
        ("INSERT INTO U VALUES (2, 3)", "a second 2"),
        ("INSERT INTO N VALUES (NULL)", "a null"),
        ("INSERT INTO Q VALUES (NULL)", "a null key"),
        ("INSERT INTO Q VALUES (1)", "a second key 1"),
        ("DELETE FROM P", "C references 1"),
    ] {
        s.run(sql).unwrap();
        assert_eq!(state(&s, "COMMIT WORK"), "40002", "{what}");
        let e = s.run("COMMIT").map(|_| ()).map_err(|e| e.code);
        assert_eq!(e, Ok(()), "the transaction is over: {what}");
    }
    assert_eq!(sorted(&s, "SELECT K FROM C"), vec!["1"], "each rolled back");
    assert_eq!(sorted(&s, "SELECT K FROM P"), vec!["1"]);
    s.run("DELETE FROM P").unwrap();
    s.run("INSERT INTO P VALUES (1)").unwrap();
    assert_eq!(state(&s, "COMMIT"), "00000", "a row referenced deleted and put back");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §14.2 SR1, SR2, GR1-GR3;
/// §10.6 GR3]: SET CONSTRAINTS defers a deferrable constraint, by name or
/// ALL; IMMEDIATE checks what waits, failing, the constraint left deferred,
/// while it is not satisfied; a constraint not deferrable, or no
/// constraint, is refused; it sets the modes of the next transaction if
/// none is active, and modes end with their transaction.
#[test]
fn set_constraints_14_2() {
    let db = deferring();
    let s = db.session("D");
    assert_eq!(exec(&s, "INSERT INTO T VALUES (-1, 1)"), Err(Code::Constraint), "POS is initially immediate");
    assert_eq!(exec(&s, "SET CONSTRAINTS FIRM DEFERRED"), Err(Code::SyntaxRule), "FIRM is not deferrable");
    assert_eq!(exec(&s, "SET CONSTRAINTS NOTHING IMMEDIATE"), Err(Code::SyntaxRule), "no such constraint");
    s.run("SET CONSTRAINTS POS DEFERRED").unwrap();
    s.run("INSERT INTO T VALUES (-1, 1)").unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (1, -1)"), Err(Code::Constraint), "FIRM is still checked");
    assert_eq!(state(&s, "SET CONSTRAINTS POS IMMEDIATE"), "23000");
    s.run("INSERT INTO T VALUES (-2, 1)").unwrap();
    s.run("UPDATE T SET A = 1").unwrap();
    assert_eq!(exec(&s, "SET CONSTRAINTS D.POS IMMEDIATE"), Ok(Completion::Success));
    assert_eq!(exec(&s, "INSERT INTO T VALUES (-1, 1)"), Err(Code::Constraint), "immediate again");
    s.run("COMMIT").unwrap();

    s.run("SET CONSTRAINTS ALL DEFERRED").unwrap();
    s.run("INSERT INTO T VALUES (-1, 1)").unwrap();
    s.run("INSERT INTO C VALUES (9)").unwrap();
    assert_eq!(state(&s, "SET CONSTRAINTS C_P, POS IMMEDIATE"), "23000");
    s.run("ROLLBACK").unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (-1, 1)"), Err(Code::Constraint), "the modes ended with the transaction");
    s.run("ROLLBACK").unwrap();
    s.run("SET CONSTRAINTS POS DEFERRED").unwrap();
    assert_eq!(s.transaction(), None, "SET CONSTRAINTS initiates no transaction");
    s.run("INSERT INTO T VALUES (-1, 1)").unwrap();
    s.run("ALTER TABLE T DROP CONSTRAINT POS RESTRICT").unwrap();
    assert_eq!(state(&s, "COMMIT"), "00000", "the constraint deferred is gone");
    assert_eq!(sorted(&s, "SELECT A FROM T"), vec!["-1", "1", "1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §10.6 GR3, §11.16 GR2]: a
/// constraint added INITIALLY DEFERRED by ALTER TABLE holds of the rows
/// there are at COMMIT; a CHECK with a subquery may be deferred too.
#[test]
fn constraints_added_deferred_10_6() {
    let db = deferring();
    let s = db.session("D");
    s.run("INSERT INTO N VALUES (5)").unwrap();
    s.run("ALTER TABLE N ADD CONSTRAINT IN_P CHECK (K IN (SELECT K FROM P)) INITIALLY DEFERRED").unwrap();
    s.run("INSERT INTO P VALUES (5)").unwrap();
    assert_eq!(state(&s, "COMMIT"), "00000");
    s.run("DELETE FROM P").unwrap();
    assert_eq!(state(&s, "COMMIT"), "40002", "N's 5 is in no P");
    assert_eq!(sorted(&s, "SELECT K FROM P"), vec!["5"]);
}
