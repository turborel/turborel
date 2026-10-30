//! Domains [ISO/IEC 9075:1992 §11.21–§11.27]: columns of a domain, its
//! default and constraints, the statements that change and drop it, CAST
//! to it, and USAGE on it.

use turborel::db::Database;
use turborel::error::Code;

use crate::common::{code, count, exec, rows, sorted};

/// Schema A: domain AMOUNT, INTEGER, default 1, positive; table T of a
/// column K of it and another, N.
fn domains() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE DOMAIN AMOUNT AS INTEGER DEFAULT 1 CONSTRAINT POSITIVE CHECK (VALUE > 0)
           CREATE TABLE T (K AMOUNT, N INTEGER)",
    )
    .unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.4 SR5, §11.5 GR2,
/// §11.21 GR5, GR6] a column of a domain takes its data type and its
/// default, and its constraints hold of the column, VALUE the column's
/// value; its own default comes first.
#[test]
fn a_column_of_a_domain_11_21() {
    let db = domains();
    let a = db.session("A");
    a.run("INSERT INTO T (N) VALUES (7)").unwrap();
    assert_eq!(rows(&a, "SELECT K FROM T"), Ok(vec!["1".into()]), "the domain's default");
    assert_eq!(exec(&a, "INSERT INTO T VALUES (0, 0)").err(), Some(Code::Constraint), "POSITIVE");
    assert_eq!(exec(&a, "INSERT INTO T VALUES ('X', 0)").err(), Some(Code::SyntaxRule), "an INTEGER");
    a.run("CREATE TABLE U (J AMOUNT DEFAULT 5)").unwrap();
    a.run("INSERT INTO U VALUES (DEFAULT)").unwrap();
    assert_eq!(rows(&a, "SELECT J FROM U"), Ok(vec!["5".into()]), "its own default first");
    assert_eq!(exec(&a, "INSERT INTO U VALUES (-1)").err(), Some(Code::Constraint));
    a.run("ALTER TABLE U ADD COLUMN M AMOUNT").unwrap();
    assert_eq!(exec(&a, "UPDATE U SET M = -2").err(), Some(Code::Constraint), "a column added of it");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.21 GR5; §14.2 SR1, GR2b,
/// GR3b; §10.6 GR3] SET CONSTRAINTS names a domain's constraint, and sets
/// it on each column of the domain: deferred, a row may break it; set
/// immediate, it is checked then, and refused while a row breaks it.
#[test]
fn set_constraints_of_a_domain_14_2() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE DOMAIN AMOUNT AS INTEGER CONSTRAINT POSITIVE CHECK (VALUE > 0) DEFERRABLE
           CREATE TABLE T (K AMOUNT)",
    )
    .unwrap();
    let a = db.session("A");
    assert_eq!(exec(&a, "INSERT INTO T VALUES (0)").err(), Some(Code::Constraint), "immediate");
    a.run("SET CONSTRAINTS POSITIVE DEFERRED").unwrap();
    a.run("INSERT INTO T VALUES (0)").unwrap();
    assert_eq!(exec(&a, "SET CONSTRAINTS POSITIVE IMMEDIATE").err(), Some(Code::Constraint));
    a.run("UPDATE T SET K = 1").unwrap();
    a.run("SET CONSTRAINTS POSITIVE IMMEDIATE").unwrap();
    a.commit().unwrap();
    assert_eq!(count(&a, "SELECT K FROM T"), 1);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4 SR8a, §11.21 GR5] a
/// domain constraint's tables are those of the domain's schema, on a
/// column of a table in another schema too.
#[test]
fn a_domain_constraint_in_another_schema_11_21() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER)
           CREATE DOMAIN LISTED AS INTEGER CHECK (VALUE IN (SELECT K FROM T))
           GRANT USAGE ON DOMAIN LISTED TO B
           GRANT REFERENCES ON T TO B",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1)").unwrap();
    a.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (K INTEGER) CREATE TABLE U (J A.LISTED)").unwrap();
    let b = db.session("B");
    b.run("INSERT INTO U VALUES (1)").unwrap();
    assert_eq!(exec(&b, "INSERT INTO U VALUES (2)").err(), Some(Code::Constraint), "of A.T, not B.T");
    b.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.22–§11.26] ALTER DOMAIN
