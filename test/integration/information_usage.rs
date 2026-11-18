//! The views of usage of the Information Schema [ISO/IEC 9075-11:2006
//! draft §5.13, §5.16, §5.17, §5.20, §5.24, §5.44, §5.46 to §5.48, §5.74]:
//! what SQL routines, views, check constraints and generated columns
//! depend on, the columns of a user-defined type, each collation's
//! character set, and the data type descriptors a user may see, each
//! shown to the owner of the schema of what is depended on.

use turborel::db::{Database, Session};

use crate::common::rows;

/// Schema A: table T (K, N), of generated column G on K and N; distinct
/// type MONEY, column C.M of it; deterministic function D; function H of
/// D; function LATEST reading T.K; procedure DRAW drawing from sequence Q;
/// view V of D; domain POS of a check of D; table CK of a check of D;
/// assertion SMALL of D; collation FOLD. B holds SELECT on T and EXECUTE
/// on D.
pub fn depended() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER, N INTEGER, G INTEGER GENERATED ALWAYS AS (K + N))
           CREATE TYPE MONEY AS DECIMAL(9, 2) FINAL
           CREATE TABLE C (M MONEY)
           CREATE SEQUENCE Q
           CREATE COLLATION FOLD FOR SQL_TEXT FROM CASE_INSENSITIVE NO PAD
           GRANT SELECT ON T TO B",
    )
    .unwrap();
    let a = db.session("A");
    for sql in [
        "CREATE FUNCTION D (X INTEGER) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN X + 1",
        "CREATE FUNCTION H (X INTEGER) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN D(X)",
        "CREATE FUNCTION LATEST () RETURNS INTEGER READS SQL DATA RETURN (SELECT MAX(K) FROM T)",
        "CREATE PROCEDURE DRAW () MODIFIES SQL DATA INSERT INTO T (K) VALUES (NEXT VALUE FOR Q)",
        "CREATE VIEW V AS SELECT D(K) AS DK FROM T",
        "CREATE DOMAIN POS AS INTEGER CONSTRAINT POS_D CHECK (D(VALUE) > 0)",
        "CREATE TABLE CK (N INTEGER CONSTRAINT CK_D CHECK (D(N) > 0))",
        "CREATE ASSERTION SMALL CHECK (NOT EXISTS (SELECT * FROM T WHERE D(K) > 100))",
        "GRANT EXECUTE ON FUNCTION D TO B",
    ] {
        a.run(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    a.commit().unwrap();
    db
}

/// The rows of `sql` in `s`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// The specific name of A's routine `name`.
fn specific(s: &Session, name: &str) -> String {
    let sql = format!("SELECT SPECIFIC_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_NAME = '{name}'");
    query(s, &sql).pop().unwrap_or_default()
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.44, §5.46 to §5.48, §6.35,
/// §6.37 to §6.39] ROUTINE_TABLE_USAGE and ROUTINE_COLUMN_USAGE show the
/// tables and columns an SQL routine's body names, ROUTINE_ROUTINE_USAGE
/// the routines it invokes, each by its specific name, and
/// ROUTINE_SEQUENCE_USAGE the sequence generators it draws from; each to
/// the owner of the schema of what is used, not to another user.
#[test]
fn routine_usage_5_44() {
    let db = depended();
    let a = db.session("A");
    let (read, draw, h, d) = (specific(&a, "LATEST"), specific(&a, "DRAW"), specific(&a, "H"), specific(&a, "D"));
    let tables = "SELECT SPECIFIC_NAME, ROUTINE_NAME, TABLE_NAME FROM INFORMATION_SCHEMA.ROUTINE_TABLE_USAGE ORDER BY 2";
    assert_eq!(query(&a, tables), [format!("{draw},DRAW,T"), format!("{read},LATEST,T")]);
    let columns = "SELECT ROUTINE_NAME, TABLE_NAME, COLUMN_NAME FROM INFORMATION_SCHEMA.ROUTINE_COLUMN_USAGE ORDER BY 1, 3";
    assert_eq!(query(&a, columns), ["LATEST,T,K"], "the columns its queries reference, not an INSERT's column list");
    let routines = "SELECT SPECIFIC_NAME, ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINE_ROUTINE_USAGE";
    assert_eq!(query(&a, routines), [format!("{h},{d}")], "ROUTINE_NAME the specific name of the routine invoked");
    let sequences = "SELECT SPECIFIC_NAME, SEQUENCE_NAME FROM INFORMATION_SCHEMA.ROUTINE_SEQUENCE_USAGE";
    assert_eq!(query(&a, sequences), [format!("{draw},Q")]);
    let b = db.session("B");
    for view in ["ROUTINE_TABLE_USAGE", "ROUTINE_COLUMN_USAGE", "ROUTINE_ROUTINE_USAGE", "ROUTINE_SEQUENCE_USAGE"] {
        assert_eq!(query(&b, &format!("SELECT COUNT(*) FROM INFORMATION_SCHEMA.{view}")), ["0"], "{view}: B owns nothing used");
    }
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.74, §5.13, §6.63, §6.13]
/// VIEW_ROUTINE_USAGE shows the routines a view's query invokes, and
/// CHECK_CONSTRAINT_ROUTINE_USAGE those an assertion's, a domain
/// constraint's and a table check constraint's condition invokes; each to
/// the routine's schema's owner.
#[test]
fn view_and_constraint_routine_usage_5_74() {
    let db = depended();
    let a = db.session("A");
    let d = specific(&a, "D");
    let views = "SELECT TABLE_NAME, SPECIFIC_NAME FROM INFORMATION_SCHEMA.VIEW_ROUTINE_USAGE";
    assert_eq!(query(&a, views), [format!("V,{d}")]);
    let checks = "SELECT CONSTRAINT_NAME, SPECIFIC_NAME FROM INFORMATION_SCHEMA.CHECK_CONSTRAINT_ROUTINE_USAGE ORDER BY 1";
    assert_eq!(query(&a, checks), [format!("CK_D,{d}"), format!("POS_D,{d}"), format!("SMALL,{d}")]);
    let b = db.session("B");
    assert_eq!(query(&b, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.CHECK_CONSTRAINT_ROUTINE_USAGE"), ["0"]);
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.17, §5.20, §5.16, §6.18,
/// §6.17] COLUMN_COLUMN_USAGE shows each column a generated column depends
/// on; COLUMN_UDT_USAGE each column of a user-defined type, with the type;
/// COLLATION_CHARACTER_SET_APPLICABILITY each collation of a schema the
/// user owns, applicable to SQL_TEXT, so not the built-in ones.
#[test]
fn column_usage_and_applicability_5_17() {
    let db = depended();
    let a = db.session("A");
    let generated = "SELECT TABLE_NAME, COLUMN_NAME, DEPENDENT_COLUMN FROM INFORMATION_SCHEMA.COLUMN_COLUMN_USAGE ORDER BY 2";
    assert_eq!(query(&a, generated), ["T,K,G", "T,N,G"]);
    let udt = "SELECT UDT_SCHEMA, UDT_NAME, TABLE_NAME, COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMN_UDT_USAGE";
    assert_eq!(query(&a, udt), ["A,MONEY,C,M"]);
    let applicability = "SELECT COLLATION_SCHEMA, COLLATION_NAME, CHARACTER_SET_SCHEMA, CHARACTER_SET_NAME \
                         FROM INFORMATION_SCHEMA.COLLATION_CHARACTER_SET_APPLICABILITY";
    assert_eq!(query(&a, applicability), ["A,FOLD,INFORMATION_SCHEMA,SQL_TEXT"]);
    let b = db.session("B");
    for view in ["COLUMN_COLUMN_USAGE", "COLUMN_UDT_USAGE", "COLLATION_CHARACTER_SET_APPLICABILITY"] {
        assert_eq!(query(&b, &format!("SELECT COUNT(*) FROM INFORMATION_SCHEMA.{view}")), ["0"], "{view}");
    }
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.24] DATA_TYPE_PRIVILEGES shows
/// the object and DTD_IDENTIFIER of each data type descriptor the other
/// views show the user: a column's, a domain's, a routine's parameter's
/// and result's, a distinct type's source's; not a column of a domain,
/// whose type is its domain's. OBJECT_TYPE is of the type the UNION's
/// literals make, a fixed-length string, so trimmed here.
#[test]
fn data_type_privileges_5_24() {
    let db = depended();
    let a = db.session("A");
    let d = specific(&a, "D");
    let shown = |object: &str| {
        format!(
            "SELECT TRIM(OBJECT_TYPE), DTD_IDENTIFIER FROM INFORMATION_SCHEMA.DATA_TYPE_PRIVILEGES WHERE OBJECT_NAME = '{object}' ORDER BY 2"
        )
    };
    assert_eq!(query(&a, &shown("T")), ["TABLE,1", "TABLE,2", "TABLE,3"]);
    assert_eq!(query(&a, &shown("POS")), ["DOMAIN,1"]);
    assert_eq!(query(&a, &shown(&d)), ["ROUTINE,0", "ROUTINE,1"]);
    assert_eq!(query(&a, &shown("MONEY")), ["USER-DEFINED TYPE,1"]);
    let b = db.session("B");
    assert_eq!(query(&b, &shown("T")), ["TABLE,1", "TABLE,2", "TABLE,3"], "B's SELECT shows it T's");
    assert_eq!(query(&b, &shown("CK")), Vec::<String>::new());
}
