//! USAGE on a collation taken away [ISO/IEC 9075-2:2023 §11.2, §11.44,
//! §12.7; ISO/IEC 9075:1992 §11.37; RULES.md D60]: the columns reverted, in
//! what order, and what is refused.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::{Code, SqlError};

use crate::common::{Scratch, after_crash};

fn query(s: &Session, sql: &str) -> Vec<String> {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// The SQLCODE class and SQLSTATE of a refusal.
fn refused(result: Result<turborel::db::Completion, SqlError>) -> (Option<Code>, String) {
    let e = result.err();
    (e.as_ref().map(|e| e.code), e.map(|e| e.sqlstate().as_str().to_owned()).unwrap_or_default())
}

/// Schema A holds collation FOLD, case-insensitive and NO PAD, and grants
/// USAGE on it to B (and C, if `also_c`); B has table T of it, unique, Bob
/// in it.
fn granted(db: &Database, also_c: bool) {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE COLLATION FOLD FOR SQL_TEXT FROM CASE_INSENSITIVE NO PAD
           CREATE TABLE OWN (C VARCHAR(9) COLLATE A.FOLD)",
    )
    .unwrap();
    let a = db.session("A");
    a.run(if also_c { "GRANT USAGE ON COLLATION A.FOLD TO B, C" } else { "GRANT USAGE ON COLLATION A.FOLD TO B" }).unwrap();
    a.run("INSERT INTO OWN VALUES ('Ann')").unwrap();
    a.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (C VARCHAR(9) COLLATE A.FOLD UNIQUE)").unwrap();
    let b = db.session("B");
    b.run("INSERT INTO T VALUES ('Bob')").unwrap();
    b.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 GR12n, GR12q, GR12ad]
