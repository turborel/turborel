//! \[EXTENSION\] SQL-schema statements outside a `<schema definition>`
//! [ISO/IEC 9075:1992 §20.1], in transactions (RULES.md D33, D47): CREATE
//! TABLE [§11.3], CREATE VIEW [§11.19] and GRANT [§11.36]; REVOKE is
//! `revoke.rs`'s.

use std::path::Path;

use turborel::db::{Completion, Database, Direct, OpenOptions};
use turborel::error::{Code, SqlState};

use crate::common::{Scratch, after_crash, code, exec, rows};

/// Schemas A, with table T holding 1 and 2, and B, empty; C has none.
fn schemas() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, N CHAR(4))").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 'one')").unwrap();
    a.run("INSERT INTO T VALUES (2, 'two')").unwrap();
    a.commit().unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.3 GR1, GR3, GR4, §4.28]; [DESIGN]
/// RULES.md D33, D47: CREATE TABLE on its own defines a base table in the
/// session's schema, its constraints and all: its transaction writes and
/// reads it at once, others see it once it commits, and a ROLLBACK forgets
/// it; its owner holds every privilege on it.
#[test]
fn a_table_is_created_in_a_transaction_11_3() {
    let db = schemas();
    let a = db.session("A");
    let created = "CREATE TABLE U (K INTEGER NOT NULL UNIQUE, T INTEGER REFERENCES T, CHECK (K > 0))";
    assert_eq!(exec(&a, created), Ok(Completion::Success));
    assert_eq!(exec(&a, "INSERT INTO U VALUES (1, 1)"), Ok(Completion::Success));
    assert_eq!(exec(&a, "INSERT INTO U VALUES (1, 2)"), Err(Code::Constraint), "UNIQUE");
    assert_eq!(exec(&a, "INSERT INTO U VALUES (2, 9)"), Err(Code::Constraint), "REFERENCES");
    assert_eq!(exec(&a, "INSERT INTO U VALUES (0, 1)"), Err(Code::Constraint), "CHECK");
    assert_eq!(rows(&a, "SELECT K, T FROM U"), Ok(vec!["1,1".into()]));
    let other = db.session("A");
    assert_eq!(code(&other, "SELECT K FROM U"), Code::SyntaxRule, "not yet committed");
    a.commit().unwrap();
    assert_eq!(code(&other, "SELECT K FROM U"), Code::Conflict, "its snapshot predates U's rows");
    other.rollback().unwrap();
    assert_eq!(rows(&other, "SELECT K, T FROM U"), Ok(vec!["1,1".into()]));
    assert_eq!(exec(&a, "CREATE TABLE GONE (X INTEGER)"), Ok(Completion::Success));
    a.rollback().unwrap();
    assert_eq!(code(&a, "SELECT X FROM GONE"), Code::SyntaxRule, "rolled back");
    let names = rows(&a, "SELECT TRIM(TABLE_NAME) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'A' ORDER BY 1");
    assert_eq!(names, Ok(vec!["T".into(), "U".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.3 SR2, AR1, §11.19 SR3, AR1,
/// §11.8 AR1, §5.4 SR8b, §12.3 GR6c v]; [DESIGN] RULES.md D47: a table or
/// view is defined in a schema that exists, by its owner, under a name no
/// table or view of it has, its constraints' names new too; not in a READ
/// ONLY transaction.
#[test]
fn what_a_definition_needs_11_3() {
    let db = schemas();
    let a = db.session("A");
    assert_eq!(exec(&a, "CREATE TABLE T (X INTEGER)"), Err(Code::SyntaxRule), "SR2: a table T");
    assert_eq!(exec(&a, "CREATE VIEW T AS SELECT K FROM T"), Err(Code::SyntaxRule), "a table T");
    assert_eq!(exec(&a, "CREATE TABLE U (X INTEGER, CONSTRAINT T_PRIMARY1 CHECK (X > 0))"), Err(Code::SyntaxRule), "a constraint's name");
    assert_eq!(exec(&a, "CREATE TABLE B.U (X INTEGER)"), Err(Code::Privilege), "in another's schema");
    let c = db.session("C").execute("CREATE TABLE U (X INTEGER)", &mut turborel::params::Params::new()).unwrap_err();
    assert_eq!((c.code, c.sqlstate()), (Code::SyntaxRule, SqlState::INVALID_SCHEMA_NAME), "no schema C");
    let b = db.session("B");
    assert_eq!(exec(&b, "CREATE TABLE U (X INTEGER REFERENCES A.T)"), Err(Code::Privilege), "§11.8 AR1");
    assert_eq!(exec(&b, "CREATE VIEW V AS SELECT K FROM A.T"), Err(Code::Privilege), "SELECT on A.T");
    b.run("SET SCHEMA 'A'").unwrap();
    assert_eq!(exec(&b, "CREATE TABLE U (X INTEGER)"), Err(Code::Privilege), "AR1: A's schema");
    b.run("SET SCHEMA 'B'").unwrap();
    b.rollback().unwrap();
    b.run("SET TRANSACTION READ ONLY").unwrap();
    assert_eq!(exec(&b, "CREATE TABLE U (X INTEGER)"), Err(Code::TransactionState));
    b.rollback().unwrap();
    assert_eq!(exec(&b, "CREATE TABLE U (X INTEGER)"), Ok(Completion::Success), "B's own");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.19 GR1-GR3]; [DESIGN]
/// RULES.md D47: CREATE VIEW on its own defines a view in the session's
/// schema, over what its owner may read; it is the transaction's until it
/// commits; and GRANT on its own gives others what its grantor holds with
/// grant option, `TABLE` written or not.
#[test]
fn views_and_grants_on_their_own_11_19_11_36() {
    let db = schemas();
    let a = db.session("A");
    assert_eq!(exec(&a, "CREATE VIEW ODD AS SELECT K, N FROM T WHERE K = 1"), Ok(Completion::Success));
    assert_eq!(exec(&a, "GRANT SELECT ON TABLE ODD TO B WITH GRANT OPTION"), Ok(Completion::Success));
    assert_eq!(exec(&a, "GRANT INSERT ON T TO B"), Ok(Completion::Success));
    assert_eq!(rows(&a, "SELECT N FROM ODD"), Ok(vec!["one ".into()]));
    a.commit().unwrap();
    let b = db.session("B");
    assert_eq!(rows(&b, "SELECT N FROM A.ODD"), Ok(vec!["one ".into()]));
    assert_eq!(code(&b, "SELECT N FROM A.T"), Code::Privilege);
    assert_eq!(exec(&b, "INSERT INTO A.T VALUES (3, 'b')"), Ok(Completion::Success));
    assert_eq!(exec(&b, "CREATE VIEW MINE AS SELECT N FROM A.ODD"), Ok(Completion::Success));
    assert_eq!(exec(&b, "GRANT SELECT ON MINE TO C"), Ok(Completion::Success), "its SELECT on A.ODD is grantable");
    assert_eq!(exec(&b, "GRANT SELECT ON NOPE TO C"), Err(Code::SyntaxRule));
    b.commit().unwrap();
    assert_eq!(rows(&db.session("C"), "SELECT N FROM B.MINE"), Ok(vec!["one ".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.36 GR12, GR13, §21.2.10]
/// a privilege granted twice is held once, grantable if either grant
/// said WITH GRANT OPTION, as TABLE_PRIVILEGES shows it.
#[test]
fn a_privilege_granted_twice_is_one_11_36() {
    let db = schemas();
    let a = db.session("A");
    a.run("GRANT SELECT ON T TO B").unwrap();
    a.run("GRANT SELECT, DELETE ON T TO B WITH GRANT OPTION").unwrap();
    a.run("GRANT DELETE ON T TO B").unwrap();
    a.commit().unwrap();
    let held =
        rows(&a, "SELECT TRIM(PRIVILEGE_TYPE), IS_GRANTABLE FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES WHERE GRANTEE = 'B' ORDER BY 1");
    assert_eq!(held, Ok(vec!["DELETE,YES".into(), "SELECT,YES".into()]));
}

/// Verifies: [DESIGN] RULES.md D28, D47: two transactions defining one
/// name take turns on its lock: the second is refused while the first
/// holds it (no-wait), and, once the first commits, finds the name taken.
#[test]
fn one_name_is_defined_once_11_3() {
    let db = schemas();
    let (first, second) = (db.session("A"), db.session("A"));
    assert_eq!(exec(&first, "CREATE TABLE U (X INTEGER)"), Ok(Completion::Success));
    assert_eq!(exec(&second, "CREATE VIEW U AS SELECT K FROM T"), Err(Code::Conflict), "the name's lock");
    first.commit().unwrap();
    assert_eq!(exec(&second, "CREATE VIEW U AS SELECT K FROM T"), Err(Code::SyntaxRule), "SR2");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1, §17.6]; [DESIGN] RULES.md
/// D45, D47: the statements are direct SQL statements, and preparable.
#[test]
fn definitions_are_direct_and_preparable_20_1() {
    let db = schemas();
    let a = db.session("A");
    assert!(matches!(a.direct("CREATE TABLE U (X INTEGER)"), Ok(Direct::Done(Completion::Success))));
    let grant = a.prepare("GRANT SELECT ON U TO B").unwrap();
    assert_eq!(a.execute_prepared(&grant, &[]).unwrap(), Completion::Success);
    a.commit().unwrap();
    assert_eq!(rows(&db.session("B"), "SELECT X FROM A.U"), Ok(vec![]));
}

/// The database in `dir`, opened as after a crash.
fn opened(dir: &Path) -> Database {
    let db = Database::new();
    db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D25, D33, D47: the definitions of a
/// transaction are logged and recovered with it: a table created, filled,
/// dropped and created again under its name keeps only the rows of the
/// one that stands; a view and the privileges granted stand; a table a
/// transaction open at the crash created is gone.
#[test]
fn definitions_are_recovered_11_3() {
    let scratch = Scratch::new("schema-statements-recovered");
    let dir = scratch.path("db");
    let crashed = opened(&dir);
    crashed.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = crashed.session("A");
    for statement in ["CREATE TABLE U (X INTEGER NOT NULL UNIQUE)", "INSERT INTO U VALUES (1)", "INSERT INTO U VALUES (2)"] {
        a.run(statement).unwrap();
    }
    a.commit().unwrap();
    for statement in ["DROP TABLE U CASCADE", "CREATE TABLE U (X INTEGER, Y CHAR(2))", "INSERT INTO U VALUES (3, 'c')"] {
        a.run(statement).unwrap();
    }
    a.run("CREATE VIEW V AS SELECT X FROM U").unwrap();
    a.run("GRANT SELECT ON V TO B").unwrap();
    a.commit().unwrap();
    let open = crashed.session("A");
    open.run("CREATE TABLE W (X INTEGER)").unwrap();

    let db = opened(&dir);
    assert_eq!(rows(&db.session("A"), "SELECT X, Y FROM U"), Ok(vec!["3,c ".into()]));
    assert_eq!(rows(&db.session("B"), "SELECT X FROM A.V"), Ok(vec!["3".into()]));
    assert_eq!(code(&db.session("A"), "SELECT X FROM W"), Code::SyntaxRule, "not committed");
    assert_eq!(exec(&db.session("A"), "INSERT INTO U VALUES (1, 'a')"), Ok(Completion::Success), "no UNIQUE of the old U");
    drop((db, crashed, a, open));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.9 GR2, §11.19 GR13,
/// §11.34 GR4]: a definition whose search condition or query expression
/// the Information Schema cannot show whole, being longer than
/// CHARACTER_DATA, completes with a warning: 01009 of a CHECK, a domain's
/// or a table's, added by ALTER TABLE or ALTER DOMAIN too, and of an
/// assertion; 0100A of a view's query; in a `<schema definition>` too. The
/// Information Schema shows it as null. One it shows completes with none.
#[test]
fn a_definition_too_long_to_show_is_warned_of_11_9() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER) CREATE DOMAIN D AS INTEGER").unwrap();
    let list = (0..12_000).map(|k| k.to_string()).collect::<Vec<_>>().join(", ");
    let a = db.session("A");
    let warned = |sql: &str| exec(&a, sql).map(|done| done.sqlstate().as_str().to_owned());
    for (sql, state) in [
        (format!("CREATE TABLE U (K INTEGER CHECK (K IN ({list})))"), "01009"),
        (format!("ALTER TABLE T ADD CHECK (K IN ({list}))"), "01009"),
        (format!("ALTER TABLE T ADD COLUMN L INTEGER CHECK (L IN ({list}))"), "01009"),
        (format!("CREATE DOMAIN E AS INTEGER CHECK (VALUE IN ({list}))"), "01009"),
        (format!("ALTER DOMAIN D ADD CHECK (VALUE IN ({list}))"), "01009"),
        (format!("CREATE ASSERTION SHORT CHECK (NOT EXISTS (SELECT * FROM T WHERE K IN ({list})))"), "01009"),
        (format!("CREATE VIEW V AS SELECT K FROM T WHERE K IN ({list})"), "0100A"),
        ("CREATE TABLE W (K INTEGER CHECK (K > 0))".to_owned(), "00000"),
    ] {
        assert_eq!(warned(&sql), Ok(state.to_owned()), "{}", sql.get(..40).unwrap_or_default());
    }
    a.commit().unwrap();
    let shown = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.CHECK_CONSTRAINTS WHERE CHECK_CLAUSE IS NULL";
    assert_eq!(rows(&a, shown), Ok(vec!["6".into()]), "U's, T's two, E's, D's, and the assertion's");
    // An <in value list> so long binds to a condition of no great depth.
    a.run("DROP ASSERTION SHORT").unwrap();
    a.run("INSERT INTO T VALUES (11999, 11999)").unwrap();
    assert_eq!(rows(&a, "SELECT K FROM V"), Ok(vec!["11999".into()]));
    a.rollback().unwrap();
    let schema = format!("CREATE SCHEMA AUTHORIZATION B CREATE TABLE X (K INTEGER CHECK (K IN ({list})))");
    assert_eq!(db.schema(&schema).unwrap().sqlstate().as_str(), "01009", "in a <schema definition>");
}
