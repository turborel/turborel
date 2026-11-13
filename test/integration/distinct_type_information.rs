//! Distinct types in the Information Schema [ISO/IEC 9075-2:2023 §4.9.2,
//! §11.51 GR1, GR2; RULES.md D69]: USER_DEFINED_TYPES, UDT_PRIVILEGES,
//! ROUTINES, PARAMETERS, ROUTINE_PRIVILEGES and TRANSFORMS, and the name of
//! a column's, element type's or parameter's type.

use turborel::db::{Database, Session};

use crate::common::rows;

/// Schema A: MONEY of NUMERIC(10, 2) and TAGS of an array, table T of a
/// column of each and of an array of MONEY; USAGE on MONEY and EXECUTE on
/// MONEY_TO_DISTINCT, grantable, granted to B.
fn described() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TYPE MONEY AS NUMERIC(10, 2)
           CREATE TYPE TAGS AS CHARACTER VARYING(8) ARRAY[3]
           CREATE TABLE T (K INTEGER, P MONEY, L MONEY ARRAY[2])
           GRANT USAGE ON TYPE MONEY TO B
           GRANT EXECUTE ON SPECIFIC FUNCTION MONEY_TO_DISTINCT TO B WITH GRANT OPTION",
    )
    .unwrap();
    db
}

