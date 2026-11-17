//! Structured types, reference types and typed tables in the Information
//! Schema [ISO/IEC 9075-11:2006 draft §5.11, §5.25, §5.26, §5.33, §5.34,
//! §5.36, §5.49, §5.59, §5.61, §5.70, §5.72]: USER_DEFINED_TYPES, ATTRIBUTES,
//! DIRECT_SUPERTYPES, METHOD_SPECIFICATIONS and their parameters, the
//! routines a structured type's definition makes, TABLES and COLUMNS of a
//! typed table, DIRECT_SUPERTABLES, REFERENCED_TYPES and the privileges on
//! types and on a typed table's methods.

use turborel::db::{Database, Session};

use crate::common::rows;

/// Schema A: PERSON of NAME, NOT FINAL, of a method GREETING and its body;
/// EMPLOYEE under it of PAY and MANAGER, a reference to a PERSON.
fn types() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20) DEFAULT 'NOBODY') NOT FINAL
             REF IS SYSTEM GENERATED
             METHOD GREETING () RETURNS CHARACTER VARYING(30) CONTAINS SQL
           CREATE TYPE EMPLOYEE UNDER PERSON AS (PAY INTEGER, MANAGER REF(PERSON)) NOT FINAL",
    )
    .unwrap();
    let a = db.session("A");
    a.run("CREATE METHOD GREETING () RETURNS CHARACTER VARYING(30) FOR PERSON RETURN SELF.NAME").unwrap();
    a.commit().unwrap();
    db
}

