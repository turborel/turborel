//! \[EXTENSION\] CHECK constraints whose condition has a subquery [ISO/IEC
//! 9075:1992 §11.9; Full SQL]: what their definition refuses [SR5, SR6,
//! AR1], their evaluation over the whole table whenever it or a table read
//! changes [§11.6 GR3], and DROP TABLE and DROP COLUMN of what they read
//! [§11.15 SR4, §11.18 SR4].

use turborel::db::{Completion, Database};
use turborel::error::Code;

use crate::common::{exec, sorted};

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.9 SR5, SR6, AR1; §7.8
/// SR3; §7.9 SR13; §7.10 SR15]: a CHECK refuses USER and the like, and
/// CURRENT_DATE and the like, in its
/// subqueries too; a subquery that is possibly non-deterministic, by
/// DISTINCT, MAX or MIN of a character string, a character string grouping
/// column, or a set operation but UNION ALL with one, in a view it reads too, of its own schema or another; a table it holds no
/// REFERENCES on, or a column of it it does not; and takes one it holds
/// REFERENCES on.
#[test]
fn what_checks_refuse_11_9() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION O
           CREATE TABLE P (K INTEGER, C CHAR(2))
           CREATE VIEW V AS SELECT DISTINCT C FROM P
           GRANT REFERENCES (K) ON P TO S",
    )
    .unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION Q CREATE TABLE U (K INTEGER, C CHAR(2))").unwrap();
    let cases = [
        ("CHECK (C <> USER)", Code::SyntaxRule),
        ("CHECK (C <> CURRENT_USER)", Code::SyntaxRule),
        ("CHECK (CURRENT_DATE > DATE '2000-01-01')", Code::SyntaxRule),
        ("CHECK (EXISTS (SELECT * FROM T WHERE CURRENT_TIMESTAMP IS NULL))", Code::SyntaxRule),
        ("CHECK (EXISTS (SELECT * FROM T X WHERE X.C = SESSION_USER))", Code::SyntaxRule),
        ("CHECK (C IN (SELECT DISTINCT C FROM T))", Code::SyntaxRule),
        ("CHECK (C = (SELECT MAX(C) FROM T))", Code::SyntaxRule),
        ("CHECK (EXISTS (SELECT K FROM T GROUP BY C HAVING C = 'A'))", Code::SyntaxRule),
        ("CHECK (C IN (SELECT C FROM O.V))", Code::SyntaxRule),
        ("CHECK (C IN (SELECT C FROM T UNION SELECT C FROM T))", Code::SyntaxRule),
        ("CHECK (EXISTS (SELECT K, C FROM T INTERSECT ALL SELECT K, C FROM T))", Code::SyntaxRule),
        ("CHECK (K IN (SELECT K FROM Q.U))", Code::Privilege),
        ("CHECK (EXISTS (SELECT * FROM Q.U))", Code::Privilege),
        ("CHECK (C IN (SELECT C FROM O.P))", Code::Privilege),
    ];
    for (check, expected) in cases {
        let schema = format!("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER, C CHAR(2), {check})");
        assert_eq!(db.schema(&schema).expect_err(check).code, expected, "{check}");
    }
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE T (K INTEGER, C CHAR(2), CHECK (K IN (SELECT K FROM O.P)), CHECK (K = (SELECT MAX(K) FROM T X)),
             CHECK (K IN (SELECT K FROM T UNION SELECT K FROM T)), CHECK (C IN (SELECT C FROM T UNION ALL VALUES 'A')))
           CREATE VIEW W AS SELECT DISTINCT C FROM T",
    )
    .unwrap();
    let in_w = "CREATE SCHEMA AUTHORIZATION R CREATE TABLE M (C CHAR(2), CHECK (C IN (SELECT C FROM S.W)))";
    assert_eq!(db.schema(in_w).expect_err("W is DISTINCT over a character string").code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.6 GR3; §11.16 GR2]: a