/// The rows of `sql` in `s`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.51 GR1, GR2c] USER_DEFINED_TYPES
/// shows each distinct type instantiable and final, with its source type's
/// descriptor. MC/DC of the ordering: a source of no collection type is
/// ordered FULL BY MAP with the specific name of its FNSDT, and an array
/// source leaves the type of ordering NONE. A user is shown only the types
/// it or PUBLIC holds a privilege on.
#[test]
fn user_defined_types_11_51() {
    let db = described();
    let sql = "SELECT USER_DEFINED_TYPE_NAME, USER_DEFINED_TYPE_CATEGORY, IS_INSTANTIABLE, IS_FINAL, ORDERING_FORM, \
               ORDERING_CATEGORY, ORDERING_ROUTINE_NAME, DATA_TYPE, CHARACTER_SET_NAME, NUMERIC_PRECISION, NUMERIC_SCALE, \
               MAXIMUM_CARDINALITY, SOURCE_DTD_IDENTIFIER FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES ORDER BY 1";
    let money = "MONEY,DISTINCT,YES,YES,FULL,MAP,MONEY_TO_SOURCE,NUMERIC,NULL,10,2,NULL,1";
    assert_eq!(query(&db.session("A"), sql), [money, "TAGS,DISTINCT,YES,YES,NONE,NULL,NULL,ARRAY,NULL,NULL,NULL,3,1"]);
    assert_eq!(query(&db.session("B"), sql), [money], "B holds USAGE on MONEY alone");
    assert_eq!(query(&db.session("C"), sql), Vec::<String>::new());
    let elements = "SELECT OBJECT_NAME, OBJECT_TYPE, COLLECTION_TYPE_IDENTIFIER, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH, \
                    CHARACTER_SET_NAME, DTD_IDENTIFIER FROM INFORMATION_SCHEMA.ELEMENT_TYPES \
                    WHERE OBJECT_TYPE = 'USER-DEFINED TYPE'";
    assert_eq!(query(&db.session("A"), elements), ["TAGS,USER-DEFINED TYPE,1,CHARACTER VARYING,8,SQL_TEXT,1.1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.9.2] a column, and an element type, of
/// a distinct type is described by DATA_TYPE USER-DEFINED and the type's
/// name in UDT_CATALOG, UDT_SCHEMA and UDT_NAME, which are null of any
/// other type.
#[test]
fn types_named_4_9_2() {
    let db = described();
    let a = db.session("A");
    let columns = "SELECT COLUMN_NAME, DATA_TYPE, UDT_CATALOG, UDT_SCHEMA, UDT_NAME, NUMERIC_PRECISION \
                   FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' ORDER BY 1";
    let expected = ["K,INTEGER,NULL,NULL,NULL,10", "L,ARRAY,NULL,NULL,NULL,NULL", "P,USER-DEFINED,TURBOREL,A,MONEY,NULL"];
    assert_eq!(query(&a, columns), expected);
    let elements = "SELECT OBJECT_NAME, DATA_TYPE, UDT_CATALOG, UDT_SCHEMA, UDT_NAME, DTD_IDENTIFIER \
                    FROM INFORMATION_SCHEMA.ELEMENT_TYPES WHERE OBJECT_TYPE = 'TABLE'";
    assert_eq!(query(&a, elements), ["T,USER-DEFINED,TURBOREL,A,MONEY,3.1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.51 GR2b] ROUTINES shows each
/// type's FNUDT and FNSDT, SQL functions of their specific names, each
/// DETERMINISTIC, CONTAINS SQL, called on null input and of definer's
/// rights, with its result type; PARAMETERS each one's one input
/// parameter. A user is shown the functions it or PUBLIC holds EXECUTE
/// on, and their parameters.
#[test]
fn routines_and_parameters_11_51() {
    let db = described();
    let routines = "SELECT SPECIFIC_NAME, ROUTINE_NAME, ROUTINE_TYPE, DATA_TYPE, NUMERIC_PRECISION, TYPE_UDT_NAME, \
                    MAXIMUM_CARDINALITY, ROUTINE_BODY, IS_DETERMINISTIC, SQL_DATA_ACCESS, IS_NULL_CALL, \
                    SCHEMA_LEVEL_ROUTINE, SECURITY_TYPE FROM INFORMATION_SCHEMA.ROUTINES ORDER BY 1";
    let traits = "SQL,YES,CONTAINS SQL,NO,YES,DEFINER";
    let to_distinct = format!("MONEY_TO_DISTINCT,MONEY,FUNCTION,USER-DEFINED,NULL,MONEY,NULL,{traits}");
    assert_eq!(
        query(&db.session("A"), routines),
        [
            to_distinct.clone(),
            format!("MONEY_TO_SOURCE,NUMERIC,FUNCTION,NUMERIC,10,NULL,NULL,{traits}"),
            format!("TAGS_TO_DISTINCT,TAGS,FUNCTION,USER-DEFINED,NULL,TAGS,NULL,{traits}"),
            format!("TAGS_TO_SOURCE,ARRAY,FUNCTION,ARRAY,NULL,NULL,3,{traits}"),
        ]
    );
    assert_eq!(query(&db.session("B"), routines), [to_distinct], "B holds EXECUTE on MONEY_TO_DISTINCT alone");
    let parameters = "SELECT SPECIFIC_NAME, ORDINAL_POSITION, PARAMETER_MODE, IS_RESULT, AS_LOCATOR, PARAMETER_NAME, \
                      DATA_TYPE, NUMERIC_SCALE, UDT_NAME FROM INFORMATION_SCHEMA.PARAMETERS WHERE SPECIFIC_NAME LIKE 'MONEY%' \
                      ORDER BY 1";
    let parameter = "MONEY_TO_DISTINCT,1,IN,NO,NO,SDTP,NUMERIC,2,NULL";
    assert_eq!(query(&db.session("A"), parameters), [parameter, "MONEY_TO_SOURCE,1,IN,NO,NO,UDTP,USER-DEFINED,NULL,MONEY"]);
    assert_eq!(query(&db.session("B"), parameters), [parameter]);
    let elements = "SELECT OBJECT_NAME, COLLECTION_TYPE_IDENTIFIER, DATA_TYPE FROM INFORMATION_SCHEMA.ELEMENT_TYPES \
                    WHERE OBJECT_TYPE = 'ROUTINE' ORDER BY 1";
    assert_eq!(query(&db.session("A"), elements), ["TAGS_TO_DISTINCT,1,CHARACTER VARYING", "TAGS_TO_SOURCE,0,CHARACTER VARYING"]);
    assert_eq!(query(&db.session("C"), routines), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.3 SR4a, SR4d; §11.51 GR2] UDT_PRIVILEGES
/// shows TYPE USAGE and ROUTINE_PRIVILEGES EXECUTE, each its owner's,
/// grantable, and each granted, to its grantee and its grantor alone.
#[test]
fn privileges_12_3() {
    let db = described();
    let types = "SELECT GRANTOR, GRANTEE, UDT_NAME, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.UDT_PRIVILEGES \
                 ORDER BY UDT_NAME, GRANTEE";
    let usage = "A,B,MONEY,TYPE USAGE,NO";
    assert_eq!(query(&db.session("A"), types), ["_SYSTEM,A,MONEY,TYPE USAGE,YES", usage, "_SYSTEM,A,TAGS,TYPE USAGE,YES"]);
    assert_eq!(query(&db.session("B"), types), [usage]);
    assert_eq!(query(&db.session("C"), types), Vec::<String>::new());
    let routines = "SELECT GRANTOR, GRANTEE, SPECIFIC_NAME, ROUTINE_NAME, PRIVILEGE_TYPE, IS_GRANTABLE \
                    FROM INFORMATION_SCHEMA.ROUTINE_PRIVILEGES WHERE SPECIFIC_NAME LIKE 'MONEY%' ORDER BY 3, 2";
    let execute = "A,B,MONEY_TO_DISTINCT,MONEY,EXECUTE,YES";
    let owned = ["_SYSTEM,A,MONEY_TO_DISTINCT,MONEY,EXECUTE,YES", execute, "_SYSTEM,A,MONEY_TO_SOURCE,NUMERIC,EXECUTE,YES"];
    assert_eq!(query(&db.session("A"), routines), owned);
    assert_eq!(query(&db.session("B"), routines), [execute]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.51 GR2b; §4.9.6] TRANSFORMS shows
/// each type's one transform group, named FNUDT, its FROM SQL function
/// FNSDT and its TO SQL function FNUDT, to a user shown the type.
#[test]
fn transforms_11_51() {
    let db = described();
    let sql = "SELECT UDT_NAME, SPECIFIC_NAME, GROUP_NAME, TRANSFORM_TYPE FROM INFORMATION_SCHEMA.TRANSFORMS ORDER BY 1, 4";
    let money = ["MONEY,MONEY_TO_SOURCE,MONEY,FROM SQL", "MONEY,MONEY_TO_DISTINCT,MONEY,TO SQL"];
    assert_eq!(
        query(&db.session("A"), sql),
        [money.as_slice(), &["TAGS,TAGS_TO_SOURCE,TAGS,FROM SQL", "TAGS,TAGS_TO_DISTINCT,TAGS,TO SQL"]].concat()
    );
    assert_eq!(query(&db.session("B"), sql), money);
    assert_eq!(query(&db.session("C"), sql), Vec::<String>::new());
}
