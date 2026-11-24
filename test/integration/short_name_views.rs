//! The short name views of the Information Schema [ISO/IEC 9075-11:2006
//! draft §5.77; ISO/IEC 9075-4:2006 draft §18.8; ISO/IEC 9075-14:2006
//! draft §20.15]: each shows a user, under the draft's short names, the
//! rows its view shows that user, column for column.

use turborel::db::{Database, Session};

use crate::common::{rows, sorted};
use crate::roles::{committed, owned};
use crate::xml_validate::ORDERS;

/// The columns that describe a data type, as the short name views name
/// them, each `SHORT=LONG` where its view names it otherwise.
const TYPE: &str = "DATA_TYPE CHAR_MAX_LENGTH=CHARACTER_MAXIMUM_LENGTH \
                    CHAR_OCTET_LENGTH=CHARACTER_OCTET_LENGTH CHAR_SET_CATALOG=CHARACTER_SET_CATALOG \
                    CHAR_SET_SCHEMA=CHARACTER_SET_SCHEMA CHARACTER_SET_NAME COLLATION_CATALOG \
                    COLLATION_SCHEMA COLLATION_NAME NUMERIC_PRECISION NUMERIC_PREC_RADIX=NUMERIC_PRECISION_RADIX \
                    NUMERIC_SCALE DATETIME_PRECISION INTERVAL_TYPE INTERVAL_PRECISION";

/// A reference type's scope, and a collection's cardinality.
const SCOPE: &str = "SCOPE_CATALOG SCOPE_SCHEMA SCOPE_NAME MAX_CARDINALITY=MAXIMUM_CARDINALITY DTD_IDENTIFIER";

/// The columns of an XML type [ISO/IEC 9075-14:2006 draft §20.15].
const XML: &str = "XML_PRIMARY_MOD=XML_PRIMARY_MODIFIER XML_SECONDARY_MOD=XML_SECONDARY_MODIFIER \
                   XML_SCHEMA_CATALOG XML_SCHEMA_SCHEMA XML_SCHEMA_NAME XML_SCHEMA_NSPACE=XML_SCHEMA_NAMESPACE \
                   XML_SCHEMA_ELEMENT";

/// The columns of an XML type's associated string type.
const XML_STRING: &str = "XML_CHAR_TYPE XML_CHAR_MAX_LEN XML_CHAR_OCT_LEN XML_CHAR_SET_CAT XML_CHAR_SET_SCH \
                          XML_CHAR_SET_NAME XML_CHAR_COLL_CAT XML_CHAR_COLL_SCH XML_CHAR_COLL_NAME XML_CHAR_DTD_ID \
                          XML_OPTION";

/// A database of users OWNER, B and C and their schemas, A's table T (K)
/// among them (`roles::owned`), and in A: domain D, of a constraint;
/// table U, of an identity column, a reference to T, a column of D, a
/// generated column, an XML column of the registered XML Schema O, a row
/// and an array; view V; sequence generator Q; types PERSON, of a method,
/// and EMPLOYEE under it, of a reference to PERSON, and typed table
/// PEOPLE; function F, of an XML parameter and result; trigger TR on T,
/// whose condition invokes function H, drawing from Q; module SHOP, reading T. Role R is granted B WITH ADMIN
/// OPTION, and SELECT on PEOPLE and EXECUTE on F to R; B holds SELECT on
/// U and USAGE on D, Q and O.
fn cataloged() -> Database {
    let db = owned();
    committed(&db, "OWNER", &["CREATE ROLE R", "GRANT R TO B WITH ADMIN OPTION"]);
    let a = db.session("A");
    a.run(&format!("CREATE XML SCHEMA O LOCATION 'o.xsd' AS '{ORDERS}'")).unwrap();
    a.commit().unwrap();
    committed(
        &db,
        "A",
        &[
            "CREATE DOMAIN D AS INTEGER CONSTRAINT POSITIVE CHECK (VALUE > 0)",
            "CREATE TABLE U (ID INTEGER GENERATED ALWAYS AS IDENTITY, K INTEGER REFERENCES T, N D, \
             G INTEGER GENERATED ALWAYS AS (K * 2), X XML(DOCUMENT(XMLSCHEMA ID A.O)), \
             W ROW(F INTEGER), Y INTEGER ARRAY[3], CHECK (K < 100))",
            "CREATE VIEW V AS SELECT K FROM T",
            "CREATE SEQUENCE Q",
            "CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20), NOTE XML) NOT FINAL REF IS SYSTEM GENERATED \
             METHOD GREETING (P XML) RETURNS CHARACTER VARYING(30) CONTAINS SQL",
            "CREATE TYPE EMPLOYEE UNDER PERSON AS (MANAGER REF(PERSON)) NOT FINAL",
            "CREATE METHOD GREETING (P XML) RETURNS CHARACTER VARYING(30) FOR PERSON RETURN SELF.NAME",
            "CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED)",
            "CREATE FUNCTION F (P XML(DOCUMENT(UNTYPED)), Q INTEGER) RETURNS XML(CONTENT(ANY)) CONTAINS SQL RETURN P",
            "CREATE FUNCTION H (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X",
            "CREATE TRIGGER TR AFTER UPDATE OF K ON T REFERENCING OLD ROW AS O FOR EACH ROW \
             WHEN (O.K > H(0)) INSERT INTO U (K) VALUES (NEXT VALUE FOR Q)",
            "CREATE MODULE SHOP FUNCTION COUNTED () RETURNS INTEGER READS SQL DATA RETURN (SELECT COUNT(K) FROM T); END MODULE",
            "GRANT SELECT ON PEOPLE TO R",
            "GRANT EXECUTE ON FUNCTION F TO R",
            "GRANT SELECT ON U TO B",
            "GRANT USAGE ON DOMAIN D TO B",
            "GRANT USAGE ON SEQUENCE Q TO B",
            "GRANT USAGE ON XML SCHEMA O TO B",
        ],
    );
    db
}