/// sets and drops its default, each column without its own taking the
/// default dropped; adds a constraint, which the rows there are must
/// satisfy, and drops one.
#[test]
fn alter_domain_11_22() {
    let db = domains();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (5, 0)").unwrap();
    a.run("ALTER DOMAIN AMOUNT SET DEFAULT 2").unwrap();
    a.run("INSERT INTO T (N) VALUES (1)").unwrap();
    assert_eq!(sorted(&a, "SELECT K FROM T"), ["2", "5"]);
    a.run("ALTER DOMAIN AMOUNT DROP DEFAULT").unwrap();
    a.run("INSERT INTO T (N) VALUES (2)").unwrap();
    assert_eq!(sorted(&a, "SELECT K FROM T"), ["2", "2", "5"], "the default dropped stays the column's");
    assert_eq!(exec(&a, "ALTER DOMAIN AMOUNT DROP DEFAULT").err(), Some(Code::SyntaxRule), "§11.24 SR2: none now");
    assert_eq!(exec(&a, "ALTER DOMAIN AMOUNT ADD CONSTRAINT SMALL CHECK (VALUE < 4)").err(), Some(Code::Constraint), "5 is there");
    a.run("ALTER DOMAIN AMOUNT ADD CONSTRAINT SMALL CHECK (VALUE < 9)").unwrap();
    assert_eq!(exec(&a, "INSERT INTO T VALUES (9, 0)").err(), Some(Code::Constraint));
    a.run("ALTER DOMAIN AMOUNT DROP CONSTRAINT SMALL").unwrap();
    a.run("INSERT INTO T VALUES (9, 0)").unwrap();
    a.run("ALTER DOMAIN AMOUNT DROP CONSTRAINT POSITIVE").unwrap();
    a.run("INSERT INTO T VALUES (-9, 0)").unwrap();
    assert_eq!(exec(&a, "ALTER DOMAIN AMOUNT DROP CONSTRAINT NOPE").err(), Some(Code::SyntaxRule), "§11.26 SR3");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.27 SR2, GR1] DROP DOMAIN
