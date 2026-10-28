//! Diagnostics: \[DESIGN\] the positions of errors, an
//! error a statement's rules raise placed at the construct it is about, the
//! innermost one with a position: a column, a table, a predicate, a select
//! list item, a set function, a subquery, a SET clause, an inserted value
//! or column, a column definition, a target; and \[EXTENSION\] the SQLSTATE
//! of each outcome [ISO/IEC 9075:1992 §22.1].

use turborel::db::Database;
use turborel::error::{Code, SqlError, SqlState};
use turborel::params::Params;
use turborel::value::{DataType, Value};

use crate::common::{rows, setup};

/// Where in `sql` the error of running it is placed, and its code.
fn placed(db: &Database, sql: &str) -> (Code, Option<usize>) {
    let s = db.session("APP");
    let error: SqlError = if sql.starts_with("SELECT") && !sql.contains(" INTO ") {
        s.open_cursor(sql).err().unwrap()
    } else {
        let mut p = Params::new().declare("N", DataType::Exact { precision: 9, scale: 0 }, Value::Null);
        s.execute(sql, &mut p).unwrap_err()
    };
    (error.code, error.pos)
}

/// The character offset of the `nth` (from 0) occurrence of `part` in `sql`.
fn offset(sql: &str, part: &str, nth: usize) -> Option<usize> {
    let byte = sql.match_indices(part).nth(nth)?.0;
    Some(sql.get(..byte)?.chars().count())
}

/// Verifies: [DESIGN] error positions: errors of the query rules are placed at
/// what they are about: an unknown or ambiguous column [X3.135-1986 §5.7
/// SR3] at the column, even inside a predicate; an unknown table [§5.4
/// SR5] at its name; a comparison of incomparable types [§5.11 SR1] at the
/// predicate; an arithmetic operator on a string [§5.9 SR3] at its select
/// list item; a set function in WHERE [§5.21 SR3] at the function; a
/// column outside GROUP BY [§5.25 SR7] at the column; a subquery of two
/// columns [§5.24 SR2a] at the subquery; ORDER BY a column not in the
/// result [§8.3 SR6] at the column.
#[test]
fn query_errors_are_placed_at_their_construct() {
    let db = setup();
    let cases: [(&str, &str, usize); 10] = [
        ("SELECT NOPE FROM T", "NOPE", 0),
        ("SELECT ID FROM T WHERE 1 = T.NOPE", "T.NOPE", 0),
        ("SELECT ID FROM T, U", "ID", 0),
        ("SELECT ID FROM NOPE", "NOPE", 0),
        ("SELECT ID FROM T WHERE ID = 1 AND NAME = 1", "NAME", 0),
        ("SELECT ID, NAME + 1 FROM T", "NAME + 1", 0),
        ("SELECT ID FROM T WHERE MAX(ID) > 1", "MAX", 0),
        ("SELECT NAME FROM T GROUP BY ID", "NAME", 0),
        ("SELECT ID FROM T WHERE ID IN (SELECT * FROM U)", "(SELECT", 0),
        ("SELECT ID FROM T ORDER BY NOPE", "NOPE", 0),
    ];
    for (sql, part, nth) in cases {
        let (code, pos) = placed(&db, sql);
        assert_eq!(code, Code::SyntaxRule, "{sql}");
        assert_eq!(pos, offset(sql, part, nth), "{sql}: at {part}");
    }
}

