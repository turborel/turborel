//! The SET statements of a session [ISO/IEC 9075:1992 §14.1, §16.2]: SET
//! TRANSACTION, the modes of the next transaction, and SET SCHEMA, the
//! default schema of unqualified names.

use turborel::ast::{AccessMode, IsolationLevel};
use turborel::db::{Completion, Session, TransactionMode};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Value};

use crate::common::{count, exec, expected, rows, setup};
use turborel::value::Spelled;

/// The SQLSTATE `sql` ends with in `s`.
fn state(s: &Session, sql: &str) -> String {
    match s.run(sql) {
        Ok(done) => done.sqlstate().as_str().to_owned(),
        Err(e) => e.sqlstate().as_str().to_owned(),
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §14.1, §4.28, §13.6-§13.10 GR1,
/// §12.3 GR6c v]: SET TRANSACTION sets the modes of the next transaction
/// alone, and is refused while one is active; a READ ONLY one reads, but
/// changes no data or schema (SQLSTATE 25000); one begun with no SET
/// TRANSACTION before it is READ WRITE and SERIALIZABLE.
#[test]
fn set_transaction_sets_the_next_transaction_14_1() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(exec(&s, "SET TRANSACTION READ ONLY"), Ok(Completion::Success));
    assert_eq!(s.transaction(), None, "not transaction-initiating");
    assert_eq!(count(&s, "SELECT ID FROM T"), 3);
    let read_only = TransactionMode { access: AccessMode::ReadOnly, isolation: IsolationLevel::Serializable };
    assert_eq!(s.transaction_mode(), Some(read_only));
    for change in [
        "INSERT INTO T (ID) VALUES (9)",
        "UPDATE T SET AMT = 1",
        "DELETE FROM T",
        "DROP TABLE U CASCADE",
        "ALTER TABLE U ADD COLUMN X INTEGER",
    ] {
        assert_eq!(exec(&s, change), Err(Code::TransactionState), "{change}");
    }
    assert_eq!(state(&s, "DELETE FROM T WHERE ID = 1"), "25000");
    assert_eq!(exec(&s, "SET TRANSACTION READ WRITE"), Err(Code::TransactionState), "§14.1 GR1");
    s.commit().unwrap();
    assert_eq!(exec(&s, "INSERT INTO T (ID) VALUES (9)"), Ok(Completion::Success), "the next is READ WRITE again");
    assert_eq!(s.transaction_mode(), Some(TransactionMode::default()));
    assert_eq!(TransactionMode::default(), TransactionMode { access: AccessMode::ReadWrite, isolation: IsolationLevel::Serializable });
    s.rollback().unwrap();
    assert_eq!(count(&s, "SELECT ID FROM T WHERE ID = 9"), 0);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §14.1 SR1-SR5, GR2, GR5]: the
/// modes each written once; READ UNCOMMITTED implies READ ONLY and is not
/// with READ WRITE; every level is taken, each run as SERIALIZABLE; a
/// `<diagnostics size>` is exact with scale 0, and at least 1 (SQLSTATE
/// 35000).
#[test]
fn transaction_modes_are_checked_14_1() {
    let db = setup();
    let s = db.session("APP");
    let mode = |sql: &str| {
        s.run(sql).unwrap();
        count(&s, "SELECT ID FROM T WHERE ID = 1");
        let found = s.transaction_mode();
        s.rollback().unwrap();
        found.unwrap()
    };
    let read = |isolation| TransactionMode { access: AccessMode::ReadOnly, isolation };
    let write = |isolation| TransactionMode { access: AccessMode::ReadWrite, isolation };
    assert_eq!(mode("SET TRANSACTION ISOLATION LEVEL READ UNCOMMITTED"), read(IsolationLevel::ReadUncommitted));
    assert_eq!(mode("SET TRANSACTION ISOLATION LEVEL READ COMMITTED"), write(IsolationLevel::ReadCommitted));
    assert_eq!(mode("SET TRANSACTION READ ONLY, ISOLATION LEVEL REPEATABLE READ"), read(IsolationLevel::RepeatableRead));
    assert_eq!(mode("set transaction isolation level serializable, read write, diagnostics size 5"), write(IsolationLevel::Serializable));
    for refused in [
        "SET TRANSACTION ISOLATION LEVEL READ UNCOMMITTED, READ WRITE",
        "SET TRANSACTION READ ONLY, READ WRITE",
        "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE, ISOLATION LEVEL READ COMMITTED",
        "SET TRANSACTION DIAGNOSTICS SIZE 1, DIAGNOSTICS SIZE 2",
        "SET TRANSACTION DIAGNOSTICS SIZE 1.5",
        "SET TRANSACTION DIAGNOSTICS SIZE 'A'",
    ] {
        assert_eq!(exec(&s, refused), Err(Code::SyntaxRule), "{refused}");
    }
    assert_eq!(exec(&s, "SET TRANSACTION ISOLATION LEVEL SOMETIMES"), Err(Code::Syntax));
    assert_eq!(exec(&s, "SET TRANSACTION"), Err(Code::Syntax));
    assert_eq!(state(&s, "SET TRANSACTION DIAGNOSTICS SIZE 0"), "35000");
    let mut p = Params::new().declare(
        "N",
        DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric },
        Value::Exact(Decimal::new(-2, 0)),
    );
    assert_eq!(s.execute("SET TRANSACTION DIAGNOSTICS SIZE N", &mut p).unwrap_err().sqlstate().as_str(), "35000", "by a parameter");
    assert_eq!(mode("SET TRANSACTION READ WRITE"), write(IsolationLevel::Serializable), "the refused ones changed nothing");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §16.2, §4.30]: SET SCHEMA sets
