//! Valid SQL-92 this implementation does not take: refused as valid but
//! not implemented (SQLCODE -900, RULES.md D18), of character sets other
//! than SQL_TEXT, collations and translations never (D32), of dynamic SQL
//! in SQL text and names qualified by their catalog not yet; each read in
//! full first, so SQL that breaks a Format is a syntax error still.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{exec, rows, setup};

/// The code `sql` fails with, as a statement run or a query opened.
fn failed(s: &Session, sql: &str) -> Code {
    s.direct(sql).err().map_or_else(|| panic!("{sql} was taken"), |e| e.code)
}

/// The code `sql` fails with as it is prepared.
fn prepared(s: &Session, sql: &str) -> Code {
    s.prepare(sql).err().map_or_else(|| panic!("{sql} was taken"), |e| e.code)
}

/// Verifies: [EXTENSION] RULES.md D18, D32 [ISO/IEC 9075:1992 §5.3, §5.4,
/// §6.1, §6.7, §6.13, §7.7, §10.4, §10.5, §11.1, §11.4, §11.21, §11.28-§11.33,
/// §11.36, §11.37, §13.1, §16.3] a character set other than SQL_TEXT, a
/// collation or a translation, named anywhere SQL-92 allows, is refused as
/// valid but never implemented (SQLCODE -900); each statement written
/// wrong is a syntax error (SQLCODE -102).
#[test]
fn character_sets_and_collations_are_never_implemented() {
    let db = setup();
    let s = db.session("APP");
    for never in [
        "SELECT _LATIN1'abc' FROM T",
        "SELECT _LATIN1\"NAME\" FROM T",
        "CREATE TABLE W (A CHAR(5) CHARACTER SET LATIN1)",
        "CREATE TABLE W (A VARCHAR(5) CHARACTER SET APP.LATIN1 COLLATE Y)",
        "CREATE TABLE W (A CHAR(5) NOT NULL COLLATE Y)",
        "CREATE DOMAIN D2 AS CHAR(3) COLLATE Y",
        "SELECT CONVERT(NAME USING X) FROM T",
        "SELECT TRANSLATE(NAME USING APP.X) FROM T",
        "SELECT NAME FROM T WHERE NAME COLLATE X = 'A'",
        "SELECT NAME, COUNT(*) FROM T GROUP BY NAME COLLATE X",
        "SELECT NAME FROM T ORDER BY NAME COLLATE X DESC",
        "CREATE CHARACTER SET CS AS GET LATIN1 COLLATE CL",
        "CREATE CHARACTER SET CS GET LATIN1 COLLATION FROM TRANSLATION TR THEN COLLATION CL",
        "CREATE COLLATION CL FOR LATIN1 FROM EXTERNAL('x') NO PAD",
        "CREATE COLLATION CL FOR LATIN1 FROM DESC(C2) PAD SPACE",
        "CREATE COLLATION CL FOR LATIN1 FROM DEFAULT",
        "CREATE TRANSLATION TR FOR A TO B FROM IDENTITY",
        "CREATE TRANSLATION TR FOR A TO B FROM EXTERNAL('x')",
        "CREATE TRANSLATION TR FOR A TO B FROM APP.TR2",
        "DROP CHARACTER SET CS",
        "DROP COLLATION CL",
        "DROP TRANSLATION TR",
        "GRANT USAGE ON COLLATION CL TO PUBLIC",
        "GRANT USAGE ON CHARACTER SET CS TO PUBLIC",
        "REVOKE USAGE ON TRANSLATION TR FROM PUBLIC CASCADE",
        "SET NAMES 'LATIN1'",
    ] {
        assert_eq!(failed(&s, never), Code::Unsupported, "{never}");
    }
    for wrong in [
        "CREATE COLLATION CL FOR LATIN1",
        "CREATE TRANSLATION TR FOR A FROM IDENTITY",
        "SELECT CONVERT(NAME) FROM T",
        "SELECT CONVERT FROM T",
        "SELECT NAME COLLATE X FROM T WHERE",
        "CREATE TABLE W (A CHAR(5) CHARACTER SET)",
        "GRANT USAGE ON COLLATION TO PUBLIC",
    ] {
        assert_eq!(failed(&s, wrong), Code::Syntax, "{wrong}");
    }
}