/// CHECK with a subquery is false, and its statement refused, when a row
/// of its own table is written, or another table it reads changes, so that
/// a row of its table makes it false; correlated, it is evaluated for each
/// row; added by ALTER TABLE, it holds of the rows there are; it holds
/// with SQLSTATE 23000 on failure, the statement undone.
#[test]
fn checks_read_other_tables_11_6() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE P (K INTEGER)
           CREATE TABLE C (A INTEGER CONSTRAINT IN_P CHECK (A IN (SELECT K FROM P)))
           CREATE TABLE T (K INTEGER, V INTEGER, CHECK (2 >= (SELECT COUNT(*) FROM T X)),
                           CHECK (V >= ALL (SELECT X.V FROM T X WHERE X.K < T.K)))",
    )
    .unwrap();
    let s = db.session("S");
    s.run("INSERT INTO P VALUES (1)").unwrap();
    s.run("INSERT INTO P VALUES (2)").unwrap();
    s.run("INSERT INTO C VALUES (1)").unwrap();
    assert_eq!(exec(&s, "INSERT INTO C VALUES (3)"), Err(Code::Constraint), "3 is not in P");
    s.run("INSERT INTO C VALUES (NULL)").unwrap();
    assert_eq!(exec(&s, "DELETE FROM P WHERE K = 1"), Err(Code::Constraint), "a change to P falsifies it");
    assert_eq!(exec(&s, "UPDATE P SET K = 5 WHERE K = 1"), Err(Code::Constraint));
    s.run("DELETE FROM P WHERE K = 2").unwrap();
    assert_eq!(sorted(&s, "SELECT K FROM P"), vec!["1"], "each refusal undone");

    s.run("INSERT INTO T VALUES (1, 10)").unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (2, 5)"), Err(Code::Constraint), "V falls");
    s.run("INSERT INTO T VALUES (2, 20)").unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (3, 30)"), Err(Code::Constraint), "a third row");
    assert_eq!(exec(&s, "UPDATE T SET V = 15 WHERE K = 1"), Ok(Completion::Success), "still rising");
    assert_eq!(exec(&s, "UPDATE T SET V = 25 WHERE K = 1"), Err(Code::Constraint), "a row it is not about now falsifies it");

    s.run("INSERT INTO P VALUES (7)").unwrap();
    s.run("ALTER TABLE T ADD CONSTRAINT T_IN_P CHECK (K IN (SELECT K FROM P))").unwrap_err();
    s.run("DELETE FROM T WHERE K = 2").unwrap();
    s.run("UPDATE T SET K = 7").unwrap();
    s.run("ALTER TABLE T ADD CONSTRAINT T_IN_P CHECK (K IN (SELECT K FROM P))").unwrap();
    assert_eq!(exec(&s, "DELETE FROM P WHERE K = 7"), Err(Code::Constraint), "the CHECK added holds");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.18 SR4, §11.15 SR4,
/// §11.11 GR2 note]: a table a CHECK of another reads is dropped only with
/// CASCADE, which drops the CHECK; a column it refers to, likewise; one it
/// does not refer to is dropped, the CHECK kept; a column added to a table
/// it reads with the * of a subquery is none of the subquery's.
#[test]
fn what_checks_read_is_dropped_with_them_11_18() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE P (K INTEGER, L INTEGER, M INTEGER)
           CREATE TABLE U (K INTEGER)
           CREATE TABLE C (A INTEGER CHECK (A IN (SELECT K FROM P)), B INTEGER CHECK (B IN (SELECT * FROM U)))",
    )
    .unwrap();
    let s = db.session("S");
    s.run("INSERT INTO P VALUES (1, 1, 1)").unwrap();
    s.run("INSERT INTO U VALUES (4)").unwrap();
    assert_eq!(exec(&s, "ALTER TABLE P DROP COLUMN K RESTRICT"), Err(Code::SyntaxRule), "the CHECK refers to K");
    s.run("ALTER TABLE P DROP COLUMN L RESTRICT").unwrap();
    assert_eq!(exec(&s, "INSERT INTO C VALUES (2, NULL)"), Err(Code::Constraint), "kept");
    assert_eq!(exec(&s, "ALTER TABLE U ADD COLUMN J INTEGER"), Ok(Completion::Success), "B IN (SELECT * FROM U): U's K alone");
    assert_eq!(exec(&s, "INSERT INTO C VALUES (NULL, 4)"), Ok(Completion::Success), "still one column, K");
    s.run("DELETE FROM C WHERE A IS NULL").unwrap();
    assert_eq!(exec(&s, "DROP TABLE P RESTRICT"), Err(Code::SyntaxRule));
    s.run("ALTER TABLE P DROP COLUMN K CASCADE").unwrap();
    s.run("INSERT INTO C VALUES (2, NULL)").unwrap();
    assert_eq!(exec(&s, "INSERT INTO C VALUES (2, 5)"), Err(Code::Constraint), "B's CHECK kept");
    s.run("DROP TABLE U CASCADE").unwrap();
    s.run("INSERT INTO C VALUES (2, 5)").unwrap();
    s.commit().unwrap();
    assert_eq!(sorted(&s, "SELECT A, B FROM C"), vec!["2,5", "2,NULL"], "B IN (SELECT * FROM U) is unknown for NULL; U had 4");
}