/// The short and long names of `columns`, each `SHORT=LONG`, or a name
/// both views give the column.
fn names(columns: &[&str]) -> (Vec<String>, Vec<String>) {
    let each = columns.iter().flat_map(|c| c.split_whitespace());
    each.map(|c| c.split_once('=').map_or((c.to_owned(), c.to_owned()), |(s, l)| (s.to_owned(), l.to_owned()))).unzip()
}

/// The most characters a name has without Feature F391 [ISO/IEC
/// 9075-2:2023 §5.2 CR1, CR2].
const SHORT: usize = 18;

/// The number of rows the short name view `short` shows in `s`, which
/// must have `columns` of the view `long`, in order, each named in
/// `SHORT` characters as it is, and show the rows that view shows, column
/// for column.
fn same(s: &Session, (short, long): (&str, &str), columns: &[&str]) -> usize {
    let (shorts, longs) = names(columns);
    let long_names: Vec<&String> = shorts.iter().filter(|n| n.len() > SHORT).collect();
    assert!(short.len() <= SHORT && long_names.is_empty(), "{short}: {long_names:?}");
    let named = format!(
        "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS \
         WHERE TABLE_SCHEMA = 'INFORMATION_SCHEMA' AND TABLE_NAME = '{short}' ORDER BY ORDINAL_POSITION"
    );
    assert_eq!(rows(s, &named).unwrap(), shorts, "the columns of {short}");
    let shown = sorted(s, &format!("SELECT * FROM INFORMATION_SCHEMA.{short}"));
    assert_eq!(shown, sorted(s, &format!("SELECT {} FROM INFORMATION_SCHEMA.{long}", longs.join(", "))), "{short}");
    s.commit().unwrap();
    shown.len()
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.77; ISO/IEC 9075-14:2006
/// draft §20.15] the short name views of the views of ISO/IEC 9075:1992,
/// and SEQUENCES_S, show what their views show the owner and a grantee:
/// COLUMNS_S and DOMAINS_S with the XML columns, each column of no column
/// of its view here left out.
#[test]
fn short_name_views_of_sql_92_5_77() {
    let db = cataloged();
    let columns = [
        "TABLE_CATALOG TABLE_SCHEMA TABLE_NAME COLUMN_NAME ORDINAL_POSITION COLUMN_DEFAULT IS_NULLABLE",
        "DATA_TYPE CHAR_MAX_LENGTH=CHARACTER_MAXIMUM_LENGTH CHAR_OCTET_LENGTH=CHARACTER_OCTET_LENGTH",
        "NUMERIC_PRECISION NUMERIC_PREC_RADIX=NUMERIC_PRECISION_RADIX NUMERIC_SCALE DATETIME_PRECISION INTERVAL_TYPE",
        "INTERVAL_PRECISION",
        "CHAR_SET_CATALOG=CHARACTER_SET_CATALOG CHAR_SET_SCHEMA=CHARACTER_SET_SCHEMA CHARACTER_SET_NAME",
        "COLLATION_CATALOG COLLATION_SCHEMA COLLATION_NAME DOMAIN_CATALOG DOMAIN_SCHEMA DOMAIN_NAME",
        "UDT_CATALOG UDT_SCHEMA UDT_NAME",
        SCOPE,
        "IS_SELF_REF=IS_SELF_REFERENCING IS_IDENTITY ID_GENERATION=IDENTITY_GENERATION ID_START=IDENTITY_START",
        "ID_INCREMENT=IDENTITY_INCREMENT ID_MAXIMUM=IDENTITY_MAXIMUM ID_MINIMUM=IDENTITY_MINIMUM",
        "ID_CYCLE=IDENTITY_CYCLE IS_GENERATED GENERATION_EXPR=GENERATION_EXPRESSION IS_UPDATABLE",
        XML,
    ];
    let views: [(&str, &str, &[&str]); 14] = [
        ("CATALOG_NAME", "INFORMATION_SCHEMA_CATALOG_NAME", &["CATALOG_NAME"]),
        (
            "SCHEMATA_S",
            "SCHEMATA",
            &["CATALOG_NAME SCHEMA_NAME SCHEMA_OWNER DEF_CHAR_SET_CAT=DEFAULT_CHARACTER_SET_CATALOG \
               DEF_CHAR_SET_SCH=DEFAULT_CHARACTER_SET_SCHEMA DEF_CHAR_SET_NAME=DEFAULT_CHARACTER_SET_NAME SQL_PATH"],
        ),
        ("DOMAINS_S", "DOMAINS", &["DOMAIN_CATALOG DOMAIN_SCHEMA DOMAIN_NAME", TYPE, "DOMAIN_DEFAULT", SCOPE_TAIL, XML]),
        (
            "TABLES_S",
            "TABLES",
            &["TABLE_CATALOG TABLE_SCHEMA TABLE_NAME TABLE_TYPE SELF_REF_COL_NAME=SELF_REFERENCING_COLUMN_NAME \
               REF_GENERATION=REFERENCE_GENERATION UDT_CATALOG=USER_DEFINED_TYPE_CATALOG \
               UDT_SCHEMA=USER_DEFINED_TYPE_SCHEMA UDT_NAME=USER_DEFINED_TYPE_NAME IS_INSERTABLE_INTO IS_TYPED COMMIT_ACTION"],
        ),
        ("COLUMNS_S", "COLUMNS", &columns),
        (
            "REF_CONSTRAINTS",
            "REFERENTIAL_CONSTRAINTS",
            &["CONSTRAINT_CATALOG CONSTRAINT_SCHEMA CONSTRAINT_NAME UNIQUE_CONSTR_CAT=UNIQUE_CONSTRAINT_CATALOG \
               UNIQUE_CONSTR_SCH=UNIQUE_CONSTRAINT_SCHEMA UNIQUE_CONSTR_NAME=UNIQUE_CONSTRAINT_NAME \
               MATCH_OPTION UPDATE_RULE DELETE_RULE"],
        ),
        (
            "KEY_COLUMN_USAGE_S",
            "KEY_COLUMN_USAGE",
            &[
                CONSTRAINT,
                "TABLE_CATALOG TABLE_SCHEMA TABLE_NAME COLUMN_NAME ORDINAL_POSITION POSITION_IN_UC=POSITION_IN_UNIQUE_CONSTRAINT",
            ],
        ),
        (
            "CHARACTER_SETS_S",
            "CHARACTER_SETS",
            &["CHAR_SET_CATALOG=CHARACTER_SET_CATALOG CHAR_SET_SCHEMA=CHARACTER_SET_SCHEMA CHARACTER_SET_NAME \
               CHAR_REPERTOIRE=CHARACTER_REPERTOIRE FORM_OF_USE DEF_COLLATE_CAT=DEFAULT_COLLATE_CATALOG DEF_COLLATE_SCHEMA=DEFAULT_COLLATE_SCHEMA \
               DEF_COLLATE_NAME=DEFAULT_COLLATE_NAME"],
        ),
        ("COLLATIONS_S", "COLLATIONS", &["COLLATION_CATALOG COLLATION_SCHEMA COLLATION_NAME PAD_ATTRIBUTE"]),
        (
            "SEQUENCES_S",
            "SEQUENCES",
            &["SEQUENCE_CATALOG SEQUENCE_SCHEMA SEQUENCE_NAME DATA_TYPE NUMERIC_PRECISION \
               NUMERIC_PREC_RADIX=NUMERIC_PRECISION_RADIX NUMERIC_SCALE MAXIMUM_VALUE MINIMUM_VALUE INCREMENT CYCLE_OPTION"],
        ),
        ("CONSTR_TABLE_USAGE", "CONSTRAINT_TABLE_USAGE", &["TABLE_CATALOG TABLE_SCHEMA TABLE_NAME", CONSTRAINT]),
        ("CONSTR_COL_USAGE", "CONSTRAINT_COLUMN_USAGE", &["TABLE_CATALOG TABLE_SCHEMA TABLE_NAME COLUMN_NAME", CONSTRAINT]),
        (
            "COL_DOMAIN_USAGE",
            "COLUMN_DOMAIN_USAGE",
            &["DOMAIN_CATALOG DOMAIN_SCHEMA DOMAIN_NAME TABLE_CATALOG TABLE_SCHEMA TABLE_NAME COLUMN_NAME"],
        ),
        (
            "TRANSLATIONS_S",
            "TRANSLATIONS",
            &["TRANS_CATALOG=TRANSLATION_CATALOG TRANSLATION_SCHEMA TRANSLATION_NAME \
               SRC_CHAR_SET_CAT=SOURCE_CHARACTER_SET_CATALOG SRC_CHAR_SET_SCH=SOURCE_CHARACTER_SET_SCHEMA \
               SRC_CHAR_SET_NAME=SOURCE_CHARACTER_SET_NAME TGT_CHAR_SET_CAT=TARGET_CHARACTER_SET_CATALOG \
               TGT_CHAR_SET_SCH=TARGET_CHARACTER_SET_SCHEMA TGT_CHAR_SET_NAME=TARGET_CHARACTER_SET_NAME \
               TRANS_SRC_CATALOG=TRANSLATION_SOURCE_CATALOG TRANS_SRC_SCHEMA=TRANSLATION_SOURCE_SCHEMA \
               TRANS_SRC_NAME=TRANSLATION_SOURCE_NAME"],
        ),
    ];
    for user in ["A", "B"] {
        let s = db.session(user);
        for (short, long, columns) in views {
            let shown = same(&s, (short, long), columns);
            assert!(shown > 0 || short == "TRANSLATIONS_S" || user == "B", "{user} sees no row of {short}");
        }
    }
    let b = db.session("B");
    assert_eq!(rows(&b, "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES_S WHERE TABLE_SCHEMA = 'A'").unwrap(), ["U"]);
}

/// A constraint's catalog, schema and name.
const CONSTRAINT: &str = "CONSTRAINT_CATALOG CONSTRAINT_SCHEMA CONSTRAINT_NAME";

/// `SCOPE`'s last two, of a view that shows no scope.
const SCOPE_TAIL: &str = "MAX_CARDINALITY=MAXIMUM_CARDINALITY DTD_IDENTIFIER";

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.77; ISO/IEC 9075-14:2006
/// draft §20.15] the short name views of the views of arrays and row
/// types, user-defined types, routines, structured types and reference
/// types show what their views show the owner and a grantee, with the XML
/// columns; ROUTINES_S has XML_CHAR_TYPE once, at its end.
#[test]
fn short_name_views_of_types_and_routines_5_77() {
    let db = cataloged();
    let object = "OBJECT_CATALOG OBJECT_SCHEMA OBJECT_NAME OBJECT_TYPE";
    let udt = "UDT_CATALOG UDT_SCHEMA UDT_NAME";
    let specific = "SPECIFIC_CATALOG SPECIFIC_SCHEMA SPECIFIC_NAME";
    let parameter = "ORDINAL_POSITION PARAMETER_MODE IS_RESULT AS_LOCATOR PARAMETER_NAME";
    let routines = [
        specific,
        "ROUTINE_CATALOG ROUTINE_SCHEMA ROUTINE_NAME ROUTINE_TYPE MODULE_CATALOG MODULE_SCHEMA MODULE_NAME",
        udt,
        TYPE,
        "TYPE_UDT_CATALOG TYPE_UDT_SCHEMA TYPE_UDT_NAME",
        SCOPE,
        "XML_PRIMARY_MOD=XML_PRIMARY_MODIFIER XML_SECOND_MOD=XML_SECONDARY_MODIFIER XML_SCHEMA_CAT=XML_SCHEMA_CATALOG \
         XML_SCH_SCH=XML_SCHEMA_SCHEMA XML_SCH_NAME=XML_SCHEMA_NAME XML_SCH_NSPACE=XML_SCHEMA_NAMESPACE \
         XML_SCH_ELEMENT=XML_SCHEMA_ELEMENT",
        "ROUTINE_BODY ROUTINE_DEFINITION EXTERNAL_NAME EXTERNAL_LANGUAGE PARAMETER_STYLE IS_DETERMINISTIC",
        "SQL_DATA_ACCESS IS_NULL_CALL SQL_PATH SCH_LEVEL_ROUTINE=SCHEMA_LEVEL_ROUTINE MAX_DYN_RESLT_SETS=MAX_DYNAMIC_RESULT_SETS",
        "IS_USER_DEFND_CAST=IS_USER_DEFINED_CAST IS_IMP_INVOCABLE=IS_IMPLICITLY_INVOCABLE SECURITY_TYPE",
        TO_SQL,
        "AS_LOCATOR CREATED LAST_ALTERED NEW_SAVEPOINT_LVL=NEW_SAVEPOINT_LEVEL IS_UDT_DEPENDENT",
        "RC_FROM_DATA_TYPE=RESULT_CAST_FROM_DATA_TYPE RC_AS_LOCATOR=RESULT_CAST_AS_LOCATOR",
        "RC_CHAR_MAX_LENGTH=RESULT_CAST_CHAR_MAX_LENGTH RC_CHAR_OCT_LENGTH=RESULT_CAST_CHAR_OCTET_LENGTH",
        "RC_CHAR_SET_CAT=RESULT_CAST_CHAR_SET_CATALOG RC_CHAR_SET_SCHEMA=RESULT_CAST_CHAR_SET_SCHEMA",
        "RC_CHAR_SET_NAME=RESULT_CAST_CHARACTER_SET_NAME",
        RC_COLLATION,
        "RC_NUM_PREC=RESULT_CAST_NUMERIC_PRECISION",
        RC_TAIL,
        RC_XML,
        XML_STRING,
        "XML_RETURN_MECH=XML_RETURN_MECHANISM",
    ];
    let specs = [
        specific,
        udt,
        "METHOD_NAME IS_STATIC IS_OVERRIDING IS_CONSTRUCTOR",
        TYPE,
        "RETURN_UDT_CATALOG RETURN_UDT_SCHEMA RETURN_UDT_NAME",
        SCOPE,
        "METHOD_LANGUAGE PARAMETER_STYLE IS_DETERMINISTIC SQL_DATA_ACCESS IS_NULL_CALL TO_SQL_SPEC_CAT=TO_SQL_SPECIFIC_CATALOG",
        "TO_SQL_SPEC_SCHEMA=TO_SQL_SPECIFIC_SCHEMA TO_SQL_SPEC_NAME=TO_SQL_SPECIFIC_NAME AS_LOCATOR",
        "CREATED LAST_ALTERED",
        "RC_FROM_DATA_TYPE=RESULT_CAST_FROM_DATA_TYPE RC_AS_LOCATOR=RESULT_CAST_AS_LOCATOR",
        "RC_CHAR_MAX_LENGTH=RESULT_CAST_CHAR_MAX_LENGTH RC_CHAR_OCT_LENGTH=RESULT_CAST_CHAR_OCTET_LENGTH",
        "RC_CHAR_SET_CAT=RESULT_CAST_CHAR_SET_CATALOG RC_CHAR_SET_SCHEMA=RESULT_CAST_CHAR_SET_SCHEMA",
        "RC_CHAR_SET_NAME=RESULT_CAST_CHAR_SET_NAME",
        RC_COLLATION,
        "RC_NUMERIC_PREC=RESULT_CAST_NUMERIC_PRECISION",
        RC_TAIL,
        XML,
        RC_XML,
    ];
    let spec_params = [
        specific,
        parameter,
        FROM_SQL,
        TYPE,
        "PARM_UDT_CATALOG=PARAMETER_UDT_CATALOG PARM_UDT_SCHEMA=PARAMETER_UDT_SCHEMA PARM_UDT_NAME=PARAMETER_UDT_NAME",
        SCOPE,
        XML,
    ];
    let attributes = [
        udt,
        "ATTRIBUTE_NAME ORDINAL_POSITION ATTRIBUTE_DEFAULT",
        TYPE,
        "ATT_UDT_CAT=ATTRIBUTE_UDT_CATALOG ATT_UDT_SCHEMA=ATTRIBUTE_UDT_SCHEMA ATT_UDT_NAME=ATTRIBUTE_UDT_NAME",
        SCOPE,
        "IS_DERIVED_REF_ATT=IS_DERIVED_REFERENCE_ATTRIBUTE",
        XML,
    ];
    let views: [(&str, &str, &[&str]); 10] = [
        ("ELEMENT_TYPES_S", "ELEMENT_TYPES", &[object, "COLLECTION_TYPE_ID=COLLECTION_TYPE_IDENTIFIER", TYPE, udt, SCOPE, XML]),
        ("FIELDS_S", "FIELDS", &[object, "ROW_IDENTIFIER FIELD_NAME ORDINAL_POSITION", TYPE, udt, SCOPE, XML]),
        (
            "UDT_S",
            "USER_DEFINED_TYPES",
            &[
                "UDT_CATALOG=USER_DEFINED_TYPE_CATALOG UDT_SCHEMA=USER_DEFINED_TYPE_SCHEMA UDT_NAME=USER_DEFINED_TYPE_NAME",
                "UDT_CATEGORY=USER_DEFINED_TYPE_CATEGORY IS_INSTANTIABLE IS_FINAL ORDERING_FORM ORDERING_CATEGORY",
                "ORDERING_ROUT_CAT=ORDERING_ROUTINE_CATALOG ORDERING_ROUT_SCH=ORDERING_ROUTINE_SCHEMA",
                "ORDERING_ROUT_NAME=ORDERING_ROUTINE_NAME REFERENCE_TYPE",
                TYPE,
                "SOURCE_DTD_ID=SOURCE_DTD_IDENTIFIER REF_DTD_IDENTIFIER",
            ],
        ),
        ("ROUTINES_S", "ROUTINES", &routines),
        (
            "PARAMETERS_S",
            "PARAMETERS",
            &[
                specific,
                parameter,
                FROM_SQL,
                TO_SQL,
                TYPE,
                udt,
                SCOPE,
                XML,
                XML_STRING,
                "XML_PASSING_MECH=XML_PASSING_MECHANISM",
                "PARAMETER_DEFAULT",
            ],
        ),
        ("ATTRIBUTES_S", "ATTRIBUTES", &attributes),
        ("METHOD_SPECS", "METHOD_SPECIFICATIONS", &specs),
        ("METHOD_SPEC_PARAMS", "METHOD_SPECIFICATION_PARAMETERS", &spec_params),
        ("REFERENCED_TYPES_S", "REFERENCED_TYPES", &[object, "REFERENCE_TYPE_ID=REFERENCE_TYPE_IDENTIFIER", TYPE, udt, SCOPE]),
        ("TABLE_METHOD_PRIVS", "TABLE_METHOD_PRIVILEGES", &[GRANT, "TABLE_CATALOG TABLE_SCHEMA TABLE_NAME", specific, "IS_GRANTABLE"]),
    ];
    for user in ["A", "B"] {
        let s = db.session(user);
        for (short, long, columns) in views {
            assert!(same(&s, (short, long), columns) > 0 || user == "B", "{user} sees no row of {short}");
        }
    }
}

/// A privilege's grantor and grantee.
const GRANT: &str = "GRANTOR GRANTEE";

/// A routine's or parameter's from-sql routine.
const FROM_SQL: &str = "FROM_SQL_SPEC_CAT=FROM_SQL_SPECIFIC_CATALOG FROM_SQL_SPEC_SCH=FROM_SQL_SPECIFIC_SCHEMA \
                        FROM_SQL_SPEC_NAME=FROM_SQL_SPECIFIC_NAME";

/// A routine's or parameter's to-sql routine.
const TO_SQL: &str = "TO_SQL_SPEC_CAT=TO_SQL_SPECIFIC_CATALOG TO_SQL_SPEC_SCHEMA=TO_SQL_SPECIFIC_SCHEMA \
                      TO_SQL_SPEC_NAME=TO_SQL_SPECIFIC_NAME";

/// The collation of a `<result cast>`'s type.
const RC_COLLATION: &str = "RC_COLLATION_CAT=RESULT_CAST_COLLATION_CATALOG RC_COLLATION_SCH=RESULT_CAST_COLLATION_SCHEMA \
                            RC_COLLATION_NAME=RESULT_CAST_COLLATION_NAME";

/// The rest of a `<result cast>`'s type, from its radix on.
const RC_TAIL: &str = "RC_NUMERIC_RADIX=RESULT_CAST_NUMERIC_RADIX RC_NUMERIC_SCALE=RESULT_CAST_NUMERIC_SCALE \
                       RC_DATETIME_PREC=RESULT_CAST_DATETIME_PRECISION RC_INTERVAL_TYPE=RESULT_CAST_INTERVAL_TYPE \
                       RC_INTERVAL_PREC=RESULT_CAST_INTERVAL_PRECISION RC_TYPE_UDT_CAT=RESULT_CAST_TYPE_UDT_CATALOG \
                       RC_TYPE_UDT_SCHEMA=RESULT_CAST_TYPE_UDT_SCHEMA RC_TYPE_UDT_NAME=RESULT_CAST_TYPE_UDT_NAME \
                       RC_SCOPE_CATALOG=RESULT_CAST_SCOPE_CATALOG RC_SCOPE_SCHEMA=RESULT_CAST_SCOPE_SCHEMA \
                       RC_SCOPE_NAME=RESULT_CAST_SCOPE_NAME RC_MAX_CARDINALITY=RESULT_CAST_MAX_CARDINALITY \
                       RC_DTD_IDENTIFIER=RESULT_CAST_DTD_IDENTIFIER";

/// The XML columns of a `<result cast>`'s type [ISO/IEC 9075-14:2006 draft
/// §20.15].
const RC_XML: &str = "RC_XML_PRIMARY_MOD=RESULT_CAST_XML_PRIMARY_MODIFIER RC_XML_SECOND_MOD=RESULT_CAST_XML_SECONDARY_MODIFIER \
                      RC_XML_SCHEMA_CAT=RESULT_CAST_XML_SCHEMA_CATALOG RC_XML_SCH_SCH=RESULT_CAST_XML_SCHEMA_SCHEMA \
                      RC_XML_SCH_NAME=RESULT_CAST_XML_SCHEMA_NAME RC_XML_SCH_NSPACE=RESULT_CAST_XML_SCHEMA_NAMESPACE \
                      RC_XML_SCH_ELEMENT=RESULT_CAST_XML_SCHEMA_ELEMENT";

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.77] the short name views of
/// the views of roles and triggers show what their views show, to a user
/// once it sets a role as before.
#[test]
fn short_name_views_of_roles_and_triggers_5_77() {
    let db = cataloged();
    let trigger = "TRIGGER_CATALOG TRIGGER_SCHEMA TRIGGER_NAME";
    let event = "EVENT_OBJECT_CAT=EVENT_OBJECT_CATALOG EVENT_OBJECT_SCH=EVENT_OBJECT_SCHEMA EVENT_OBJECT_TABLE";
    let views: [(&str, &str, &[&str]); 9] = [
        ("ADMIN_ROLE_AUTHS", "ADMINISTRABLE_ROLE_AUTHORIZATIONS", &["GRANTEE ROLE_NAME IS_GRANTABLE"]),
        (
            "ROLE_ROUT_GRANTS",
            "ROLE_ROUTINE_GRANTS",
            &[
                GRANT,
                "SPECIFIC_CATALOG SPECIFIC_SCHEMA SPECIFIC_NAME ROUTINE_CATALOG ROUTINE_SCHEMA ROUTINE_NAME PRIVILEGE_TYPE IS_GRANTABLE",
            ],
        ),
        (
            "ROL_TAB_METH_GRNTS",
            "ROLE_TABLE_METHOD_GRANTS",
            &[GRANT, "TABLE_CATALOG TABLE_SCHEMA TABLE_NAME SPECIFIC_CATALOG SPECIFIC_SCHEMA SPECIFIC_NAME IS_GRANTABLE"],
        ),
        (
            "TRIGGERS_S",
            "TRIGGERS",
            &[
                trigger,
                "EVENT_MANIPULATION",
                event,
                "ACTION_ORDER ACTION_CONDITION ACTION_STATEMENT ACTION_ORIENTATION ACTION_TIMING",
                "ACT_REF_OLD_TABLE=ACTION_REFERENCE_OLD_TABLE ACT_REF_NEW_TABLE=ACTION_REFERENCE_NEW_TABLE",
                "ACT_REF_OLD_ROW=ACTION_REFERENCE_OLD_ROW ACT_REF_NEW_ROW=ACTION_REFERENCE_NEW_ROW CREATED",
            ],
        ),
        ("TRIG_UPDATE_COLS", "TRIGGERED_UPDATE_COLUMNS", &[trigger, event, "EVENT_OBJECT_COL=EVENT_OBJECT_COLUMN"]),
        ("TRIG_TABLE_USAGE", "TRIGGER_TABLE_USAGE", &[trigger, "TABLE_CATALOG TABLE_SCHEMA TABLE_NAME"]),
        ("TRIG_COLUMN_USAGE", "TRIGGER_COLUMN_USAGE", &[trigger, "TABLE_CATALOG TABLE_SCHEMA TABLE_NAME COLUMN_NAME"]),
        ("TRIG_ROUT_USAGE_S", "TRIGGER_ROUTINE_USAGE", &[trigger, "SPECIFIC_CATALOG SPECIFIC_SCHEMA SPECIFIC_NAME"]),
        ("TRIG_SEQ_USAGE_S", "TRIGGER_SEQUENCE_USAGE", &[trigger, "SEQUENCE_CATALOG SEQUENCE_SCHEMA SEQUENCE_NAME"]),
    ];
    let (a, b) = (db.session("A"), db.session("B"));
    b.run("SET ROLE 'R'").unwrap();
    for (short, long, columns) in views {
        let roles = short.starts_with("ADMIN") || short.starts_with("ROL");
        let s = if roles { &b } else { &a };
        assert!(same(s, (short, long), columns) > 0, "no row of {short}");
    }
}