/// Verifies: [EXTENSION] RULES.md D32 [ISO/IEC 9075:1992 §10.4 SR3,
/// §21.3.23] the one character set, SQL_TEXT, named, alone or of
/// INFORMATION_SCHEMA, is taken: of a column, a literal, an identifier and
/// a schema.
#[test]
fn sql_text_is_taken_by_name() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A DEFAULT CHARACTER SET SQL_TEXT CREATE TABLE T (C CHAR(3) CHARACTER SET INFORMATION_SCHEMA.SQL_TEXT)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (_SQL_TEXT'ab')").unwrap();
    assert_eq!(rows(&a, "SELECT _SQL_TEXT\"C\" FROM T WHERE C = _INFORMATION_SCHEMA.SQL_TEXT'ab'").unwrap(), ["ab "]);
    a.run("CREATE TABLE U (D VARCHAR(4) CHARACTER SET SQL_TEXT)").unwrap();
    assert_eq!(exec(&a, "CREATE TABLE W (D NCHAR(4) CHARACTER SET SQL_TEXT)").err(), Some(Code::Syntax), "a national type takes none");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D18 [ISO/IEC 9075:1992 §17.2-§17.20]
/// dynamic SQL in SQL text is refused as valid but not implemented
/// (SQLCODE -900), each statement read in full: written wrong, a syntax
/// error (SQLCODE -102).
#[test]
fn dynamic_sql_in_sql_text_is_refused() {
    let db = setup();
    let s = db.session("APP");
    for refused in [
        "ALLOCATE DESCRIPTOR 'D1' WITH MAX 10",
        "ALLOCATE DESCRIPTOR GLOBAL 'D1'",
        "DEALLOCATE DESCRIPTOR LOCAL 'D1'",
        "GET DESCRIPTOR 'D1' :N = COUNT",
        "GET DESCRIPTOR 'D1' VALUE 1 :T = TYPE, :L = LENGTH, :P = PRECISION",
        "SET DESCRIPTOR 'D1' COUNT = 2",
        "SET DESCRIPTOR GLOBAL 'D1' VALUE 1 TYPE = 1, DATA = 'x'",
        "PREPARE S1 FROM 'SELECT ID FROM T'",
        "PREPARE GLOBAL 'S1' FROM :TEXT",
        "DEALLOCATE PREPARE S1",
        "DESCRIBE S1 USING SQL DESCRIPTOR 'D1'",
        "DESCRIBE INPUT S1 INTO SQL DESCRIPTOR 'D1'",
        "DESCRIBE OUTPUT GLOBAL 'S1' USING SQL DESCRIPTOR 'D1'",
        "EXECUTE S1",
        "EXECUTE S1 INTO :A, :B USING :C",
        "EXECUTE S1 USING SQL DESCRIPTOR 'D1'",
        "EXECUTE IMMEDIATE 'DELETE FROM T'",
        "ALLOCATE 'C1' INSENSITIVE SCROLL CURSOR FOR 'S1'",
        "OPEN C1 USING SQL DESCRIPTOR 'D1'",
        "OPEN GLOBAL 'C1'",
        "FETCH NEXT FROM C1 USING SQL DESCRIPTOR 'D1'",
        "FETCH GLOBAL 'C1' INTO SQL DESCRIPTOR 'D1'",
        "CLOSE GLOBAL 'C1'",
        "DELETE FROM T WHERE CURRENT OF GLOBAL 'C1'",
        "UPDATE T SET ID = 1 WHERE CURRENT OF 'C1'",
    ] {
        assert_eq!(failed(&s, refused), Code::Unsupported, "{refused}");
    }
    for wrong in
        ["PREPARE S1", "EXECUTE", "DESCRIBE S1", "GET DESCRIPTOR 'D1' VALUE 1 :T = COLOUR", "ALLOCATE 'C1' CURSOR", "FETCH GLOBAL 'C1'"]
    {
        assert_eq!(failed(&s, wrong), Code::Syntax, "{wrong}");
    }
    for refused in ["DELETE WHERE CURRENT OF C1", "UPDATE SET ID = 1 WHERE CURRENT OF C1"] {
        assert_eq!(prepared(&s, refused), Code::Unsupported, "{refused}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1, §7.10, §6.7, §5.4] a
/// direct statement may end in one semicolon; a joined table is a query on
/// its own; TRIM takes FROM alone; a name qualified by its catalog is
/// valid but not implemented (SQLCODE -900).
#[test]
fn smaller_parts_of_sql_92() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID = 1;").unwrap(), ["1"]);
    s.run("UPDATE T SET AMT = 1 WHERE ID = 3;").unwrap();
    assert_eq!(failed(&s, "SELECT ID FROM T;;"), Code::Syntax, "one semicolon");
    assert_eq!(failed(&s, "SELECT ID FROM T; SELECT ID FROM T"), Code::Syntax, "one statement");
    assert!(s.direct("T CROSS JOIN U").is_ok(), "a joined table");
    assert!(s.prepare("T CROSS JOIN U").is_ok(), "a joined table, prepared");
    assert_eq!(rows(&s, "SELECT TRIM(FROM '  a ') FROM T WHERE ID = 1").unwrap(), ["a"]);
    for catalog in ["SELECT ID FROM CAT.APP.T", "SELECT CAT.APP.T.ID FROM T", "SELECT CAT.APP.T.* FROM T", "DROP TABLE CAT.APP.W CASCADE"] {
        assert_eq!(failed(&s, catalog), Code::Unsupported, "{catalog}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.1 SR1, §5.4] NULL and
/// DEFAULT as row values outside an INSERT break a Syntax Rule (SQLCODE
/// -103); a parameter written with a colon, of the module language and
/// embedded SQL, which are not claimed (RULES.md D32), is refused as not
/// implemented (SQLCODE -900) wherever it stands.
#[test]
fn null_default_and_colon_parameters() {
    let db = setup();
    let s = db.session("APP");
    for broken in [
        "SELECT ID FROM T WHERE (ID, NULL) = (1, 2)",
        "SELECT ID FROM T WHERE ID = DEFAULT",
        "SELECT ID FROM T WHERE ID BETWEEN NULL AND 3",
        "SELECT ID FROM T WHERE NULL IN (1, 2)",
    ] {
        assert_eq!(failed(&s, broken), Code::SyntaxRule, "{broken}");
    }
    assert_eq!(failed(&s, "SELECT ID FROM T WHERE ID = :P"), Code::Unsupported, "of a query");
    for colon in ["FETCH C INTO :P", "SELECT ID INTO :X FROM T", "FETCH ABSOLUTE :N FROM C INTO X"] {
        assert_eq!(exec(&s, colon).err(), Some(Code::Unsupported), "{colon}");
    }
}

/// Verifies: [EXTENSION] RULES.md D18 [ISO/IEC 9075:1992 §5.3, §5.4, §10.1,
/// §11.28-§11.32, §17] each refused construct written wrong is a syntax
/// error (SQLCODE -102), not refused as valid: its Format is read in full.
#[test]
fn refused_constructs_written_wrong_are_syntax_errors() {
    let db = setup();
    let s = db.session("APP");
    for wrong in [
        "SELECT _LATIN1 FROM T",
        "SELECT NAME FROM T WHERE NAME = _LATIN1 5",
        "CREATE TABLE W (A CHAR(5) CHARACTER SET \"a-b\")",
        "CREATE CHARACTER SET CS LATIN1",
        "CREATE CHARACTER SET CS GET LATIN1 COLLATION DEFAULT",
        "CREATE COLLATION CL FOR LATIN1 FROM EXTERNAL(X)",
        "CREATE COLLATION CL FOR LATIN1 FROM DESC C2",
        "CREATE COLLATION CL FOR LATIN1 FROM TRANSLATION TR THEN CL",
        "CREATE COLLATION CL FOR LATIN1 FROM DEFAULT NO SPACE",
        "CREATE COLLATION CL FOR LATIN1 FROM DEFAULT PAD",
        "CREATE TRANSLATION TR FOR A TO B FROM EXTERNAL",
        "DROP CHARACTER SET",
        "SET NAMES",
        "SELECT TRANSLATE(NAME USING) FROM T",
        "ALLOCATE DESCRIPTOR 'D1' WITH 10",
        "ALLOCATE DESCRIPTOR",
        "DEALLOCATE X",
        "DEALLOCATE PREPARE",
        "GET DESCRIPTOR 'D1' :N = TYPE",
        "GET DESCRIPTOR 'D1' VALUE 1 :T TYPE",
        "SET DESCRIPTOR 'D1' VALUE 1 COLOUR = 1",
        "SET DESCRIPTOR 'D1' COUNT 2",
        "DESCRIBE S1 USING :X",
        "EXECUTE S1 USING SQL D1",
        "EXECUTE IMMEDIATE",
        "PREPARE S1 FROM",
        "OPEN GLOBAL",
        "FETCH NEXT FROM C1 USING",
        "DELETE FROM T WHERE CURRENT OF GLOBAL",
        "CREATE TABLE W (A INTERVAL DAY TO)",
        "CREATE TABLE W (A INTERVAL SECOND(3,))",
        "SELECT (NAME) DAY FROM T",
        "SELECT ID FROM T WHERE ID = 1 AT LOCAL AT",
        "SET TIME ZONE",
        "SELECT ID FROM CAT.APP.",
    ] {
        assert_eq!(failed(&s, wrong), Code::Syntax, "{wrong}");
    }
    for wrong in ["DELETE WHERE CURRENT C1", "UPDATE SET ID = 1 WHERE C1"] {
        assert_eq!(prepared(&s, wrong), Code::Syntax, "{wrong}");
    }
}