/// The types of `types`, and typed tables PEOPLE of PERSON, its
/// self-referencing column ID, and STAFF of EMPLOYEE under it, its MANAGER
/// scoped to PEOPLE; SELECT on PEOPLE granted to B.
fn described() -> Database {
    let db = types();
    let a = db.session("A");
    for sql in [
        "CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED)",
        "CREATE TABLE STAFF OF EMPLOYEE UNDER PEOPLE (MANAGER WITH OPTIONS SCOPE PEOPLE)",
        "GRANT SELECT ON PEOPLE TO B",
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

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.72, §6.61 Description 2 to 10,
/// §5.26] USER_DEFINED_TYPES shows each structured type, STRUCTURED,
/// instantiable, final or not, of no ordering and no source type, its
/// reference type SYSTEM GENERATED; DIRECT_SUPERTYPES each direct
/// supertype. A user is shown only the types it or PUBLIC holds a
/// privilege on.
#[test]
fn structured_types_5_72() {
    let db = types();
    let sql = "SELECT USER_DEFINED_TYPE_NAME, USER_DEFINED_TYPE_CATEGORY, IS_INSTANTIABLE, IS_FINAL, ORDERING_FORM, \
               REFERENCE_TYPE, SOURCE_DTD_IDENTIFIER, DATA_TYPE FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES ORDER BY 1";
    assert_eq!(
        query(&db.session("A"), sql),
        ["EMPLOYEE,STRUCTURED,YES,NO,NONE,SYSTEM GENERATED,NULL,NULL", "PERSON,STRUCTURED,YES,NO,NONE,SYSTEM GENERATED,NULL,NULL"]
    );
    assert_eq!(query(&db.session("C"), sql), Vec::<String>::new(), "C holds no privilege");
    let supertypes = "SELECT UDT_NAME, SUPERTYPE_SCHEMA, SUPERTYPE_NAME FROM INFORMATION_SCHEMA.DIRECT_SUPERTYPES";
    assert_eq!(query(&db.session("A"), supertypes), ["EMPLOYEE,A,PERSON"]);
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.11, §6.6 Description 1 to 6]
/// ATTRIBUTES shows a subtype's attributes, inherited first, at their
/// ordinal positions, with a default as written and a reference type's
/// referenced type.
#[test]
fn attributes_5_11() {
    let db = types();
    let sql = "SELECT ATTRIBUTE_NAME, ORDINAL_POSITION, ATTRIBUTE_DEFAULT, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH, \
               ATTRIBUTE_UDT_NAME, IS_DERIVED_REFERENCE_ATTRIBUTE FROM INFORMATION_SCHEMA.ATTRIBUTES \
               WHERE UDT_NAME = 'EMPLOYEE' ORDER BY 2";
    assert_eq!(
        query(&db.session("A"), sql),
        ["NAME,1,'NOBODY',CHARACTER VARYING,20,NULL,NO", "PAY,2,NULL,INTEGER,NULL,NULL,NO", "MANAGER,3,NULL,REF,NULL,PERSON,NO"]
    );
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.33, §5.34, §5.49 Description
/// 4, 5; ISO/IEC 9075-2:2023 §11.51 SR10g] METHOD_SPECIFICATIONS shows the
/// observer and mutator of each attribute a type defines and its own
/// method specifications; METHOD_SPECIFICATION_PARAMETERS a mutator's one
/// parameter; ROUTINES each observer, mutator and method body an INSTANCE
/// METHOD of its type, and the constructor function a FUNCTION.
#[test]
fn methods_5_34() {
    let db = types();
    let a = db.session("A");
    let specs = "SELECT METHOD_NAME, IS_STATIC, IS_OVERRIDING, IS_CONSTRUCTOR, DATA_TYPE, METHOD_LANGUAGE, IS_NULL_CALL \
                 FROM INFORMATION_SCHEMA.METHOD_SPECIFICATIONS WHERE UDT_NAME = 'PERSON' ORDER BY 1, 7";
    assert_eq!(
        query(&a, specs),
        ["GREETING,NO,NO,NO,CHARACTER VARYING,SQL,NO", "NAME,NO,NO,NO,USER-DEFINED,SQL,NO", "NAME,NO,NO,NO,CHARACTER VARYING,SQL,YES"]
    );
    let parameters = "SELECT M.METHOD_NAME, P.ORDINAL_POSITION, P.PARAMETER_NAME, P.DATA_TYPE \
                      FROM INFORMATION_SCHEMA.METHOD_SPECIFICATION_PARAMETERS AS P \
                      JOIN INFORMATION_SCHEMA.METHOD_SPECIFICATIONS AS M USING (SPECIFIC_CATALOG, SPECIFIC_SCHEMA, SPECIFIC_NAME) \
                      WHERE M.UDT_NAME = 'PERSON'";
    assert_eq!(query(&a, parameters), ["NAME,1,ATTR,CHARACTER VARYING"]);
    let routines = "SELECT ROUTINE_NAME, ROUTINE_TYPE, UDT_NAME, DATA_TYPE, IS_USER_DEFINED_CAST FROM INFORMATION_SCHEMA.ROUTINES \
                    WHERE ROUTINE_SCHEMA = 'A' AND ROUTINE_NAME IN ('GREETING', 'PERSON', 'NAME') ORDER BY 1, 4";
    assert_eq!(
        query(&a, routines),
        [
            "GREETING,INSTANCE METHOD,PERSON,CHARACTER VARYING,NO",
            "NAME,INSTANCE METHOD,PERSON,CHARACTER VARYING,NO",
            "NAME,INSTANCE METHOD,PERSON,USER-DEFINED,NO",
            "PERSON,FUNCTION,NULL,USER-DEFINED,NO"
        ]
    );
    let parameters = "SELECT ORDINAL_POSITION, IS_RESULT, PARAMETER_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.PARAMETERS \
                      WHERE SPECIFIC_NAME = 'PERSON_NAME_MUTATOR' ORDER BY 1";
    assert_eq!(query(&a, parameters), ["1,YES,SELF,USER-DEFINED", "2,NO,ATTR,CHARACTER VARYING"]);
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.61, §6.50 Description 3, 5, 6,
/// 8, §5.21, §5.25] TABLES shows a typed table's self-referencing column,
/// how its values are made, its type and that it is typed; COLUMNS which
/// column is self-referencing and a reference column's scope;
/// DIRECT_SUPERTABLES a subtable's direct supertable. MC/DC of IS_TYPED: a
/// typed table and one that is not.
#[test]
fn typed_tables_5_61() {
    let db = described();
    let a = db.session("A");
    a.run("CREATE TABLE PLAIN (K INTEGER)").unwrap();
    let tables = "SELECT TABLE_NAME, SELF_REFERENCING_COLUMN_NAME, REFERENCE_GENERATION, USER_DEFINED_TYPE_NAME, IS_TYPED \
                  FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'A' ORDER BY 1";
    assert_eq!(
        query(&a, tables),
        ["PEOPLE,ID,SYSTEM GENERATED,PERSON,YES", "PLAIN,NULL,NULL,NULL,NO", "STAFF,ID,SYSTEM GENERATED,EMPLOYEE,YES"]
    );
    let columns = "SELECT COLUMN_NAME, IS_SELF_REFERENCING, DATA_TYPE, UDT_NAME, SCOPE_SCHEMA, SCOPE_NAME \
                   FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'STAFF' ORDER BY ORDINAL_POSITION";
    assert_eq!(
        query(&a, columns),
        [
            "ID,YES,REF,EMPLOYEE,A,STAFF",
            "NAME,NO,CHARACTER VARYING,NULL,NULL,NULL",
            "PAY,NO,INTEGER,NULL,NULL,NULL",
            "MANAGER,NO,REF,PERSON,A,PEOPLE"
        ]
    );
    let supertables = "SELECT TABLE_NAME, SUPERTABLE_NAME FROM INFORMATION_SCHEMA.DIRECT_SUPERTABLES";
    assert_eq!(query(&a, supertables), ["STAFF,PEOPLE"]);
    let referenced = "SELECT OBJECT_NAME, OBJECT_TYPE, REFERENCE_TYPE_IDENTIFIER, DTD_IDENTIFIER, UDT_NAME \
                      FROM INFORMATION_SCHEMA.REFERENCED_TYPES WHERE OBJECT_TYPE = 'TABLE' ORDER BY 1, 3";
    assert_eq!(query(&a, referenced), ["PEOPLE,TABLE,1,1.1,PERSON", "STAFF,TABLE,1,1.1,EMPLOYEE", "STAFF,TABLE,4,4.1,PERSON"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 GR7, §12.2 GR6; ISO/IEC
/// 9075-11:2006 draft §5.59] TABLE_METHOD_PRIVILEGES shows, of each SELECT
/// on a typed table, one on each method of its type, of the same grantor,
/// grantee and grantability; a grantee sees its own.
#[test]
fn table_method_privileges_5_59() {
    let db = described();
    let sql = "SELECT GRANTOR, GRANTEE, SPECIFIC_NAME, IS_GRANTABLE FROM INFORMATION_SCHEMA.TABLE_METHOD_PRIVILEGES \
               WHERE TABLE_NAME = 'PEOPLE' AND SPECIFIC_NAME LIKE 'PERSON_NAME%' ORDER BY 1, 3";
    assert_eq!(
        query(&db.session("B"), sql),
        ["A,B,PERSON_NAME_MUTATOR,NO", "A,B,PERSON_NAME_OBSERVER,NO"],
        "B's, granted by A, who holds EXECUTE on each"
    );
    let owner = query(&db.session("A"), sql);
    assert!(owner.contains(&"_SYSTEM,A,PERSON_NAME_OBSERVER,YES".to_owned()), "{owner:?}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 GR4, GR5, §12.3; ISO/IEC
/// 9075-11:2006 draft §5.70] UDT_PRIVILEGES shows TYPE USAGE and UNDER on a
/// structured type, the owner's of "_SYSTEM", grantable, and each
/// granted; a grantee is then shown the type and its attributes.
#[test]
fn type_privileges_5_70() {
    let db = types();
    let a = db.session("A");
    a.run("GRANT USAGE ON TYPE PERSON TO C").unwrap();
    a.run("GRANT UNDER ON TYPE PERSON TO C WITH GRANT OPTION").unwrap();
    a.commit().unwrap();
    let sql = "SELECT GRANTOR, GRANTEE, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.UDT_PRIVILEGES \
               WHERE UDT_NAME = 'PERSON' ORDER BY 3, 2";
    assert_eq!(query(&a, sql), ["_SYSTEM,A,TYPE USAGE,YES", "A,C,TYPE USAGE,NO", "_SYSTEM,A,UNDER,YES", "A,C,UNDER,YES"]);
    let c = db.session("C");
    assert_eq!(query(&c, "SELECT USER_DEFINED_TYPE_NAME FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES"), ["PERSON"]);
    assert_eq!(query(&c, "SELECT ATTRIBUTE_NAME FROM INFORMATION_SCHEMA.ATTRIBUTES"), ["NAME"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.10, §4.13, §11.63, §11.65, §11.67;
/// ISO/IEC 9075-11:2006 draft §5.49 Description 16, 17, §5.62, §5.72]
/// USER_DEFINED_TYPES shows a type's ordering; ROUTINES a user-defined cast
/// function's cast, AS ASSIGNMENT or not; TRANSFORMS each function of a
/// transform group. MC/DC of IS_IMPLICITLY_INVOCABLE: a cast AS ASSIGNMENT
/// and one not.
#[test]
fn orderings_casts_and_transforms_5_62() {
    let db = types();
    let a = db.session("A");
    for sql in [
        "CREATE FUNCTION PERSON_NAME (P PERSON) RETURNS CHARACTER VARYING(20) DETERMINISTIC CONTAINS SQL RETURN P.NAME",
        "CREATE FUNCTION WEIGHT (P PERSON) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN 1",
        "CREATE ORDERING FOR PERSON ORDER FULL BY MAP WITH FUNCTION PERSON_NAME (PERSON)",
        "CREATE CAST (PERSON AS CHARACTER VARYING(20)) WITH FUNCTION PERSON_NAME (PERSON) AS ASSIGNMENT",
        "CREATE CAST (PERSON AS INTEGER) WITH FUNCTION WEIGHT (PERSON)",
        "CREATE TRANSFORM FOR PERSON G (FROM SQL WITH FUNCTION PERSON_NAME (PERSON))",
    ] {
        a.run(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    a.commit().unwrap();
    let ordering = "SELECT ORDERING_FORM, ORDERING_CATEGORY, ORDERING_ROUTINE_SCHEMA FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES \
                    WHERE USER_DEFINED_TYPE_NAME = 'PERSON'";
    assert_eq!(query(&a, ordering), ["FULL,MAP,A"]);
    let casts = "SELECT ROUTINE_NAME, IS_USER_DEFINED_CAST, IS_IMPLICITLY_INVOCABLE FROM INFORMATION_SCHEMA.ROUTINES \
                 WHERE ROUTINE_NAME IN ('PERSON_NAME', 'WEIGHT') ORDER BY 1";
    assert_eq!(query(&a, casts), ["PERSON_NAME,YES,YES", "WEIGHT,YES,NO"]);
    let transforms = "SELECT R.ROUTINE_NAME, T.GROUP_NAME, T.TRANSFORM_TYPE FROM INFORMATION_SCHEMA.TRANSFORMS AS T \
                      JOIN INFORMATION_SCHEMA.ROUTINES AS R USING (SPECIFIC_CATALOG, SPECIFIC_SCHEMA, SPECIFIC_NAME) \
                      WHERE T.UDT_NAME = 'PERSON' ORDER BY 3";
    assert_eq!(query(&a, transforms), ["PERSON_NAME,G,FROM SQL"]);
}