/// Verifies: [ISO/IEC 9075-4:2006 draft §18.8] MODULE_COL_USAGE and
/// MODULES_S show what MODULE_COLUMN_USAGE and MODULES show; MODULES_S
/// shows DEFAULT_SCHEMA_NAME as DEFAULT_SCHEMA and the timestamps as
/// CREATED and ALTERED.
#[test]
fn short_name_views_of_modules_18_8() {
    let db = cataloged();
    let module = "MODULE_CATALOG MODULE_SCHEMA MODULE_NAME";
    let modules = [
        module,
        "DEF_CHAR_SET_CAT=DEFAULT_CHARACTER_SET_CATALOG DEF_CHAR_SET_SCH=DEFAULT_CHARACTER_SET_SCHEMA",
        "DEF_CHAR_SET_NAME=DEFAULT_CHARACTER_SET_NAME DEF_SCHEMA_CATALOG=DEFAULT_SCHEMA_CATALOG",
        "DEFAULT_SCHEMA=DEFAULT_SCHEMA_NAME MODULE_DEFINITION MODULE_AUTH=MODULE_AUTHORIZATION SQL_PATH",
        "CREATED=MODULE_CREATED ALTERED=MODULE_LAST_ALTERED",
    ];
    let a = db.session("A");
    assert!(same(&a, ("MODULE_COL_USAGE", "MODULE_COLUMN_USAGE"), &[module, "TABLE_CATALOG TABLE_SCHEMA TABLE_NAME COLUMN_NAME"]) > 0);
    assert!(same(&a, ("MODULES_S", "MODULES"), &modules) > 0);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §20.15] XML_SCHEMAS_S,
