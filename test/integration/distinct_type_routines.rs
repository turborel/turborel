//! The functions a distinct type's definition makes [ISO/IEC 9075-2:2023
//! §11.51 GR2b; §10.4, §9.18; RULES.md D69]: FNUDT and FNSDT invoked by
//! name, found on the SQL-path or in the schema named, chosen among those
//! of one name by their parameters' types, and invoked only with EXECUTE.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{code, rows};

/// The rows of `sql` in `s`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// Schema A: MONEY of NUMERIC(10, 2); EUROS of DECIMAL(8, 2) and YEN of
/// INTEGER, whose FNSDT are both AMOUNT; CODE of CHARACTER(3) and LABEL of
/// CHARACTER VARYING(5), whose FNSDT are both TEXT; table T of a value of
/// each of the first three, holding one row; and schema B, empty.
fn overloaded() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TYPE MONEY AS NUMERIC(10, 2)
           CREATE TYPE EUROS AS DECIMAL(8, 2) CAST (DISTINCT AS SOURCE) WITH AMOUNT
           CREATE TYPE YEN AS INTEGER CAST (DISTINCT AS SOURCE) WITH AMOUNT
           CREATE TYPE CODE AS CHARACTER(3) CAST (DISTINCT AS SOURCE) WITH TEXT
           CREATE TYPE LABEL AS CHARACTER VARYING(5) CAST (DISTINCT AS SOURCE) WITH TEXT
           CREATE TABLE T (K INTEGER, P MONEY, E EUROS, Y YEN)",
    )
    .unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 5, 6, 7)").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.51 GR2b; §10.4; §9.18 SR10b] FNUDT
/// makes a value of the type from one assignable to its source, and FNSDT
/// gives the source's value, each of its name alone, found on the
/// SQL-path, the schema of the session's names (ID047), or of its schema
/// written. A function calls nothing else in a schema it names without
/// one, and none is written with its argument missing or another.
#[test]
fn invoked_by_name_10_4() {
    let (_db, a) = overloaded();
    assert_eq!(query(&a, "SELECT MONEY(2), A.MONEY(2.5), \"NUMERIC\"(P), A.\"NUMERIC\"(P) FROM T"), ["2.00,2.50,5.00,5.00"]);
    for sql in [
        "SELECT MONEY(Y) FROM T",
        "SELECT MONEY() FROM T",
        "SELECT MONEY(1, 2) FROM T",
        "SELECT MONEY(P) FROM T",
        "SELECT \"NUMERIC\"(5) FROM T",
    ] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    assert_eq!(code(&a, "SELECT B.MONEY(1) FROM T"), Code::SyntaxRule, "schema B has no function MONEY");
    assert_eq!(code(&a, "SELECT NOPE(1) FROM T"), Code::Syntax, "nothing of the name, and function calls not taken");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR10d, SR10h; §9.7; §9.6] of the
/// functions of one name, the argument's type chooses: AMOUNT of EUROS
/// and of YEN each by its parameter's type, and of MONEY by none. Of TEXT
/// of CHARACTER(3)'s CODE and CHARACTER VARYING(5)'s LABEL, each takes
/// its own type alone, a value of a distinct type assignable to no other.
/// A ? argument of a name of more than one function has no type.
#[test]
fn overloads_chosen_by_type_9_18() {
    let (_db, a) = overloaded();
    assert_eq!(query(&a, "SELECT AMOUNT(E), AMOUNT(Y) FROM T"), ["6.00,7"]);
    assert_eq!(code(&a, "SELECT AMOUNT(P) FROM T"), Code::SyntaxRule);
    assert_eq!(query(&a, "SELECT TEXT(CODE('ab')), TEXT(LABEL('ab')) FROM T"), ["ab ,ab"]);
    assert!(a.prepare("SELECT AMOUNT(?) FROM T").is_err(), "§9.18 SR10h: of two functions");
    let one = a.prepare("SELECT MONEY(?) FROM T").unwrap();
    assert_eq!(one.parameters().len(), 1, "of one function, its parameter's type");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR6, §6.13 AR2, §6.1 AR1; §11.51
/// GR2] a function is invoked, and a CAST to or from a distinct type made,
/// only by one who holds EXECUTE on it, and the type named only by one who
/// holds USAGE on it. MC/DC of the choice: B, holding EXECUTE on AMOUNT of
/// YEN alone, invokes it of a YEN, and of a EUROS, whose function it holds
/// no EXECUTE on, is refused (-104).
#[test]
fn invoked_with_execute_9_18() {
    let (db, a) = overloaded();
    a.run("GRANT SELECT ON T TO B").unwrap();
    a.run("GRANT EXECUTE ON FUNCTION AMOUNT(YEN) TO B").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    assert_eq!(query(&b, "SELECT A.AMOUNT(Y) FROM A.T"), ["7"]);
    for sql in ["SELECT A.AMOUNT(E) FROM A.T", "SELECT A.MONEY(1) FROM A.T", "SELECT CAST(1 AS A.MONEY) FROM A.T"] {
        assert_eq!(code(&b, sql), Code::Privilege, "{sql}");
    }
    a.run("GRANT USAGE ON TYPE MONEY TO B").unwrap();
    a.commit().unwrap();
    assert_eq!(code(&b, "SELECT CAST(P AS NUMERIC(10, 2)) FROM A.T"), Code::Privilege, "§6.13 AR2: EXECUTE on FNSDT");
    a.run("GRANT EXECUTE ON SPECIFIC FUNCTION MONEY_TO_SOURCE TO B").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&b, "SELECT CAST(P AS NUMERIC(10, 2)), A.\"NUMERIC\"(P) FROM A.T"), ["5.00,5.00"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR6b; §11.32] a view or constraint
/// that invokes a function is bound as its owner, who holds EXECUTE on
/// it, whoever reads it.
#[test]
fn invoked_as_the_owner_of_a_view_9_18() {
    let (db, a) = overloaded();
    a.run("CREATE VIEW V AS SELECT K, \"NUMERIC\"(P) AS N FROM T").unwrap();
    a.run("GRANT SELECT ON V TO B").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&db.session("B"), "SELECT N FROM A.V"), ["5.00"]);
}