/// Verifies: [DESIGN] error positions: errors of the rules of the statements
/// that change data or define them are placed at what they are about: a
/// SET clause's unknown column [X3.135-1986 §8.12 SR4], an inserted
/// column [§8.7 SR3] or value [§8.7 SR6], the table changed [§5.4 SR5], a
/// column defined twice [§6.3 SR1], a SELECT INTO target [§8.10 SR4].
#[test]
fn statement_errors_are_placed_at_their_construct() {
    let db = setup();
    let cases: [(&str, &str, usize); 6] = [
        ("UPDATE T SET AMT = 1, NOPE = 2", "NOPE", 0),
        ("INSERT INTO T (ID, NOPE) VALUES (1, 2)", "NOPE", 0),
        ("INSERT INTO T VALUES (9, 1, 1, 1)", "1", 0),
        ("DELETE FROM NOPE", "NOPE", 0),
        ("SELECT NAME INTO N FROM T WHERE ID = 1", "N F", 0),
        ("UPDATE T SET NAME = 1", "NAME", 0),
    ];
    for (sql, part, nth) in cases {
        let (code, pos) = placed(&db, sql);
        assert_eq!(code, Code::SyntaxRule, "{sql}");
        assert_eq!(pos, offset(sql, part, nth), "{sql}: at {part}");
    }
    let schema = "CREATE SCHEMA AUTHORIZATION B CREATE TABLE X (A INT, A INT)";
    let error = db.schema(schema).unwrap_err();
    assert_eq!((error.code, error.pos), (Code::SyntaxRule, offset(schema, "A INT", 1)), "the second A");
}

/// Verifies: [EXTENSION] SQLSTATE, [ISO/IEC 9075:1992 §22.1, Table 23]:
/// each outcome has its SQLSTATE: success and no data; a division by zero,
/// an overflow, a LIKE escape not followed by itself, _ or %, a null with
/// no indicator parameter (data exceptions, by subclass); a UNIQUE or NOT
/// NULL constraint; more than one row; a cursor not open; a Syntax Rule in
/// a statement invoked directly [§20.1 GR5]; a conflict undoing a
/// statement (implementation-defined, TB911) and one rolling back a
/// commit (serialization failure).
#[test]
fn outcomes_have_their_sqlstates() {
    let db = setup();
    let s = db.session("APP");
    let mut p = Params::new().declare("N", DataType::Exact { precision: 9, scale: 0 }, Value::Null);
    let mut state = |sql: &str| match s.execute(sql, &mut p) {
        Ok(done) => done.sqlstate().as_str().to_owned(),
        Err(e) => e.sqlstate().as_str().to_owned(),
    };
    let cases = [
        ("UPDATE T SET F = 3 WHERE ID = 2", "00000"),
        ("DELETE FROM T WHERE ID = 99", "02000"),
        ("SELECT ID / 0 INTO N FROM T WHERE ID = 1", "22012"),
        ("SELECT AMT * 99999999999999 * 99999999999 INTO N FROM T WHERE ID = 1", "22003"),
        ("SELECT ID INTO N FROM T WHERE NAME LIKE 'A!' ESCAPE '!'", "22025"),
        ("SELECT AMT INTO N FROM T WHERE ID = 3", "22002"),
        ("INSERT INTO T VALUES (1, 'X', 1, 1)", "23000"),
        ("INSERT INTO U (ID, CODE) VALUES (NULL, 'Z')", "23000"),
        ("SELECT ID INTO N FROM T", "21000"),
        ("FETCH C INTO N", "2A000"),
        ("SELECT NOPE INTO N FROM T", "2A000"),
    ];
    s.declare_cursor("DECLARE D CURSOR FOR SELECT ID FROM T").unwrap();
    for (sql, expected) in cases {
        assert_eq!(state(sql), expected, "{sql}");
    }
    assert_eq!(state("FETCH D INTO N"), "24000", "D is not open");
    s.commit().unwrap();

    let (a, b) = (db.session("APP"), db.session("APP"));
    a.run("UPDATE T SET F = 7 WHERE ID = 1").unwrap();
    assert_eq!(b.run("UPDATE T SET F = 8 WHERE ID = 1").unwrap_err().sqlstate().as_str(), "TB911", "the statement is undone");
    a.commit().unwrap();
    b.rollback().unwrap();
    rows(&a, "SELECT ID FROM U").unwrap();
    rows(&b, "SELECT ID FROM T").unwrap();
    a.run("UPDATE T SET F = 1 WHERE ID = 2").unwrap();
    b.run("UPDATE U SET CODE = 'Q' WHERE CODE = 'A'").unwrap();
    a.commit().unwrap();
    assert_eq!(b.commit().unwrap_err().sqlstate(), SqlState::SERIALIZATION_FAILURE, "rolled back");
}