/// XML_SCH_NSPACES_S and XML_SCH_ELEMENTS_S show what XML_SCHEMAS,
/// XML_SCHEMA_NAMESPACES and XML_SCHEMA_ELEMENTS show the owner and a
/// grantee of USAGE, the permanently registered XML Schemas among them;
/// and COLUMNS_S shows a column's registered XML Schema.
#[test]
fn short_name_views_of_registered_xml_schemas_20_15() {
    let db = cataloged();
    let schema = "XML_SCH_TGT_NSPACE=XML_SCHEMA_TARGET_NAMESPACE XML_SCH_LOCATION=XML_SCHEMA_LOCATION \
                  XML_SCHEMA_CATALOG XML_SCHEMA_SCHEMA XML_SCHEMA_NAME";
    let views: [(&str, &str, &[&str]); 3] = [
        (
            "XML_SCH_ELEMENTS_S",
            "XML_SCHEMA_ELEMENTS",
            &[schema, "XML_SCH_NSPACE=XML_SCHEMA_NAMESPACE XML_SCHEMA_ELEMENT XML_SCH_EL_IS_DET=XML_SCHEMA_ELEMENT_IS_DETERMINISTIC"],
        ),
        (
            "XML_SCH_NSPACES_S",
            "XML_SCHEMA_NAMESPACES",
            &[schema, "XML_SCH_NSPACE=XML_SCHEMA_NAMESPACE XML_SCH_NSP_IS_DET=XML_SCHEMA_NAMESPACE_IS_DETERMINISTIC"],
        ),
        (
            "XML_SCHEMAS_S",
            "XML_SCHEMAS",
            &[schema, "XML_SCHEMA_IS_DET=XML_SCHEMA_IS_DETERMINISTIC XML_SCHEMA_IS_PERM=XML_SCHEMA_IS_PERMANENT"],
        ),
    ];
    for user in ["A", "B", "C"] {
        let s = db.session(user);
        for (short, long, columns) in views {
            assert!(same(&s, (short, long), columns) > 0, "{user} sees no row of {short}");
        }
    }
    let registered = "SELECT XML_SCHEMA_SCHEMA, XML_SCHEMA_NAME, XML_SCHEMA_IS_PERM FROM INFORMATION_SCHEMA.XML_SCHEMAS_S \
                      WHERE XML_SCHEMA_SCHEMA = 'A'";
    assert_eq!(rows(&db.session("B"), registered).unwrap(), ["A,O,NO"]);
    assert_eq!(rows(&db.session("C"), registered).unwrap(), Vec::<String>::new(), "C holds no USAGE on O");
    let typed = "SELECT COLUMN_NAME, XML_PRIMARY_MOD, XML_SCHEMA_NAME, XML_SCHEMA_NSPACE FROM INFORMATION_SCHEMA.COLUMNS_S \
                 WHERE TABLE_NAME = 'U' AND XML_SCHEMA_NAME IS NOT NULL";
    assert_eq!(rows(&db.session("A"), typed).unwrap(), ["X,DOCUMENT,O,NULL"]);
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.77] the short name views of the
/// views of usage and of the collations' applicability show what their
/// views show the owner of what is used (`information_usage::depended`).
#[test]
fn short_name_views_of_usage_5_77() {
    let db = crate::information_usage::depended();
    let routine = "ROUTINE_CATALOG ROUTINE_SCHEMA ROUTINE_NAME";
    let specific = "SPECIFIC_CATALOG SPECIFIC_SCHEMA SPECIFIC_NAME";
    let table = "TABLE_CATALOG TABLE_SCHEMA TABLE_NAME";
    let applicable = "COLLATION_CATALOG COLLATION_SCHEMA COLLATION_NAME CHAR_SET_CATALOG=CHARACTER_SET_CATALOG \
                      CHAR_SET_SCHEMA=CHARACTER_SET_SCHEMA CHARACTER_SET_NAME";
    let views: [(&str, &str, &[&str]); 7] = [
        ("COLLATION_APPLIC_S", "COLLATION_CHARACTER_SET_APPLICABILITY", &[applicable]),
        ("COL_COL_USAGE", "COLUMN_COLUMN_USAGE", &[table, "COLUMN_NAME DEPENDENT_COLUMN"]),
        ("CONSTR_ROUT_USE_S", "CHECK_CONSTRAINT_ROUTINE_USAGE", &[CONSTRAINT, specific]),
        ("ROUT_ROUT_USAGE_S", "ROUTINE_ROUTINE_USAGE", &[specific, routine]),
        ("ROUT_SEQ_USAGE_S", "ROUTINE_SEQUENCE_USAGE", &[specific, "SEQUENCE_CATALOG SEQUENCE_SCHEMA SEQUENCE_NAME"]),
        ("ROUTINE_COL_USAGE", "ROUTINE_COLUMN_USAGE", &[specific, routine, table, "COLUMN_NAME"]),
        ("ROUT_TABLE_USAGE", "ROUTINE_TABLE_USAGE", &[specific, routine, table]),
    ];
    let a = db.session("A");
    for (short, long, columns) in views {
        assert!(same(&a, (short, long), columns) > 0, "no row of {short}");
    }
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.77] SQL_IMPL_INFO and
/// SQL_SIZING_PROFS have the columns of SQL_IMPLEMENTATION_INFO and
/// SQL_SIZING_PROFILES under the draft's short names, and show what those
/// show: no row, as neither has one.
#[test]
fn short_name_views_of_conformance_5_77() {
    let a = Database::new().session("A");
    let information =
        ["IMPL_INFO_ID=IMPLEMENTATION_INFO_ID IMPL_INFO_NAME=IMPLEMENTATION_INFO_NAME INTEGER_VALUE CHARACTER_VALUE COMMENTS"];
    assert_eq!(same(&a, ("SQL_IMPL_INFO", "SQL_IMPLEMENTATION_INFO"), &information), 0);
    let profiles = ["SIZING_ID SIZING_NAME PROFILE_ID PROFILE_NAME REQUIRED_VALUE COMMENTS"];
    assert_eq!(same(&a, ("SQL_SIZING_PROFS", "SQL_SIZING_PROFILES"), &profiles), 0);
}
