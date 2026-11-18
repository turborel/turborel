//! The columns of the Information Schema's views that ISO/IEC 9075-11:2006
//! draft Clause 5 gives beyond ISO/IEC 9075:1992's: an interval type's
//! fields and leading precision, whether a table is insertable-into and a
//! column updatable, a temporary table's commit action, a schema's and a
//! routine's SQL-path, the position in its unique constraint of a column a
//! foreign key references, the character repertoire, and what a routine's
//! descriptor holds of external routines, transforms and result casts.

use turborel::db::{Database, Session};

use crate::common::rows;

/// Schema A: table T (K, I of INTERVAL YEAR(3) TO MONTH, S of INTERVAL
/// SECOND(4, 2)) of a primary key (K, I), and R, whose foreign key
/// references T's key in the other order; domain D of INTERVAL DAY; views
/// UV, updatable, and RV, not; global temporary tables GD and GP, ON
/// COMMIT DELETE and PRESERVE ROWS; function F of an interval parameter and
/// result, procedure P; module M of PATH A, LIB; type PERSON. B holds
/// SELECT on T.
fn described() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION LIB").unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL, I INTERVAL YEAR(3) TO MONTH NOT NULL, S INTERVAL SECOND(4, 2),
                           PRIMARY KEY (K, I))
           CREATE TABLE R (RI INTERVAL YEAR(3) TO MONTH, RK INTEGER, FOREIGN KEY (RI, RK) REFERENCES T (I, K))
           CREATE DOMAIN D AS INTERVAL DAY
           CREATE VIEW UV AS SELECT K, S FROM T
           CREATE VIEW RV AS SELECT COUNT(*) AS N FROM T
           CREATE GLOBAL TEMPORARY TABLE GD (K INTEGER) ON COMMIT DELETE ROWS
           CREATE GLOBAL TEMPORARY TABLE GP (K INTEGER) ON COMMIT PRESERVE ROWS
           CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20)) NOT FINAL
           GRANT SELECT ON T TO B",
    )
    .unwrap();
    let a = db.session("A");
    for sql in [
        "CREATE FUNCTION F (P INTERVAL DAY(5)) RETURNS INTERVAL HOUR TO MINUTE CONTAINS SQL RETURN CAST(P AS INTERVAL HOUR TO MINUTE)",
        "CREATE PROCEDURE P () CONTAINS SQL BEGIN END",
        "CREATE MODULE M PATH A, LIB FUNCTION ONE () RETURNS INTEGER CONTAINS SQL RETURN 1; END MODULE",
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

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.21 Description 6, 8, 9, §5.21,
/// §5.28, §5.35, §5.49] an interval type's DATA_TYPE is INTERVAL, its
/// INTERVAL_TYPE its fields, YEAR TO MONTH or SECOND, its
/// INTERVAL_PRECISION its leading field precision and its
/// DATETIME_PRECISION its fractional seconds precision, in COLUMNS,
/// DOMAINS, PARAMETERS and ROUTINES; of another type each is null.
#[test]
fn interval_types_6_21() {
    let db = described();
    let a = db.session("A");
    let columns = "SELECT COLUMN_NAME, DATA_TYPE, INTERVAL_TYPE, INTERVAL_PRECISION, DATETIME_PRECISION \
                   FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' ORDER BY ORDINAL_POSITION";
    assert_eq!(query(&a, columns), ["K,INTEGER,NULL,NULL,NULL", "I,INTERVAL,YEAR TO MONTH,3,0", "S,INTERVAL,SECOND,4,2"]);
    let domains = "SELECT DATA_TYPE, INTERVAL_TYPE, INTERVAL_PRECISION FROM INFORMATION_SCHEMA.DOMAINS WHERE DOMAIN_NAME = 'D'";
    assert_eq!(query(&a, domains), ["INTERVAL,DAY,2"], "DAY of the default leading precision, 2");
    let parameters = "SELECT INTERVAL_TYPE, INTERVAL_PRECISION FROM INFORMATION_SCHEMA.PARAMETERS WHERE SPECIFIC_NAME IN (SELECT SPECIFIC_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_NAME = 'F')";
    assert_eq!(query(&a, parameters), ["DAY,5"]);
    let routines = "SELECT INTERVAL_TYPE, INTERVAL_PRECISION FROM INFORMATION_SCHEMA.ROUTINES WHERE SPECIFIC_NAME IN (SELECT SPECIFIC_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_NAME = 'F')";
    assert_eq!(query(&a, routines), ["HOUR TO MINUTE,2"]);
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.50 Description 4, 7b, §6.20
/// Description 19, §5.61, §5.76, §5.21; ISO/IEC 9075-2:2023 §4.17.6] a base
/// table is insertable-into and each of its columns updatable; an
/// updatable view is too, a view that is not is neither, nor is a view of
/// the Information Schema; VIEWS shows a view's INSERTABLE_INTO as TABLES
/// does; a temporary table's COMMIT_ACTION is DELETE or PRESERVE as its ON
/// COMMIT says, another table's null.
#[test]
fn insertable_updatable_and_commit_action_6_50() {
    let db = described();
    let a = db.session("A");
    let tables = "SELECT TABLE_NAME, IS_INSERTABLE_INTO, COMMIT_ACTION FROM INFORMATION_SCHEMA.TABLES \
                  WHERE TABLE_SCHEMA = 'A' OR TABLE_NAME = 'TABLES' ORDER BY 1";
    let shown = query(&a, tables);
    assert_eq!(shown, ["GD,YES,DELETE", "GP,YES,PRESERVE", "R,YES,NULL", "RV,NO,NULL", "T,YES,NULL", "TABLES,NO,NULL", "UV,YES,NULL",]);
    let views = "SELECT TABLE_NAME, IS_UPDATABLE, INSERTABLE_INTO FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA = 'A' ORDER BY 1";
    assert_eq!(query(&a, views), ["RV,NO,NO", "UV,YES,YES"]);
    let columns = "SELECT TABLE_NAME, COLUMN_NAME, IS_UPDATABLE FROM INFORMATION_SCHEMA.COLUMNS \
                   WHERE TABLE_NAME IN ('T', 'UV', 'RV') ORDER BY 1, 2";
    assert_eq!(query(&a, columns), ["RV,N,NO", "T,I,YES", "T,K,YES", "T,S,YES", "UV,K,YES", "UV,S,YES"]);
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.41 Description 5b, §6.28
/// Description 3, 4, §6.11 Description 2; ISO/IEC 9075-2:2023 §4.3.7 Table
/// 1] SQL_PATH is null of a schema of no `<schema path specification>`;
/// POSITION_IN_UNIQUE_CONSTRAINT is the position in the referenced key of
/// the column each referencing column references, null of a key's own
/// columns; the character set SQL_TEXT is of the repertoire SQL_TEXT.
#[test]
fn schema_path_key_positions_and_repertoire_6_28() {
    let db = described();
    let a = db.session("A");
    assert_eq!(query(&a, "SELECT SCHEMA_NAME, SQL_PATH FROM INFORMATION_SCHEMA.SCHEMATA WHERE SCHEMA_NAME = 'A'"), ["A,NULL"]);
    let keys = "SELECT TABLE_NAME, COLUMN_NAME, ORDINAL_POSITION, POSITION_IN_UNIQUE_CONSTRAINT \
                FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA = 'A' ORDER BY 1, 3";
    assert_eq!(query(&a, keys), ["R,RK,1,1", "R,RI,2,2", "T,K,1,NULL", "T,I,2,NULL"]);
    let sets = "SELECT CHARACTER_SET_NAME, CHARACTER_REPERTOIRE FROM INFORMATION_SCHEMA.CHARACTER_SETS";
    assert_eq!(query(&a, sets), ["SQL_TEXT,SQL_TEXT"]);
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.40 Description 12b, 13a, 15b,
/// 19, 20, 22, 23a, §6.31 Description 8, 9, §6.30 Description 15a, §5.34,
/// §5.35, §5.49; ISO/IEC 9075-2:2023 §4.35.1, §4.35.5] an SQL routine has
/// no external name, language or parameter style; its SQL-path is its
/// schema's, or its module's; it has 0 dynamic result sets; a function's
/// result is not AS LOCATOR, a procedure's AS_LOCATOR is null; it has no
/// to-sql routine nor `<result cast>`; a routine a type's definition makes
/// is dependent on the type, another is not; a parameter has no from-sql
/// or to-sql routine; a method specification has no `<result cast>`.
#[test]
fn what_routine_descriptors_hold_6_40() {
    let db = described();
    let a = db.session("A");
    let routines = "SELECT ROUTINE_NAME, EXTERNAL_NAME, EXTERNAL_LANGUAGE, PARAMETER_STYLE, SQL_PATH, MAX_DYNAMIC_RESULT_SETS, \
                    AS_LOCATOR, IS_UDT_DEPENDENT, TO_SQL_SPECIFIC_NAME, RESULT_CAST_FROM_DATA_TYPE, RESULT_CAST_AS_LOCATOR, \
                    RESULT_CAST_DTD_IDENTIFIER, RESULT_CAST_XML_PRIMARY_MODIFIER \
                    FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_NAME IN ('F', 'P', 'ONE') OR SPECIFIC_NAME = 'PERSON_NAME_OBSERVER' \
                    ORDER BY 1";
    assert_eq!(
        query(&a, routines),
        [
            "F,NULL,NULL,NULL,TURBOREL.A,0,NO,NO,NULL,NULL,NULL,NULL,NULL",
            "NAME,NULL,NULL,NULL,TURBOREL.A,0,NO,YES,NULL,NULL,NULL,NULL,NULL",
            "ONE,NULL,NULL,NULL,TURBOREL.A, TURBOREL.LIB,0,NO,NO,NULL,NULL,NULL,NULL,NULL",
            "P,NULL,NULL,NULL,TURBOREL.A,0,NULL,NO,NULL,NULL,NULL,NULL,NULL",
        ]
    );
    let parameters = "SELECT FROM_SQL_SPECIFIC_CATALOG, FROM_SQL_SPECIFIC_NAME, TO_SQL_SPECIFIC_SCHEMA, TO_SQL_SPECIFIC_NAME \
                      FROM INFORMATION_SCHEMA.PARAMETERS WHERE SPECIFIC_NAME IN (SELECT SPECIFIC_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_NAME = 'F')";
    assert_eq!(query(&a, parameters), ["NULL,NULL,NULL,NULL"]);
    let specs = "SELECT DISTINCT RESULT_CAST_FROM_DATA_TYPE, RESULT_CAST_AS_LOCATOR, RESULT_CAST_DTD_IDENTIFIER \
                 FROM INFORMATION_SCHEMA.METHOD_SPECIFICATIONS WHERE UDT_NAME = 'PERSON'";
    assert_eq!(query(&a, specs), ["NULL,NULL,NULL"]);
}
