//! Distinct types [ISO/IEC 9075-2:2023 §4.9, §11.51, §11.59; RULES.md
//! D69]: their definition and the rules it is checked by, DROP TYPE with
//! RESTRICT and CASCADE, DROP SCHEMA, and the definitions kept after the
//! database is closed and opened or crashes. Their values are tested in
//! `distinct_type_values.rs`, their functions in `distinct_type_routines.rs`,
//! their privileges in `distinct_type_privileges.rs`, dynamic SQL in
//! `distinct_type_dynamic.rs` and the Information Schema in
//! `distinct_type_information.rs`.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::{Code, SqlError};

use crate::common::{Scratch, after_crash, money, rows};

/// The SQLCODE class and the text, with its rule, of the refusal of
/// `result`.
fn refusal<T>(result: Result<T, SqlError>) -> (Code, String) {
    let e = result.err().expect("refused");
    (e.code, e.to_string())
}

/// The rows of `sql` in `s`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// The source type's DATA_TYPE, precision and scale of type `name`.
fn source_of(s: &Session, name: &str) -> Vec<String> {
    let sql = format!(
        "SELECT DATA_TYPE, NUMERIC_PRECISION, NUMERIC_SCALE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES \
         WHERE USER_DEFINED_TYPE_NAME = '{name}'"
    );
    query(s, &sql)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.51 SR5a, SR7a, GR1, GR2] CREATE TYPE
