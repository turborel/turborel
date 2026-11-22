//! The table of an INSERT run directly, resolved once while what it was
//! resolved with holds (RULES.md D45, `db/write/targets.rs`): resolved
//! again when the table is made anew or altered, a view names another
//! table, a privilege is revoked or granted, the authorization, the role
//! or the default schema change, or a transaction's own definitions are
//! undone; never so for an INSERT of a procedure's body or a triggered
//! action, which sees each table as it was when it was defined.

use std::sync::Arc;

use turborel::clock::{Clock, FixedClock};
use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::rows;

/// Schema X with T (K INTEGER) and U (K INTEGER, J INTEGER), and X's session.
fn schema() -> (Database, Session) {
    let db = Database::new();
    for user in ["OWNER", "B"] {
        db.create_user(user, b"x").unwrap();
    }
    db.set_owner("OWNER").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER) CREATE TABLE U (K INTEGER, J INTEGER)").unwrap();
    let x = db.session("X");
    (db, x)
}

/// The SQLCODE class of `sql` run in `s`, which fails; its transaction
/// rolled back.
fn refused(s: &Session, sql: &str) -> Code {
    let code = s.run(sql).unwrap_err().code;
    s.rollback().unwrap();
    code
}

/// Verifies: [X3.135-1986 §8.7 SR1-SR5, GR1]; [DESIGN] RULES.md D45: INSERTs
/// of one table differing in their values, run directly, each put their row
/// in the table named, its columns as listed, others their defaults; those
/// naming another table or other columns are not taken for them.
#[test]
fn inserts_differing_in_their_values_resolve_their_table_once() {
    let (_db, x) = schema();
    for k in 1..=3 {
        x.run(&format!("INSERT INTO T VALUES ({k})")).unwrap();
        x.run(&format!("INSERT INTO U (K) VALUES ({k})")).unwrap();
        x.run(&format!("INSERT INTO U (J) VALUES ({k})")).unwrap();
        x.run(&format!("INSERT INTO X.U VALUES ({k}, {k})")).unwrap();
    }
    assert_eq!(rows(&x, "SELECT K FROM T ORDER BY K").unwrap(), ["1", "2", "3"]);
    let u = rows(&x, "SELECT K, J FROM U ORDER BY K, J").unwrap();
    assert_eq!(u, ["1,1", "1,NULL", "2,2", "2,NULL", "3,3", "3,NULL", "NULL,1", "NULL,2", "NULL,3"]);
    assert_eq!(refused(&x, "INSERT INTO T (J) VALUES (1)"), Code::SyntaxRule, "T has no J");
}

/// Verifies: [X3.135-1986 §8.7 SR3, GR1b]; [ISO/IEC 9075:1992 §11.11 GR1, GR2];
/// [DESIGN] RULES.md D45: a column added after an INSERT ran is seen by the
/// next, its default given; a table dropped and made anew is the one the
/// next inserts into; a table made in a transaction rolled back is no more.
#[test]
fn a_table_altered_or_made_anew_is_resolved_again() {
    let (_db, x) = schema();
    x.run("INSERT INTO T VALUES (1)").unwrap();
    x.run("ALTER TABLE T ADD COLUMN D INTEGER DEFAULT 7").unwrap();
    x.run("INSERT INTO T (K) VALUES (2)").unwrap();
    x.run("INSERT INTO T VALUES (3, 30)").unwrap();
    assert_eq!(rows(&x, "SELECT K, D FROM T ORDER BY K").unwrap(), ["1,7", "2,7", "3,30"]);
    x.commit().unwrap();
    x.run("DROP TABLE T CASCADE").unwrap();
    x.run("CREATE TABLE T (K INTEGER, D INTEGER DEFAULT 8, E INTEGER DEFAULT 9)").unwrap();
    x.run("INSERT INTO T (K) VALUES (4)").unwrap();
    assert_eq!(rows(&x, "SELECT K, D, E FROM T").unwrap(), ["4,8,9"]);
    x.commit().unwrap();
    x.run("CREATE TABLE W (K INTEGER)").unwrap();
    x.run("INSERT INTO W VALUES (1)").unwrap();
    x.rollback().unwrap();
    assert_eq!(refused(&x, "INSERT INTO W VALUES (2)"), Code::SyntaxRule, "W is undone");
}

/// Verifies: [X3.135-1986 §8.7 SR2, §6.5 GR2]; [DESIGN] RULES.md D45: an
/// INSERT into a view puts its row in the view's table; once the view is
/// dropped and made over another table, in that one.
#[test]
fn a_view_made_anew_is_resolved_again() {
    let (_db, x) = schema();
    x.run("CREATE VIEW V AS SELECT K FROM T").unwrap();
    x.run("INSERT INTO V VALUES (1)").unwrap();
    x.run("DROP VIEW V CASCADE").unwrap();
    x.run("CREATE VIEW V AS SELECT K FROM U").unwrap();
    x.run("INSERT INTO V VALUES (2)").unwrap();
    assert_eq!(rows(&x, "SELECT K FROM T").unwrap(), ["1"]);
    assert_eq!(rows(&x, "SELECT K FROM U").unwrap(), ["2"]);
}