/// the schema of the unqualified names of the statements after it, the
/// session's authorization identifier's until then: its value without the
/// spaces around it, a `<schema name>`, a regular `<identifier>` in upper
/// case, perhaps qualified by the catalog's name, given by a literal or a
/// parameter. Privileges stay the session's user's, and CURRENT_USER; a
/// view's names stay its owner's. It begins no transaction.
#[test]
fn set_schema_qualifies_unqualified_names_16_2() {
    let db = setup();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION O
           CREATE TABLE T (K INTEGER)
           CREATE VIEW ALL_T AS SELECT K FROM T
           GRANT SELECT ON T TO APP
           GRANT SELECT ON ALL_T TO APP",
    )
    .unwrap();
    db.session("O").run("INSERT INTO T VALUES (7)").unwrap();
    let s = db.session("APP");
    assert_eq!(s.default_schema(), "APP");
    assert_eq!(exec(&s, "SET SCHEMA ' o '"), Ok(Completion::Success));
    assert_eq!(s.transaction(), None);
    assert_eq!(s.default_schema(), "O");
    assert_eq!(rows(&s, "SELECT K FROM T"), expected(&["7"]));
    assert_eq!(rows(&s, "SELECT T.K FROM T WHERE USER = 'APP'"), expected(&["7"]), "the qualifier, and CURRENT_USER");
    assert_eq!(count(&s, "SELECT ID FROM APP.T"), 3);
    assert_eq!(exec(&s, "INSERT INTO T VALUES (8)"), Err(Code::Privilege), "APP's privileges");
    assert_eq!(exec(&s, "DROP TABLE T CASCADE"), Err(Code::Privilege), "O's schema");
    s.rollback().unwrap();
    assert_eq!(rows(&s, "SELECT K FROM ALL_T"), expected(&["7"]), "O's view reads O.T");
    for (sql, sqlstate) in [
        ("SET SCHEMA 'NOT A NAME'", "3F000"),
        ("SET SCHEMA 'SELECT'", "3F000"),
        ("SET SCHEMA ''", "3F000"),
        ("SET SCHEMA 'ELSEWHERE.APP'", "3D000"),
        ("SET SCHEMA 5", "2A000"),
    ] {
        assert_eq!(state(&s, sql), sqlstate, "{sql}");
    }
    assert_eq!(s.default_schema(), "O", "unchanged by the refused ones");
    assert_eq!(exec(&s, "SET SCHEMA 'TURBOREL.APP'"), Ok(Completion::Success));
    assert_eq!(count(&s, "SELECT ID FROM T"), 3);
    let mut p = Params::new().declare("S", DataType::Character { length: 8 }, Value::Char("\"O\"".into()));
    s.execute("SET SCHEMA S", &mut p).unwrap();
    assert_eq!(rows(&s, "SELECT K FROM T"), expected(&["7"]), "a delimited identifier, by a parameter");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §16.1 SR1, GR2; §5.4 SR20]:
/// SET CATALOG of the one catalog's name, in any case and padded,
/// succeeds; of another's, or of no identifier, is refused with SQLSTATE
/// 3D000; of a number, as a Syntax Rule broken. It begins no transaction.
#[test]
fn set_catalog_names_the_one_catalog_16_1() {
    let db = setup();
    let s = db.session("APP");
    s.run("SET CATALOG ' turborel '").unwrap();
    assert!(s.transaction().is_none(), "no transaction begun");
    for other in ["SET CATALOG 'OTHER'", "SET CATALOG 'TURBOREL.X'"] {
        let e = s.run(other).unwrap_err();
        assert_eq!((e.code, e.sqlstate().as_str()), (Code::Data, "3D000"), "{other}");
    }
    assert_eq!(exec(&s, "SET CATALOG 1").err(), Some(Code::SyntaxRule), "SR1");
}

/// Verifies: [EXTENSION]; [DESIGN] RULES.md D36: a value a setting does
/// not take is refused (SQLCODE -301), saying what it takes as
/// INFORMATION_SCHEMA.SETTINGS shows it, and the value as SQL writes it;
/// so is a name nothing is registered under, in SET DATABASE.
#[test]
fn a_value_refused_is_told_in_words() {
    let db = turborel::db::Database::new();
    db.create_user("A", b"").unwrap();
    let s = db.session("A");
    let code = s.run("SET DATABASE DEFAULT_ENGINE = 'nope'").err().map(|e| e.code);
    assert_eq!(code, Some(Code::Data), "not the API's misuse");
    let told = |sql: &str| s.execute(sql, &mut Params::new()).unwrap_err().msg;
    assert_eq!(told("SET LOCK_WAIT = 'SOMETIMES'"), "LOCK_WAIT takes ONE OF NO_WAIT, WAIT, not 'SOMETIMES'");
    assert_eq!(told("SET WORK_MEMORY = 'big'"), "WORK_MEMORY takes INTEGER FROM 65536 TO 9223372036854775807, not 'big'");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1 GR6b iii, iv]: a
/// `<schema definition>` is an SQL-schema statement, so it is refused in a
/// READ ONLY transaction (SQLCODE -917), the one active or the one SET
/// TRANSACTION makes next; in one READ WRITE, it is made.
#[test]
fn a_read_only_transaction_defines_no_schema_20_1() {
    let db = setup();
    let s = db.session("APP");
    s.run("SET TRANSACTION READ ONLY").unwrap();
    let defined = |sql: &str| s.direct(sql).err().map(|e| e.code);
    assert_eq!(defined("CREATE SCHEMA NEXT_ONE"), Some(Code::TransactionState), "the next");
    s.run("SET TRANSACTION READ ONLY").unwrap();
    rows(&s, "SELECT ID FROM T").unwrap();
    assert_eq!(defined("CREATE SCHEMA ACTIVE_ONE"), Some(Code::TransactionState), "the active");
    s.rollback().unwrap();
    assert_eq!(defined("CREATE SCHEMA WRITTEN"), None);
}