/// makes a distinct type of a predefined type or an array type, in a schema
/// definition and as a statement of its own. An exact source type is
/// NUMERIC of its precision and scale, and an approximate one FLOAT of its
/// precision (IV223). FINAL may be written, and the CAST options in either
/// order (S028), naming FNUDT and FNSDT.
#[test]
fn a_distinct_type_is_defined_11_51() {
    let db = money();
    let a = db.session("A");
    a.run("CREATE TYPE COUNTED AS SMALLINT FINAL").unwrap();
    a.run("CREATE TYPE RATE AS REAL").unwrap();
    a.run("CREATE TYPE CODE AS CHARACTER(3) CAST (DISTINCT AS SOURCE) WITH TEXT CAST (SOURCE AS DISTINCT) WITH TO_CODE").unwrap();
    a.run("CREATE TYPE A.TAGS AS INTEGER ARRAY[3]").unwrap();
    assert_eq!(source_of(&a, "MONEY"), ["NUMERIC,10,2,NULL"]);
    assert_eq!(source_of(&a, "COUNTED"), ["NUMERIC,5,0,NULL"], "SMALLINT respelled");
    assert_eq!(source_of(&a, "RATE"), ["FLOAT,24,NULL,NULL"], "REAL respelled");
    assert_eq!(source_of(&a, "CODE"), ["CHARACTER,NULL,NULL,3"]);
    assert_eq!(source_of(&a, "TAGS"), ["ARRAY,NULL,NULL,NULL"]);
    let names = "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE SPECIFIC_NAME LIKE 'CODE%' ORDER BY 1";
    assert_eq!(query(&a, names), ["TEXT", "TO_CODE"]);
    let names = "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE SPECIFIC_NAME LIKE 'COUNTED%' ORDER BY 1";
    assert_eq!(query(&a, names), ["COUNTED", "NUMERIC"], "the type's name, and the source type's [§9.9]");
    a.commit().unwrap();
    assert_eq!(query(&a, "SELECT TEXT(TO_CODE('ab')) FROM A.T"), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.51 SR1, SR2, SR3, SR4, SR5a,
/// SR7b-SR7f; §11.34 SR3] CREATE TYPE is refused, each by its rule: of a
/// schema not there (3F000), or another than its schema definition's; of
/// a name a type or a domain of the schema has, and CREATE DOMAIN of a
/// type's name; of an option written twice; of a row type, another
/// distinct type or a structured type as its representation; and of each
/// option of a structured type.
#[test]
fn a_definition_is_refused_by_its_rules_11_51() {
    let db = money();
    let a = db.session("A");
    let refused = |sql: &str| {
        let (code, msg) = refusal(a.run(sql));
        let _ = a.rollback();
        (code, msg)
    };
    assert_eq!(refused("CREATE TYPE B.X AS INTEGER").0, Code::SyntaxRule, "SR1");
    let other = db.schema("CREATE SCHEMA AUTHORIZATION C CREATE TYPE A.X AS INTEGER");
    assert!(other.unwrap_err().to_string().contains("§11.51 SR2"));
    for (sql, cited) in [
        ("CREATE TYPE MONEY AS INTEGER", "SR3"),
        ("CREATE DOMAIN MONEY AS INTEGER", "§11.34 SR3"),
        ("CREATE TYPE X AS INTEGER FINAL FINAL", "SR4"),
        ("CREATE TYPE X AS INTEGER CAST (SOURCE AS DISTINCT) WITH Y CAST (SOURCE AS DISTINCT) WITH Z", "SR4"),
        ("CREATE TYPE X AS ROW(A INTEGER)", "SR5a"),
        ("CREATE TYPE X AS MONEY", "SR5a"),
        ("CREATE TYPE X AS INTEGER INSTANTIABLE", "SR7b"),
        ("CREATE TYPE X AS INTEGER NOT FINAL", "SR7c"),
        ("CREATE TYPE X UNDER MONEY AS INTEGER", "SR7d"),
        ("CREATE TYPE X AS INTEGER REF USING INTEGER", "SR7e"),
        ("CREATE TYPE X AS INTEGER CAST (SOURCE AS REF) WITH F", "SR7f"),
    ] {
        let (code, msg) = refused(sql);
        assert_eq!(code, Code::SyntaxRule, "{sql}");
        assert!(msg.contains(cited), "{sql}: {msg}");
    }
    assert_eq!(refused("CREATE TYPE X AS (A INTEGER)").0, Code::Unsupported, "a structured type [SR5b]");
    assert_eq!(refused("CREATE DOMAIN D AS MONEY").0, Code::SyntaxRule, "a domain's type is a predefined type");
    assert_eq!(refused("CREATE DOMAIN D AS MONEY ARRAY[2]").0, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR20b; §11.51 GR2b] a function a
/// type's definition makes may not take a parameter of the type designator
/// of another function's of its name in the schema. MC/DC: of the name of
/// MONEY's FNUDT and a source of its designator, refused; of the name and
/// another designator, made; of the designator and another name, made.
#[test]
fn a_function_name_clash_is_refused_11_60() {
    let db = money();
    let a = db.session("A");
    let (code, msg) = refusal(a.run("CREATE TYPE X AS INTEGER CAST (SOURCE AS DISTINCT) WITH MONEY"));
    assert_eq!(code, Code::SyntaxRule);
    assert!(msg.contains("§11.60 SR20b"), "{msg}");
    a.rollback().unwrap();
    a.run("CREATE TYPE X AS CHARACTER(3) CAST (SOURCE AS DISTINCT) WITH MONEY").unwrap();
    a.run("CREATE TYPE Y AS INTEGER").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&a, "SELECT \"NUMERIC\"(Y(2)) FROM T"), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.59 SR3, SR4b, SR4f, SR4h, GR7-GR9]
/// DROP TYPE with RESTRICT is refused while a column is of a type based
/// on it, another type is based on it, or a view, constraint, assertion or
/// generated column references it or calls its functions, and of a type not
/// there. With CASCADE each of those goes with it, and the rows of its
/// table stay.
#[test]
fn drop_type_restrict_and_cascade_11_59() {
    let db = money();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 5)").unwrap();
    a.run("CREATE TABLE W (K INTEGER, N NUMERIC(10, 2) CHECK (CAST(N AS MONEY) > MONEY(0)), X NUMERIC(10, 2) GENERATED ALWAYS AS (\"NUMERIC\"(MONEY(N))))").unwrap();
    a.run("CREATE VIEW V AS SELECT K, MONEY(K) AS M FROM T").unwrap();
    a.commit().unwrap();
    let cited = |sql: &str| {
        let (_, msg) = refusal(a.run(sql));
        a.rollback().unwrap();
        msg
    };
    assert!(cited("DROP TYPE NOPE RESTRICT").contains("§11.59 SR3"));
    let msg = cited("DROP TYPE MONEY RESTRICT");
    assert!(msg.contains("column A.T.P") && msg.contains("§11.59 SR4b]"), "{msg}");
    a.run("ALTER TABLE T DROP COLUMN P CASCADE").unwrap();
    assert!(cited("DROP TYPE MONEY RESTRICT").contains("SR4f, SR4h"), "the view, the CHECK and the generated column");
    a.run("ALTER TABLE T DROP COLUMN P CASCADE").unwrap();
    a.run("DROP VIEW V CASCADE").unwrap();
    a.run("ALTER TABLE W DROP COLUMN X CASCADE").unwrap();
    assert!(cited("DROP TYPE MONEY RESTRICT").contains("constraint A.W_CHECK1 of A.W references type A.MONEY"));
    a.run("ALTER TABLE T DROP COLUMN P CASCADE").unwrap();
    a.run("DROP TYPE MONEY CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&a, "SELECT * FROM T"), ["1"]);
    assert_eq!(query(&a, "SELECT * FROM W"), Vec::<String>::new());
    let gone = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'V'";
    assert_eq!(query(&a, gone), ["0"]);
    let constraints = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE TABLE_NAME = 'W'";
    assert_eq!(query(&a, constraints), ["0"]);
    let columns = "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'W' ORDER BY 1";
    assert_eq!(query(&a, columns), ["K", "N"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.59 SR4f v] DROP TYPE with RESTRICT
/// is refused while another type's source is an array of it, and with
/// CASCADE drops that type too.
#[test]
fn a_type_based_on_another_11_59() {
    let db = money();
    let a = db.session("A");
    a.run("ALTER TABLE T DROP COLUMN P CASCADE").unwrap();
    a.run("CREATE TYPE PRICES AS MONEY ARRAY[4]").unwrap();
    a.commit().unwrap();
    let (_, msg) = refusal(a.run("DROP TYPE MONEY RESTRICT"));
    assert!(msg.contains("type A.PRICES is based on A.MONEY") && msg.contains("§11.59 SR4f]"), "{msg}");
    a.rollback().unwrap();
    a.run("DROP TYPE MONEY CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES"), ["0"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.2 GR; §11.59 GR] DROP SCHEMA CASCADE
/// drops its types, and the columns of other schemas' tables of them.
#[test]
fn drop_schema_drops_its_types_11_2() {
    let db = money();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
    a.run("GRANT USAGE ON TYPE MONEY TO B").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    b.run("CREATE TABLE U (K INTEGER, M A.MONEY)").unwrap();
    b.run("INSERT INTO U VALUES (1, 2)").unwrap();
    b.commit().unwrap();
    a.run("DROP SCHEMA A CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&b, "SELECT * FROM U"), ["1"]);
    assert_eq!(query(&b, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES"), ["0"]);
}

/// The database in `dir`, opened from its files by `fs`.
fn opened(fs: &Arc<dyn FileSystem>, dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D69, D25 a distinct type, its columns, an
/// index on one, USAGE and EXECUTE granted, and a type dropped are kept
/// after the database is closed and opened, crashes, or crashes after a
/// checkpoint.
#[test]
fn kept_after_reopening() {
    for (case, checkpoint, closed) in [("closed", false, true), ("crashed", false, false), ("checkpointed", true, false)] {
        let scratch = Scratch::new(&format!("distinct-{case}"));
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let before = opened(&fs, &dir);
        before
            .schema("CREATE SCHEMA AUTHORIZATION A CREATE TYPE MONEY AS NUMERIC(10, 2) CREATE TABLE T (K INTEGER, P MONEY UNIQUE)")
            .unwrap();
        let a = before.session("A");
        a.run("INSERT INTO T VALUES (1, 5), (2, 7.5)").unwrap();
        a.run("CREATE TYPE GONE AS INTEGER").unwrap();
        a.commit().unwrap();
        if checkpoint {
            before.checkpoint().unwrap();
        }
        a.run("GRANT USAGE ON TYPE MONEY TO B").unwrap();
        a.run("GRANT EXECUTE ON FUNCTION \"NUMERIC\"(MONEY) TO B").unwrap();
        a.run("DROP TYPE GONE RESTRICT").unwrap();
        a.commit().unwrap();
        if closed {
            before.close().unwrap();
        }
        let db = opened(&if closed { Arc::clone(&fs) } else { after_crash() }, &dir);
        let a = db.session("A");
        assert_eq!(query(&a, "SELECT K FROM T WHERE P = MONEY(7.5)"), ["2"], "{case}");
        assert_eq!(a.run("INSERT INTO T VALUES (3, 5)").unwrap_err().code, Code::Constraint, "{case}: the key kept");
        let held = "SELECT UDT_NAME, PRIVILEGE_TYPE FROM INFORMATION_SCHEMA.UDT_PRIVILEGES WHERE GRANTEE = 'B'";
        assert_eq!(query(&a, held), ["MONEY,TYPE USAGE"], "{case}");
        let executes = "SELECT SPECIFIC_NAME FROM INFORMATION_SCHEMA.ROUTINE_PRIVILEGES WHERE GRANTEE = 'B'";
        assert_eq!(query(&a, executes), ["MONEY_TO_SOURCE"], "{case}");
        assert_eq!(query(&a, "SELECT USER_DEFINED_TYPE_NAME FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES"), ["MONEY"], "{case}");
        a.rollback().unwrap();
        db.close().unwrap();
        drop((db, before));
    }
}