/// REVOKE USAGE on a collation with RESTRICT is refused (SQLSTATE 2B000)
/// while a column or domain of a schema it leaves without USAGE is of it,
/// and changes nothing; with CASCADE their columns and domains revert to
/// the default, the rows copied and the UNIQUE index made again, and a
/// view's column with them; the owner's table, and that of a grantee
/// keeping USAGE, stay of it.
#[test]
fn revoke_usage_reverts_impacted_columns_12_7() {
    let db = Database::new();
    granted(&db, true);
    db.schema("CREATE SCHEMA AUTHORIZATION C CREATE TABLE K (C VARCHAR(9) COLLATE A.FOLD)").unwrap();
    let c = db.session("C");
    c.run("INSERT INTO K VALUES ('Cy')").unwrap();
    c.commit().unwrap();
    let b = db.session("B");
    b.run("CREATE DOMAIN D AS VARCHAR(9) COLLATE A.FOLD").unwrap();
    b.run("CREATE VIEW V AS SELECT C FROM T").unwrap();
    b.commit().unwrap();
    let a = db.session("A");
    assert_eq!(refused(a.run("REVOKE USAGE ON COLLATION A.FOLD FROM B RESTRICT")), (Some(Code::SyntaxRule), "2B000".to_owned()));
    a.rollback().unwrap();
    assert_eq!(query(&b, "SELECT C FROM T WHERE C = 'BOB'"), ["Bob"], "of it still");
    b.rollback().unwrap();
    a.run("REVOKE USAGE ON COLLATION A.FOLD FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&b, "SELECT C FROM T WHERE C = 'BOB'").len(), 0, "of the default now");
    assert_eq!(query(&b, "SELECT C FROM V WHERE C = 'Bob'"), ["Bob"]);
    assert_eq!(query(&b, "SELECT COLLATION FOR (C) FROM V"), ["SQL_TEXT"], "the view's column too");
    b.run("INSERT INTO T VALUES ('bob')").unwrap();
    assert_eq!(b.run("INSERT INTO T VALUES ('Bob ')").err().map(|e| e.code), Some(Code::Constraint), "PAD SPACE, the default's");
    b.rollback().unwrap();
    let domain = "SELECT COALESCE(COLLATION_NAME, 'NONE') FROM INFORMATION_SCHEMA.DOMAINS WHERE DOMAIN_NAME = 'D'";
    assert_eq!(query(&b, domain), ["SQL_TEXT"]);
    b.rollback().unwrap();
    assert_eq!(query(&a, "SELECT C FROM OWN WHERE C = 'ANN'"), ["Ann"], "the owner's");
    a.rollback().unwrap();
    assert_eq!(query(&c, "SELECT COLLATION FOR (C) FROM K"), ["A.FOLD"], "one keeping USAGE");
}

/// Verifies: [EXTENSION] RULES.md R-ERR-5, [ISO/IEC 9075:1992 §11.37 SR1;
/// ISO/IEC 9075-2:2023 §10.7 SR1] a REVOKE with CASCADE of other than USAGE,
/// or of no collation a definition made, is refused before a row is
/// copied: the transaction goes on, its table of the collation still.
#[test]
fn a_revoke_usage_refused_leaves_no_effect() {
    let db = Database::new();
    granted(&db, false);
    let a = db.session("A");
    a.run("INSERT INTO OWN VALUES ('Eve')").unwrap();
    assert_eq!(refused(a.run("REVOKE SELECT ON COLLATION A.FOLD FROM B CASCADE")).0, Some(Code::SyntaxRule));
    assert_eq!(refused(a.run("REVOKE USAGE ON COLLATION A.NOPE FROM B CASCADE")).0, Some(Code::SyntaxRule));
    a.commit().unwrap();
    assert_eq!(query(&db.session("B"), "SELECT C FROM T WHERE C = 'BOB'"), ["Bob"]);
    assert_eq!(query(&a, "SELECT COUNT(*) FROM OWN"), ["2"]);
}

/// Verifies: [EXTENSION] RULES.md R-ERR-5, [ISO/IEC 9075-2:2023 §12.7
/// GR12ad] a REVOKE with CASCADE failing on its second table, the first
/// reverted, leaves the transaction able only to roll back (SQLCODE -913),
/// and the rollback leaves both tables as they were.
#[test]
fn a_revoke_usage_failing_on_a_later_table_only_rolls_back() {
    let db = Database::new();
    granted(&db, false);
    let b = db.session("B");
    b.run("CREATE TABLE U (C VARCHAR(9) COLLATE A.FOLD UNIQUE)").unwrap();
    b.run("INSERT INTO U VALUES ('a'), ('a ')").unwrap();
    b.commit().unwrap();
    let a = db.session("A");
    assert_eq!(refused(a.run("REVOKE USAGE ON COLLATION A.FOLD FROM B CASCADE")).0, Some(Code::Constraint), "equal once padded");
    assert_eq!(a.commit().err().map(|e| e.code), Some(Code::CommitFailed));
    a.rollback().ok();
    assert_eq!(query(&b, "SELECT C FROM T WHERE C = 'BOB'"), ["Bob"]);
    assert_eq!(query(&b, "SELECT COUNT(*) FROM U"), ["2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.44 GR2, §12.7 GR12ad;
/// ISO/IEC 9075:1992 §11.8] a table referencing one reverted after it,
/// whatever their names, is checked again once both are, so a key that no
/// longer holds by the default fails the statement (SQLCODE -201); one that
/// holds is checked by the default after. Regression test (R-TEST-7):
/// the referencing table, first by name, was checked against the
/// referenced one still of the collation, and a key left dangling.
#[test]
fn a_referencing_table_is_checked_once_its_parent_reverts() {
    for revoking in [false, true] {
        let db = Database::new();
        granted(&db, false);
        let b = db.session("B");
        b.run("CREATE TABLE P (K VARCHAR(9) COLLATE A.FOLD UNIQUE)").unwrap();
        b.run("CREATE TABLE C (K VARCHAR(9) COLLATE A.FOLD REFERENCES P (K))").unwrap();
        b.run("INSERT INTO P VALUES ('Bob')").unwrap();
        b.run("INSERT INTO C VALUES ('bob')").unwrap();
        b.commit().unwrap();
        let a = db.session("A");
        let statement = if revoking { "REVOKE USAGE ON COLLATION A.FOLD FROM B CASCADE" } else { "DROP COLLATION A.FOLD CASCADE" };
        assert_eq!(refused(a.run(statement)).0, Some(Code::Constraint), "bob is not Bob by the default");
        a.rollback().ok();
        b.run("UPDATE C SET K = 'Bob'").unwrap();
        b.commit().unwrap();
        a.run(statement).unwrap();
        a.commit().unwrap();
        assert_eq!(b.run("INSERT INTO C VALUES ('bob')").err().map(|e| e.code), Some(Code::Constraint));
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 GR12ad] a table of a
/// schema keeping USAGE whose foreign key references a table reverted is
/// checked again: a key no longer matching fails the statement (SQLCODE
/// -201); one matching holds after by the referenced column's default.
#[test]
fn a_one_sided_revert_checks_the_key_again() {
    let db = Database::new();
    granted(&db, false);
    let b = db.session("B");
    b.run("GRANT REFERENCES ON T TO A").unwrap();
    b.commit().unwrap();
    let a = db.session("A");
    a.run("CREATE TABLE CH (K VARCHAR(9) COLLATE A.FOLD REFERENCES B.T (C))").unwrap();
    a.run("INSERT INTO CH VALUES ('bob')").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(a.run("REVOKE USAGE ON COLLATION A.FOLD FROM B CASCADE")).0, Some(Code::Constraint));
    a.rollback().ok();
    a.run("UPDATE CH SET K = 'Bob'").unwrap();
    a.run("REVOKE USAGE ON COLLATION A.FOLD FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(a.run("INSERT INTO CH VALUES ('BOB')").err().map(|e| e.code), Some(Code::Constraint), "by B.T's default now");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 GR12a, GR12c, GR12t,
/// GR12v; ISO/IEC 9075:1992 §11.37 SR13, SR14, SR18] a view and a CHECK of
/// a schema left without USAGE that name the collation are abandoned:
/// RESTRICT is refused (SQLSTATE 2A000), CASCADE drops them; the owner's
/// own view naming it stays.
#[test]
fn revoke_usage_abandons_what_names_the_collation_12_7() {
    let db = Database::new();
    granted(&db, false);
    let b = db.session("B");
    b.run("CREATE TABLE N (S VARCHAR(9), CONSTRAINT NAMED CHECK (S COLLATE A.FOLD <> 'x'))").unwrap();
    b.run("CREATE VIEW BV AS SELECT S FROM N WHERE S COLLATE A.FOLD = 'y'").unwrap();
    b.run("DELETE FROM T").unwrap();
    b.run("DROP TABLE T CASCADE").unwrap();
    b.commit().unwrap();
    let a = db.session("A");
    a.run("CREATE VIEW AV AS SELECT C FROM OWN WHERE C COLLATE A.FOLD = 'ann'").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(a.run("REVOKE USAGE ON COLLATION A.FOLD FROM B RESTRICT")), (Some(Code::SyntaxRule), "2A000".to_owned()));
    a.rollback().unwrap();
    a.run("REVOKE USAGE ON COLLATION A.FOLD FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(b.open_cursor("SELECT S FROM BV").err().map(|e| e.code), Some(Code::SyntaxRule), "the view went");
    b.rollback().unwrap();
    b.run("INSERT INTO N VALUES ('X')").unwrap();
    b.commit().unwrap();
    assert_eq!(query(&a, "SELECT C FROM AV"), ["Ann"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.37 SR7, SR18; ISO/IEC
/// 9075-2:2023 §12.7 GR12q] a grant option taken from B takes the grant B
/// made C with it: RESTRICT is refused, first for C's column impacted
/// (SQLSTATE 2B000), then, with none, for the grant lost (2A000); CASCADE
/// reverts C's column, and C holds USAGE no longer while B does.
#[test]
fn a_grant_option_lost_takes_a_collations_grants_with_it_11_37() {
    let db = Database::new();
    granted(&db, false);
    let a = db.session("A");
    a.run("GRANT USAGE ON COLLATION A.FOLD TO B WITH GRANT OPTION").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    b.run("GRANT USAGE ON COLLATION A.FOLD TO C").unwrap();
    b.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION C CREATE TABLE K (C VARCHAR(9) COLLATE A.FOLD)").unwrap();
    let revoke = "REVOKE GRANT OPTION FOR USAGE ON COLLATION A.FOLD FROM B";
    assert_eq!(refused(a.run(&format!("{revoke} RESTRICT"))), (Some(Code::SyntaxRule), "2B000".to_owned()), "C's column");
    a.rollback().unwrap();
    let c = db.session("C");
    c.run("DROP TABLE K CASCADE").unwrap();
    c.commit().unwrap();
    assert_eq!(refused(a.run(&format!("{revoke} RESTRICT"))), (Some(Code::SyntaxRule), "2A000".to_owned()), "the grant to C");
    a.rollback().unwrap();
    c.run("CREATE TABLE K (C VARCHAR(9) COLLATE A.FOLD)").unwrap();
    c.run("INSERT INTO K VALUES ('Cy')").unwrap();
    c.commit().unwrap();
    a.run(&format!("{revoke} CASCADE")).unwrap();
    a.commit().unwrap();
    assert_eq!(query(&c, "SELECT C FROM K WHERE C = 'Cy'"), ["Cy"]);
    assert_eq!(query(&c, "SELECT COLLATION FOR (C) FROM K"), ["SQL_TEXT"]);
    assert_eq!(c.run("CREATE TABLE L (C VARCHAR(9) COLLATE A.FOLD)").err().map(|e| e.code), Some(Code::Privilege));
    c.rollback().unwrap();
    assert_eq!(query(&b, "SELECT COLLATION FOR (C) FROM T"), ["A.FOLD"], "B holds it still");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.2 SR3, GR5; ISO/IEC
/// 9075-2:2023 §11.2 GR5, §11.44 GR2] DROP SCHEMA with RESTRICT is refused
/// while it holds a collation; with CASCADE the columns of other schemas'
/// tables of its collation revert, their rows kept and UNIQUE made again,
/// and their views naming it go.
#[test]
fn drop_schema_reverts_other_schemas_columns_11_2() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE COLLATION X FOR SQL_TEXT FROM CASE_INSENSITIVE NO PAD").unwrap();
    let s = db.session("S");
    s.run("GRANT USAGE ON COLLATION S.X TO B").unwrap();
    s.commit().unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE T (C VARCHAR(9) COLLATE S.X UNIQUE)
           CREATE VIEW V AS SELECT C FROM T WHERE C COLLATE S.X = 'bob'",
    )
    .unwrap();
    let b = db.session("B");
    b.run("INSERT INTO T VALUES ('Bob')").unwrap();
    b.commit().unwrap();
    assert_eq!(refused(s.run("DROP SCHEMA S RESTRICT")).0, Some(Code::SyntaxRule), "SR3: it holds a collation");
    s.rollback().unwrap();
    s.run("DROP SCHEMA S CASCADE").unwrap();
    s.commit().unwrap();
    assert_eq!(query(&b, "SELECT C FROM T WHERE C = 'BOB'").len(), 0);
    assert_eq!(query(&b, "SELECT C FROM T WHERE C = 'Bob'"), ["Bob"]);
    b.run("INSERT INTO T VALUES ('bob')").unwrap();
    assert_eq!(b.open_cursor("SELECT C FROM V").err().map(|e| e.code), Some(Code::SyntaxRule), "the view went");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.44 GR2] DROP COLLATION
/// reverts the columns of a table of another owner, as its General Rule has
/// no Access Rule.
#[test]
fn drop_collation_reverts_another_owners_table_11_44() {
    let db = Database::new();
    granted(&db, false);
    let a = db.session("A");
    a.run("DROP COLLATION A.FOLD CASCADE").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    assert_eq!(query(&b, "SELECT COLLATION FOR (C) FROM T"), ["SQL_TEXT"]);
    assert_eq!(query(&b, "SELECT C FROM T WHERE C = 'Bob'"), ["Bob"]);
}

/// Verifies: [DESIGN] RULES.md D25, D33, [ISO/IEC 9075-2:2023 §12.7 GR12ad]
/// a REVOKE with CASCADE committed survives reopening, after a close and
/// from the log alone: the reverted table is of the default, its UNIQUE by
/// it; one left uncommitted at a failure leaves the table of the collation.
#[test]
fn a_revoke_usage_survives_reopening() {
    for closed in [true, false] {
        let scratch = Scratch::new(if closed { "revoked-closed" } else { "revoked-failed" });
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let crashed = Database::new();
        crashed.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        granted(&crashed, false);
        let a = crashed.session("A");
        a.run("REVOKE USAGE ON COLLATION A.FOLD FROM B CASCADE").unwrap();
        a.commit().unwrap();
        if closed {
            crashed.close().unwrap();
        } else {
            // Open at the failure, so not committed.
            a.run("DROP COLLATION A.FOLD CASCADE").unwrap();
        }
        let db = Database::new();
        db.open_files(&if closed { Arc::clone(&fs) } else { after_crash() }, &dir, &OpenOptions::default()).unwrap();
        let b = db.session("B");
        assert_eq!(query(&b, "SELECT C FROM T WHERE C = 'Bob'"), ["Bob"]);
        assert_eq!(b.run("INSERT INTO T VALUES ('Bob ')").err().map(|e| e.code), Some(Code::Constraint));
        b.rollback().unwrap();
        b.run("INSERT INTO T VALUES ('bob')").unwrap();
        b.rollback().unwrap();
        assert_eq!(query(&db.session("A"), "SELECT COLLATION FOR (C) FROM OWN"), ["A.FOLD"], "the drop uncommitted");
        db.close().unwrap();
        drop((db, a, crashed));
    }
}

/// Verifies: [EXTENSION] RULES.md D60, [ISO/IEC 9075-2:2023 §12.7 GR12ad,
/// §11.44 GR2] a table, domain or column defined of a collation made holds
/// its name until its transaction ends, so no REVOKE or DROP of it, which
/// would revert what it cannot see, runs meanwhile (SQLCODE -911).
#[test]
fn a_definition_holds_its_collation_until_it_ends() {
    let db = Database::new();
    granted(&db, false);
    let (a, b) = (db.session("A"), db.session("B"));
    for definition in [
        "CREATE TABLE U (C VARCHAR(9) COLLATE A.FOLD)",
        "CREATE DOMAIN D AS VARCHAR(9) COLLATE A.FOLD",
        "ALTER TABLE T ADD COLUMN E VARCHAR(9) COLLATE A.FOLD",
    ] {
        b.run(definition).unwrap();
        assert_eq!(refused(a.run("REVOKE USAGE ON COLLATION A.FOLD FROM B CASCADE")).0, Some(Code::Conflict), "{definition}");
        a.rollback().unwrap();
        assert_eq!(refused(a.run("DROP COLLATION A.FOLD CASCADE")).0, Some(Code::Conflict), "{definition}");
        a.rollback().unwrap();
        b.rollback().unwrap();
    }
    a.run("DROP COLLATION A.FOLD CASCADE").unwrap();
}