/// Verifies: [ISO/IEC 9075:1992 §13.8 AR1, §11.36, §11.37]; [ISO/IEC 9075-2:2023
/// §19.3]; [DESIGN] RULES.md D45: the INSERT privilege is checked at
/// each INSERT that follows a grant or a revoke, and as the role a session
/// sets or removes says.
#[test]
fn a_privilege_revoked_or_granted_is_checked_again() {
    let (db, x) = schema();
    let b = db.session("B");
    assert_eq!(refused(&b, "INSERT INTO X.T VALUES (1)"), Code::Privilege, "none granted");
    x.run("GRANT INSERT ON T TO B").unwrap();
    x.commit().unwrap();
    b.run("INSERT INTO X.T VALUES (2)").unwrap();
    b.commit().unwrap();
    x.run("REVOKE INSERT ON T FROM B CASCADE").unwrap();
    x.commit().unwrap();
    assert_eq!(refused(&b, "INSERT INTO X.T VALUES (3)"), Code::Privilege, "revoked");
    let owner = db.session("OWNER");
    owner.run("CREATE ROLE R").unwrap();
    owner.run("GRANT R TO B").unwrap();
    owner.commit().unwrap();
    x.run("GRANT INSERT ON T TO R").unwrap();
    x.commit().unwrap();
    b.run("SET ROLE 'R'").unwrap();
    b.run("INSERT INTO X.T VALUES (4)").unwrap();
    b.commit().unwrap();
    b.run("SET ROLE NONE").unwrap();
    assert_eq!(refused(&b, "INSERT INTO X.T VALUES (5)"), Code::Privilege, "R's alone");
    assert_eq!(rows(&x, "SELECT K FROM T ORDER BY K").unwrap(), ["2", "4"]);
}

/// Verifies: [ISO/IEC 9075:1992 §5.4 SR8, §16.2 GR3b, §16.4]; [DESIGN]
/// RULES.md D45: an unqualified name is that of a table of the default
/// schema at each INSERT, and of the authorization of then.
#[test]
fn the_default_schema_and_the_authorization_resolve_it_again() {
    let (db, x) = schema();
    db.schema("CREATE SCHEMA Y AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    x.run("INSERT INTO T VALUES (1)").unwrap();
    x.commit().unwrap();
    x.run("SET SCHEMA 'Y'").unwrap();
    x.run("INSERT INTO T VALUES (2)").unwrap();
    x.commit().unwrap();
    assert_eq!(rows(&x, "SELECT K FROM X.T").unwrap(), ["1"]);
    assert_eq!(rows(&x, "SELECT K FROM Y.T").unwrap(), ["2"]);
    x.commit().unwrap();
    x.run("GRANT INSERT ON X.T TO OWNER").unwrap();
    x.commit().unwrap();
    let owner = db.session("OWNER");
    owner.run("INSERT INTO X.T VALUES (3)").unwrap();
    owner.commit().unwrap();
    owner.run("SET SESSION AUTHORIZATION 'B'").unwrap();
    assert_eq!(refused(&owner, "INSERT INTO X.T VALUES (4)"), Code::Privilege, "B holds none");
}

/// Verifies: [X3.135-1986 §8.7 GR2]; [ISO/IEC 9075:1992 §6.8 GR1]; [DESIGN]
/// RULES.md D45: an INSERT through a view WITH CHECK OPTION whose condition
/// reads CURRENT_TIMESTAMP is checked by the instant of each, its table
/// resolved at each, as such a binding holds the instant.
#[test]
fn a_view_reading_the_instant_is_resolved_at_each_insert() {
    let (db, x) = schema();
    let clock = Arc::new(FixedClock::at(0));
    db.set_clock(Arc::clone(&clock) as Arc<dyn Clock>).unwrap();
    x.run("CREATE TABLE E (K INTEGER, W TIMESTAMP(0) WITH TIME ZONE)").unwrap();
    x.run("CREATE VIEW V AS SELECT K, W FROM E WHERE W <= CURRENT_TIMESTAMP WITH CHECK OPTION").unwrap();
    x.commit().unwrap();
    let later = "TIMESTAMP '1970-01-01 00:01:00+00:00'";
    x.run("INSERT INTO V VALUES (1, TIMESTAMP '1970-01-01 00:00:00+00:00')").unwrap();
    x.commit().unwrap();
    assert_eq!(refused(&x, &format!("INSERT INTO V VALUES (2, {later})")), Code::Constraint, "past the instant");
    clock.set(120_000_000);
    x.run(&format!("INSERT INTO V VALUES (3, {later})")).unwrap();
    assert_eq!(rows(&x, "SELECT K FROM E ORDER BY K").unwrap(), ["1", "3"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.21 GR2, Note 561, §11.60]; [DESIGN]
/// RULES.md D45: MC/DC of whether an INSERT's table is kept, `keeping` none
/// and `Params::sees_whole`: an INSERT run directly, both true, sees the
/// column added; one of a procedure's body or of a triggered action,
/// `sees_whole` false, sees the table as it was when it was defined, though
/// one run directly resolved it with the column; a prepared one, `keeping`
/// given, is `kept_plans.rs`'s, its plan holding the instant made at each
/// execution there.
#[test]
fn a_procedure_or_trigger_insert_is_never_kept() {
    let (_db, x) = schema();
    x.run("CREATE PROCEDURE P () MODIFIES SQL DATA INSERT INTO T VALUES (5)").unwrap();
    x.run("CREATE TRIGGER G AFTER INSERT ON U FOR EACH STATEMENT INSERT INTO T VALUES (6)").unwrap();
    x.commit().unwrap();
    x.run("ALTER TABLE T ADD COLUMN D INTEGER DEFAULT 7").unwrap();
    x.commit().unwrap();
    x.run("INSERT INTO T VALUES (1, 10)").unwrap();
    x.run("CALL P()").unwrap();
    x.run("INSERT INTO U VALUES (2, 20)").unwrap();
    x.run("INSERT INTO T VALUES (3, 30)").unwrap();
    assert_eq!(rows(&x, "SELECT K, D FROM T ORDER BY K").unwrap(), ["1,10", "3,30", "5,7", "6,7"]);
}