/// RESTRICT refuses a domain a column is of; CASCADE leaves the column its
/// data type and default, and the constraint as the table's own.
#[test]
fn drop_domain_11_27() {
    let db = domains();
    let a = db.session("A");
    assert_eq!(exec(&a, "DROP DOMAIN AMOUNT RESTRICT").err(), Some(Code::SyntaxRule), "T.K is of it");
    a.run("DROP DOMAIN AMOUNT CASCADE").unwrap();
    a.run("INSERT INTO T (N) VALUES (3)").unwrap();
    assert_eq!(rows(&a, "SELECT K FROM T"), Ok(vec!["1".into()]), "its default kept");
    assert_eq!(exec(&a, "INSERT INTO T VALUES (0, 0)").err(), Some(Code::Constraint), "its constraint kept");
    assert_eq!(exec(&a, "CREATE TABLE V (K AMOUNT)").err(), Some(Code::SyntaxRule), "gone");
    a.commit().unwrap();
    a.run("CREATE DOMAIN UNUSED AS CHAR(2)").unwrap();
    a.run("DROP DOMAIN UNUSED RESTRICT").unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.10 SR1, AR1; §10.3; §11.4
/// AR1; §11.36, §11.37] CAST to a domain takes its data type; a column of
/// it, and a CAST to it, need USAGE, which its schema's owner holds and
/// grants; revoked, it is gone; USAGE is granted on a domain alone.
#[test]
fn usage_on_a_domain_10_3() {
    let db = domains();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let (a, b) = (db.session("A"), db.session("B"));
    assert_eq!(rows(&a, "SELECT CAST(N AS AMOUNT) + 1 FROM T WHERE 1 = 0"), Ok(vec![]));
    assert_eq!(exec(&b, "CREATE TABLE U (K A.AMOUNT)").err(), Some(Code::Privilege), "no USAGE");
    a.run("GRANT USAGE ON DOMAIN AMOUNT TO B").unwrap();
    a.commit().unwrap();
    b.run("CREATE TABLE U (K A.AMOUNT)").unwrap();
    assert_eq!(exec(&b, "INSERT INTO U VALUES (0)").err(), Some(Code::Constraint), "A's constraint holds of B's column");
    b.commit().unwrap();
    assert_eq!(exec(&a, "GRANT SELECT ON DOMAIN AMOUNT TO B").err(), Some(Code::SyntaxRule), "§11.36 SR1");
    assert_eq!(exec(&a, "GRANT USAGE ON T TO B").err(), Some(Code::SyntaxRule), "§11.36 SR1");
    a.run("REVOKE USAGE ON DOMAIN AMOUNT FROM B RESTRICT").unwrap();
    a.commit().unwrap();
    assert_eq!(exec(&b, "CREATE TABLE W (K A.AMOUNT)").err(), Some(Code::Privilege), "revoked");
    assert_eq!(count(&b, "SELECT K FROM U"), 0, "a column of it stays");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.10.3, §6.10 GR13] a value
/// CAST to a domain is refused (SQLCODE -201) if a constraint of the
/// domain is false of it, one that reads a table and one added by ALTER
/// DOMAIN among them; one not false, the null value too, is its value.
#[test]
fn cast_to_a_domain_checks_its_constraints_6_10() {
    let db = domains();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (2, 2)").unwrap();
    assert_eq!(rows(&a, "SELECT CAST(0 AS AMOUNT) FROM T").err(), Some(Code::Constraint), "POSITIVE");
    assert_eq!(rows(&a, "SELECT CAST(N - 5 AS AMOUNT) FROM T").err(), Some(Code::Constraint), "of a column");
    assert_eq!(rows(&a, "SELECT CAST(N AS AMOUNT) FROM T"), Ok(vec!["2".into()]));
    assert_eq!(rows(&a, "SELECT CAST(NULL AS AMOUNT) FROM T"), Ok(vec!["NULL".into()]), "unknown is not false");
    a.run("CREATE DOMAIN KNOWN AS INTEGER CHECK (VALUE IN (SELECT N FROM T))").unwrap();
    assert_eq!(rows(&a, "SELECT CAST(2 AS KNOWN) FROM T"), Ok(vec!["2".into()]));
    assert_eq!(rows(&a, "SELECT CAST(3 AS KNOWN) FROM T").err(), Some(Code::Constraint), "reading T");
    a.run("ALTER DOMAIN AMOUNT ADD CONSTRAINT SMALL CHECK (VALUE < 10)").unwrap();
    assert_eq!(rows(&a, "SELECT CAST(10 AS AMOUNT) FROM T").err(), Some(Code::Constraint), "SMALL");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.21 SR1, §6.2 SR5, §10.6
/// SR2] refused (SQLCODE -103): a domain of a name taken, VALUE outside a
/// domain constraint, a constraint of a name taken; a domain is defined
/// by its schema's owner (SQLCODE -104).
#[test]
fn what_create_domain_refuses_11_21() {
    let db = domains();
    let a = db.session("A");
    assert_eq!(exec(&a, "CREATE DOMAIN AMOUNT AS INTEGER").err(), Some(Code::SyntaxRule), "SR1");
    assert_eq!(exec(&a, "INSERT INTO T VALUES (VALUE, 1)").err(), Some(Code::SyntaxRule), "§6.2 SR5");
    assert_eq!(exec(&a, "CREATE DOMAIN D2 AS INTEGER CONSTRAINT POSITIVE CHECK (VALUE > 0)").err(), Some(Code::SyntaxRule), "§10.6 SR2");
    let c = db.session("C");
    c.run("SET SCHEMA 'A'").unwrap();
    assert_eq!(exec(&c, "CREATE DOMAIN D3 AS INTEGER").err(), Some(Code::Privilege), "§11.21 AR1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.2.5, §21.2.6, §21.2.9,
/// §21.2.12, §21.3.5] the Information Schema gives each domain, its data
/// type and default, its constraints, the domain of each column of it,
/// and USAGE on it; a copy of a domain constraint is no table constraint.
#[test]
fn domains_in_the_information_schema_21_2() {
    let db = domains();
    let a = db.session("A");
    let squeeze = |sql: &str| rows(&a, sql).unwrap().iter().map(|r| r.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>();
    assert_eq!(
        squeeze("SELECT DOMAIN_NAME, DATA_TYPE, NUMERIC_PRECISION, DOMAIN_DEFAULT FROM INFORMATION_SCHEMA.DOMAINS"),
        ["AMOUNT,INTEGER,10,1"]
    );
    assert_eq!(squeeze("SELECT CONSTRAINT_NAME, DOMAIN_NAME FROM INFORMATION_SCHEMA.DOMAIN_CONSTRAINTS"), ["POSITIVE,AMOUNT"]);
    let columns = "SELECT COLUMN_NAME, DOMAIN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' ORDER BY COLUMN_NAME";
    assert_eq!(squeeze(columns), ["K,AMOUNT,INTEGER", "N,NULL,INTEGER"]);
    assert_eq!(squeeze("SELECT CHECK_CLAUSE FROM INFORMATION_SCHEMA.CHECK_CONSTRAINTS WHERE CONSTRAINT_NAME = 'POSITIVE'"), ["VALUE > 0"]);
    assert_eq!(count(&a, "SELECT CONSTRAINT_NAME FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE TABLE_NAME = 'T'"), 0);
    assert_eq!(
        squeeze("SELECT OBJECT_NAME, OBJECT_TYPE FROM INFORMATION_SCHEMA.USAGE_PRIVILEGES WHERE OBJECT_NAME = 'AMOUNT'"),
        ["AMOUNT,DOMAIN"]
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.27 SR2, GR2; §11.37 SR13,
/// SR18] a view that CASTs to a domain keeps it from being dropped with
/// RESTRICT, and goes with it under CASCADE; one of a schema whose owner
/// loses USAGE on it keeps the revoke from RESTRICT, and goes under
/// CASCADE.
#[test]
fn views_that_cast_to_a_domain_11_27() {
    let db = domains();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let (a, b) = (db.session("A"), db.session("B"));
    a.run("GRANT USAGE ON DOMAIN AMOUNT TO B").unwrap();
    a.run("GRANT SELECT ON T TO B").unwrap();
    a.commit().unwrap();
    b.run("CREATE VIEW W AS SELECT CAST(N AS A.AMOUNT) AS M FROM A.T").unwrap();
    b.commit().unwrap();
    assert_eq!(exec(&a, "REVOKE USAGE ON DOMAIN AMOUNT FROM B RESTRICT").err(), Some(Code::SyntaxRule), "W is abandoned");
    a.run("REVOKE USAGE ON DOMAIN AMOUNT FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(code(&b, "SELECT M FROM W"), Code::SyntaxRule, "W went");
    a.run("CREATE VIEW V AS SELECT CAST(N AS AMOUNT) AS M FROM T").unwrap();
    a.run("ALTER TABLE T DROP COLUMN K CASCADE").unwrap();
    assert_eq!(exec(&a, "DROP DOMAIN AMOUNT RESTRICT").err(), Some(Code::SyntaxRule), "V casts to it");
    a.run("DROP DOMAIN AMOUNT CASCADE").unwrap();
    assert_eq!(code(&a, "SELECT M FROM V"), Code::SyntaxRule, "V went");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.27 SR2, GR2; §11.37
/// SR15, GR5] an assertion whose search condition CASTs to a domain keeps
/// it from being dropped with RESTRICT, and goes with it under CASCADE.
#[test]
fn assertions_that_cast_to_a_domain_11_27() {
    let db = domains();
    let a = db.session("A");
    a.run("ALTER TABLE T DROP COLUMN K CASCADE").unwrap();
    a.run("CREATE ASSERTION SMALL CHECK (NOT EXISTS (SELECT * FROM T WHERE CAST(N AS AMOUNT) > 100))").unwrap();
    a.commit().unwrap();
    assert_eq!(exec(&a, "DROP DOMAIN AMOUNT RESTRICT").err(), Some(Code::SyntaxRule), "SMALL casts to it");
    a.run("DROP DOMAIN AMOUNT CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(count(&a, "SELECT * FROM INFORMATION_SCHEMA.ASSERTIONS"), 0, "SMALL went");
    a.run("INSERT INTO T VALUES (500)").unwrap();
}

/// Verifies: [DESIGN] RULES.md D25: domains, a column's domain, and their
/// changes are logged and kept by a checkpoint: as they were after a crash
/// and after a clean close.
#[test]
fn domains_are_kept() {
    let open = |dir: &std::path::Path| {
        let db = Database::new();
        db.open_files(&crate::common::os(), dir, &turborel::db::OpenOptions::default()).unwrap();
        db
    };
    let scratch = crate::common::Scratch::new("domains");
    let dir = scratch.path("db");
    let db = open(&dir);
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE DOMAIN AMOUNT AS INTEGER CHECK (VALUE > 0) CREATE TABLE T (K AMOUNT)").unwrap();
    let a = db.session("A");
    a.run("ALTER DOMAIN AMOUNT SET DEFAULT 3").unwrap();
    a.commit().unwrap();
    drop((a, db));
    let kept = |db: &Database| {
        let a = db.session("A");
        a.run("INSERT INTO T VALUES (DEFAULT)").unwrap();
        assert_eq!(rows(&a, "SELECT K FROM T"), Ok(vec!["3".into()]));
        assert_eq!(exec(&a, "INSERT INTO T VALUES (0)").err(), Some(Code::Constraint));
        a.rollback().unwrap();
    };
    let db = open(&dir);
    kept(&db);
    db.close().unwrap();
    drop(db);
    kept(&open(&dir));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.21 GR4] a domain's owner
/// holds USAGE on it grantable only while it holds grantable REFERENCES on
/// each column its constraints reference, and grantable USAGE on each
/// domain they CAST to, its own or another's: without, a grant gives a
/// warning (SQLSTATE 01007) and grants nothing, and USAGE_PRIVILEGES says
/// the owner's is not grantable; with, both as for any domain.
#[test]
fn usage_grantable_as_its_constraints_reference_11_21() {
    let db = domains();
    for s in ["B", "C"] {
        db.schema(&format!("CREATE SCHEMA AUTHORIZATION {s}")).unwrap();
    }
    let (a, b) = (db.session("A"), db.session("B"));
    a.run("GRANT REFERENCES (N) ON T TO B").unwrap();
    a.commit().unwrap();
    b.run("CREATE DOMAIN LISTED AS INTEGER CHECK (VALUE IN (SELECT N FROM A.T))").unwrap();
    b.run("CREATE DOMAIN TWICE AS INTEGER CHECK (CAST(VALUE AS LISTED) > 0)").unwrap();
    b.commit().unwrap();
    let grantable = "SELECT OBJECT_NAME, IS_GRANTABLE FROM INFORMATION_SCHEMA.USAGE_PRIVILEGES
                     WHERE OBJECT_TYPE = 'DOMAIN' AND GRANTOR = '_SYSTEM' ORDER BY OBJECT_NAME";
    let owners = |listed: &str, twice: &str| Ok(vec![format!("LISTED,{listed}"), format!("TWICE,{twice}")]);
    assert_eq!(b.run("GRANT USAGE ON DOMAIN LISTED TO C").unwrap().sqlstate().as_str(), "01007", "B may not grant it");
    b.commit().unwrap();
    let c = db.session("C");
    assert_eq!(exec(&c, "CREATE TABLE X (K B.LISTED)").err(), Some(Code::Privilege), "C holds none");
    c.rollback().unwrap();
    a.run("GRANT REFERENCES (N) ON T TO B WITH GRANT OPTION").unwrap();
    a.commit().unwrap();
    b.run("GRANT USAGE ON DOMAIN LISTED TO C").unwrap();
    b.commit().unwrap();
    c.run("CREATE DOMAIN THEIRS AS INTEGER CHECK (CAST(VALUE AS B.LISTED) > 0)").unwrap();
    c.commit().unwrap();
    assert_eq!(rows(&b, grantable), owners("YES", "YES"));
    assert_eq!(rows(&c, grantable), Ok(vec!["THEIRS,NO".into()]), "C's USAGE on LISTED is not grantable");
    a.run("REVOKE GRANT OPTION FOR REFERENCES (N) ON T FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&b, grantable), owners("NO", "NO"), "LISTED's REFERENCES no longer grantable");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.36 GR14, GR15; §11.37 SR7,
/// SR18, GR8, GR9] USAGE is granted on with grant option and passed on; a
/// holder without the grant option grants nothing, with a warning (SQLSTATE
/// 01007); revoking what was not granted warns (01006); revoking the
/// grant option a grant depends on is refused with RESTRICT, and takes
/// that grant under CASCADE; GRANT OPTION FOR keeps USAGE.
#[test]
fn usage_with_grant_option_11_37() {
    let db = domains();
    for s in ["B", "C"] {
        db.schema(&format!("CREATE SCHEMA AUTHORIZATION {s}")).unwrap();
    }
    let (a, b) = (db.session("A"), db.session("B"));
    a.run("GRANT USAGE ON DOMAIN AMOUNT TO B WITH GRANT OPTION").unwrap();
    a.commit().unwrap();
    b.run("GRANT USAGE ON DOMAIN A.AMOUNT TO C").unwrap();
    b.commit().unwrap();
    let c = db.session("C");
    assert_eq!(c.run("GRANT USAGE ON DOMAIN A.AMOUNT TO D").unwrap().sqlstate().as_str(), "01007", "C has no grant option");
    c.rollback().unwrap();
    assert_eq!(a.run("REVOKE USAGE ON DOMAIN AMOUNT FROM C RESTRICT").unwrap().sqlstate().as_str(), "01006", "A granted C none");
    a.rollback().unwrap();
    assert_eq!(
        exec(&a, "REVOKE GRANT OPTION FOR USAGE ON DOMAIN AMOUNT FROM B RESTRICT").err(),
        Some(Code::SyntaxRule),
        "C's grant hangs on it"
    );
    a.rollback().unwrap();
    a.run("REVOKE GRANT OPTION FOR USAGE ON DOMAIN AMOUNT FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(exec(&c, "CREATE TABLE X (K A.AMOUNT)").err(), Some(Code::Privilege), "C's went");
    c.rollback().unwrap();
    b.run("CREATE TABLE X (K A.AMOUNT)").unwrap();
    b.commit().unwrap();
    assert_eq!(exec(&c, "GRANT USAGE ON DOMAIN A.NOPE TO D").err(), Some(Code::SyntaxRule), "no such domain");
    assert_eq!(exec(&c, "GRANT USAGE ON DOMAIN A.AMOUNT TO D").err(), Some(Code::Privilege), "C holds none");
    c.rollback().unwrap();
    assert_eq!(exec(&a, "REVOKE SELECT ON DOMAIN AMOUNT FROM B RESTRICT").err(), Some(Code::SyntaxRule), "§11.37 SR1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.21–§11.27] refused: a
/// domain of no schema, a constraint of it named in another's schema, a
/// domain constraint not a CHECK, ALTER and DROP of a domain not there or
/// another's, a default not of its type, a table's CHECK that CASTs to it
/// on DROP DOMAIN RESTRICT; a constraint that CASTs to it goes with it
/// under CASCADE.
#[test]
fn what_domains_refuse_11_22() {
    let db = domains();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let (a, b) = (db.session("A"), db.session("B"));
    let refused = [
        ("CREATE DOMAIN D AS INTEGER CONSTRAINT B.X CHECK (VALUE > 0)", Code::Privilege),
        ("CREATE DOMAIN D AS INTEGER CONSTRAINT X UNIQUE (VALUE)", Code::Syntax),
        ("CREATE DOMAIN D AS INTEGER DEFAULT 'X'", Code::SyntaxRule),
        ("ALTER DOMAIN NOPE SET DEFAULT 1", Code::SyntaxRule),
        ("ALTER DOMAIN AMOUNT SET DEFAULT 'X'", Code::SyntaxRule),
        ("ALTER DOMAIN AMOUNT ADD CHECK (VALUE IN (SELECT K FROM NOPE))", Code::SyntaxRule),
        ("DROP DOMAIN NOPE CASCADE", Code::SyntaxRule),
    ];
    for (sql, code) in refused {
        assert_eq!(exec(&a, sql).err(), Some(code), "{sql}");
    }
    assert_eq!(exec(&b, "ALTER DOMAIN A.AMOUNT DROP DEFAULT").err(), Some(Code::Privilege), "§11.22 AR1");
    assert_eq!(exec(&b, "DROP DOMAIN A.AMOUNT CASCADE").err(), Some(Code::Privilege), "§11.27 AR1");
    let nowhere = db.session("Z");
    assert_eq!(exec(&nowhere, "CREATE DOMAIN D AS INTEGER").err(), Some(Code::SyntaxRule), "no schema Z");
    a.run("CREATE DOMAIN SMALL AS INTEGER CHECK (CAST(VALUE AS A.AMOUNT) < 10)").unwrap();
    a.run("CREATE TABLE C (K INTEGER, CHECK (CAST(K AS AMOUNT) > 0))").unwrap();
    a.run("ALTER TABLE T DROP COLUMN K CASCADE").unwrap();
    assert_eq!(exec(&a, "DROP DOMAIN AMOUNT RESTRICT").err(), Some(Code::SyntaxRule), "C's CHECK and SMALL's cast to it");
    a.run("DROP DOMAIN AMOUNT CASCADE").unwrap();
    a.run("INSERT INTO C VALUES (-1)").unwrap();
    a.run("CREATE TABLE E (K SMALL)").unwrap();
    a.run("INSERT INTO E VALUES (99)").unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.21 SR4, §11.25 SR2] no domain is in
/// usage by its own constraints: none CASTs to it, nor to a domain one of
/// whose constraints CASTs to it, however far (SQLCODE -103); a domain
/// CAST to by another's constraint is not in usage by its own.
#[test]
fn a_domain_is_in_usage_by_none_of_its_constraints_11_21() {
    let db = domains();
    let own = db.schema("CREATE SCHEMA AUTHORIZATION Z CREATE DOMAIN D AS INTEGER CHECK (CAST (VALUE AS D) > 0)").unwrap_err();
    assert_eq!(own.code, Code::SyntaxRule);
    assert!(own.msg.contains("in usage by its own constraints"), "{}", own.msg);
    let a = db.session("A");
    a.run("CREATE DOMAIN SMALL AS INTEGER CHECK (CAST (VALUE AS AMOUNT) < 10)").unwrap();
    a.run("CREATE DOMAIN TINY AS INTEGER CHECK (CAST (VALUE AS SMALL) < 5)").unwrap();
    a.commit().unwrap();
    for (sql, why) in [
        ("ALTER DOMAIN AMOUNT ADD CHECK (CAST (VALUE AS SMALL) > 0)", "through SMALL's"),
        ("ALTER DOMAIN AMOUNT ADD CHECK (CAST (VALUE AS A.TINY) > 0)", "through TINY's and SMALL's"),
        ("ALTER DOMAIN SMALL ADD CHECK (CAST (VALUE AS SMALL) > 0)", "its own"),
    ] {
        assert_eq!(exec(&a, sql).err(), Some(Code::SyntaxRule), "{why}");
    }
    a.run("ALTER DOMAIN TINY ADD CHECK (CAST (VALUE AS AMOUNT) > 0)").unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.37 SR13-SR16, SR18,
/// GR3-GR6]: what CASTs to a domain in a schema left with no USAGE on it
/// is abandoned: a view, a table's CHECK, another domain's constraint,
/// with its copies on columns, and an assertion. RESTRICT refuses to
/// abandon any (SQLCODE -103); CASCADE drops each, and nothing else.
#[test]
fn revoking_usage_abandons_what_casts_to_the_domain_11_37() {
    let db = domains();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let (a, b) = (db.session("A"), db.session("B"));
    a.run("GRANT USAGE ON DOMAIN AMOUNT TO B").unwrap();
    a.commit().unwrap();
    for sql in [
        "CREATE TABLE U (K INTEGER CHECK (CAST (K AS A.AMOUNT) < 9), CHECK (K < 99))",
        "CREATE DOMAIN WRAP AS INTEGER CONSTRAINT WRAPPED CHECK (CAST (VALUE AS A.AMOUNT) < 9)",
        "CREATE TABLE W (K WRAP)",
        "CREATE VIEW V AS SELECT CAST (K AS A.AMOUNT) AS K FROM U",
        "CREATE ASSERTION SMALL CHECK (NOT EXISTS (SELECT * FROM U WHERE CAST (K AS A.AMOUNT) > 5))",
    ] {
        b.run(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    b.commit().unwrap();
    assert_eq!(exec(&a, "REVOKE USAGE ON DOMAIN AMOUNT FROM B RESTRICT").err(), Some(Code::SyntaxRule), "SR18");
    a.rollback().unwrap();
    a.run("REVOKE USAGE ON DOMAIN AMOUNT FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(code(&b, "SELECT K FROM V"), Code::SyntaxRule, "the view");
    b.rollback().unwrap();
    for sql in ["INSERT INTO U VALUES (50)", "INSERT INTO W VALUES (50)"] {
        b.run(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    assert_eq!(exec(&b, "INSERT INTO U VALUES (100)").err(), Some(Code::Constraint), "U's other CHECK stays");
    b.rollback().unwrap();
    let left = "SELECT CONSTRAINT_NAME FROM INFORMATION_SCHEMA.DOMAIN_CONSTRAINTS WHERE DOMAIN_NAME = 'WRAP'";
    assert_eq!(rows(&b, left), Ok(vec![]), "WRAP's constraint");
    assert_eq!(count(&b, "SELECT * FROM INFORMATION_SCHEMA.ASSERTIONS WHERE CONSTRAINT_NAME = 'SMALL'"), 0, "the assertion");
}
