//! SQL/XML in the Information Schema [ISO/IEC 9075-14:2006 draft Clause
//! 20, Clause 21]: an XML type's modifiers and XML Schema in each view
//! that describes a data type, and the XML passing mechanism of a
//! routine's parameters and result.

use turborel::db::{Database, Session};

use crate::common::{rows, sorted};

/// The columns that describe an XML type, as each view names them [§21.1].
const XML_COLUMNS: &str = "XML_PRIMARY_MODIFIER, XML_SECONDARY_MODIFIER, XML_SCHEMA_CATALOG, \
                           XML_SCHEMA_SCHEMA, XML_SCHEMA_NAME, XML_SCHEMA_NAMESPACE, XML_SCHEMA_ELEMENT";

/// Schema A: domains DX of XML(DOCUMENT(ANY)) and DI of INTEGER; table T of
/// columns of each XML type, an INTEGER and one of each domain; table U of
/// a row of an XML field and an array of XML; type ST of an XML attribute
/// and a method of an XML parameter and result; function F of an XML
/// parameter and result, procedure PR of an XML parameter.
fn described() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE DOMAIN DX AS XML(DOCUMENT(ANY))
           CREATE DOMAIN DI AS INTEGER
           CREATE TABLE T (A XML, B XML(DOCUMENT(UNTYPED)), C XML(CONTENT(ANY)), D XML(SEQUENCE), E INTEGER, F DX, G DI)
           CREATE TABLE U (R ROW(F XML(CONTENT(UNTYPED)), N INTEGER), Y XML(DOCUMENT(UNTYPED)) ARRAY[3])
           CREATE TYPE ST AS (X XML(CONTENT(ANY)), N INTEGER) NOT FINAL
             METHOD M (P XML, Q INTEGER) RETURNS XML(DOCUMENT(UNTYPED)) CONTAINS SQL",
    )
    .unwrap();
    let a = db.session("A");
    for sql in [
        "CREATE FUNCTION F (P XML(DOCUMENT(UNTYPED)), Q INTEGER) RETURNS XML(CONTENT(ANY)) CONTAINS SQL RETURN P",
        "CREATE FUNCTION G (Q INTEGER) RETURNS INTEGER CONTAINS SQL RETURN Q",
        "CREATE PROCEDURE PR (IN P XML) CONTAINS SQL BEGIN END",
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

/// The names of the columns of the Information Schema's view `view` from
/// `first` on, `count` of them, in order.
fn columns_of(s: &Session, view: &str, (first, count): (&str, usize)) -> Vec<String> {
    let names = query(
        s,
        &format!(
            "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA = 'INFORMATION_SCHEMA' AND TABLE_NAME = '{view}' \
             ORDER BY ORDINAL_POSITION"
        ),
    );
    names.into_iter().skip_while(|n| n != first).take(count).collect()
}

/// `names`, as owned strings.
fn owned(names: &[&str]) -> Vec<String> {
    names.iter().map(|&n| n.to_owned()).collect()
}

/// The names of the seven columns of `XML_COLUMNS`, after `after`.
fn xml_columns_after(after: &str) -> Vec<String> {
    owned(&[
        after,
        "XML_PRIMARY_MODIFIER",
        "XML_SECONDARY_MODIFIER",
        "XML_SCHEMA_CATALOG",
        "XML_SCHEMA_SCHEMA",
        "XML_SCHEMA_NAME",
        "XML_SCHEMA_NAMESPACE",
        "XML_SCHEMA_ELEMENT",
    ])
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §20.4, §20.5, §21.1 Description
/// 1] COLUMNS shows each column's XML type's modifiers, its own or its
/// domain's: XML alone XML(SEQUENCE), of no secondary modifier, DOCUMENT
/// and CONTENT of theirs; DOMAINS each domain's. The XML Schema columns
/// are null, as no type is of XMLSCHEMA, and every column is null of a
/// type that is not XML. Each view has the columns last, as the draft adds
/// them.
#[test]
fn columns_and_domains_20_4() {
    let db = described();
    let a = db.session("A");
    let columns = format!("SELECT COLUMN_NAME, DATA_TYPE, {XML_COLUMNS} FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T'");
    assert_eq!(
        sorted(&a, &columns),
        [
            "A,XML,SEQUENCE,NULL,NULL,NULL,NULL,NULL,NULL",
            "B,XML,DOCUMENT,UNTYPED,NULL,NULL,NULL,NULL,NULL",
            "C,XML,CONTENT,ANY,NULL,NULL,NULL,NULL,NULL",
            "D,XML,SEQUENCE,NULL,NULL,NULL,NULL,NULL,NULL",
            "E,INTEGER,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
            "F,XML,DOCUMENT,ANY,NULL,NULL,NULL,NULL,NULL",
            "G,INTEGER,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
        ]
    );
    let domains = format!("SELECT DOMAIN_NAME, DATA_TYPE, {XML_COLUMNS} FROM INFORMATION_SCHEMA.DOMAINS");
    assert_eq!(sorted(&a, &domains), ["DI,INTEGER,NULL,NULL,NULL,NULL,NULL,NULL,NULL", "DX,XML,DOCUMENT,ANY,NULL,NULL,NULL,NULL,NULL"]);
    assert_eq!(columns_of(&a, "COLUMNS", ("DTD_IDENTIFIER", 9)), xml_columns_after("DTD_IDENTIFIER"));
    assert_eq!(columns_of(&a, "DOMAINS", ("DTD_IDENTIFIER", 9)), xml_columns_after("DTD_IDENTIFIER"));
    let types = "SELECT COLUMN_NAME, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS \
                 WHERE TABLE_SCHEMA = 'INFORMATION_SCHEMA' AND TABLE_NAME = 'DOMAINS' AND COLUMN_NAME LIKE 'XML%'";
    assert_eq!(
        sorted(&a, types),
        [
            "XML_PRIMARY_MODIFIER,CHARACTER VARYING,32767",
            "XML_SCHEMA_CATALOG,CHARACTER VARYING,128",
            "XML_SCHEMA_ELEMENT,CHARACTER VARYING,32767",
            "XML_SCHEMA_NAME,CHARACTER VARYING,128",
            "XML_SCHEMA_NAMESPACE,CHARACTER VARYING,32767",
            "XML_SCHEMA_SCHEMA,CHARACTER VARYING,128",
            "XML_SECONDARY_MODIFIER,CHARACTER VARYING,32767",
        ]
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §20.6, §20.7, §21.1 Description
/// 1] ELEMENT_TYPES shows an array's XML element type's modifiers, and
/// FIELDS a row type's XML field's; of a field of another type, null.
#[test]
fn element_types_and_fields_20_6() {
    let db = described();
    let a = db.session("A");
    let elements = format!("SELECT OBJECT_NAME, DATA_TYPE, {XML_COLUMNS} FROM INFORMATION_SCHEMA.ELEMENT_TYPES");
    assert_eq!(query(&a, &elements), ["U,XML,DOCUMENT,UNTYPED,NULL,NULL,NULL,NULL,NULL"]);
    let fields = format!("SELECT FIELD_NAME, DATA_TYPE, {XML_COLUMNS} FROM INFORMATION_SCHEMA.FIELDS");
    assert_eq!(sorted(&a, &fields), ["F,XML,CONTENT,UNTYPED,NULL,NULL,NULL,NULL,NULL", "N,INTEGER,NULL,NULL,NULL,NULL,NULL,NULL,NULL"]);
    assert_eq!(columns_of(&a, "ELEMENT_TYPES", ("DTD_IDENTIFIER", 9)), xml_columns_after("DTD_IDENTIFIER"));
    assert_eq!(columns_of(&a, "FIELDS", ("DTD_IDENTIFIER", 9)), xml_columns_after("DTD_IDENTIFIER"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §20.3, §20.8, §20.9, §21.1
/// Description 1] ATTRIBUTES shows an XML attribute's modifiers;
/// METHOD_SPECIFICATIONS each method's XML result type's, its observer's
/// too; METHOD_SPECIFICATION_PARAMETERS each XML parameter's, the
/// mutator's too; each null of another type.
#[test]
fn attributes_and_methods_20_3() {
    let db = described();
    let a = db.session("A");
    let attributes = format!("SELECT ATTRIBUTE_NAME, {XML_COLUMNS} FROM INFORMATION_SCHEMA.ATTRIBUTES");
    assert_eq!(sorted(&a, &attributes), ["N,NULL,NULL,NULL,NULL,NULL,NULL,NULL", "X,CONTENT,ANY,NULL,NULL,NULL,NULL,NULL"]);
    let methods = format!("SELECT SPECIFIC_NAME, DATA_TYPE, {XML_COLUMNS} FROM INFORMATION_SCHEMA.METHOD_SPECIFICATIONS");
    assert_eq!(
        sorted(&a, &methods),
        [
            "ST_M_1,XML,DOCUMENT,UNTYPED,NULL,NULL,NULL,NULL,NULL",
            "ST_N_MUTATOR,USER-DEFINED,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
            "ST_N_OBSERVER,INTEGER,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
            "ST_X_MUTATOR,USER-DEFINED,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
            "ST_X_OBSERVER,XML,CONTENT,ANY,NULL,NULL,NULL,NULL,NULL",
        ]
    );
    let parameters = format!(
        "SELECT SPECIFIC_NAME, PARAMETER_NAME, {XML_COLUMNS} FROM INFORMATION_SCHEMA.METHOD_SPECIFICATION_PARAMETERS \
         WHERE SPECIFIC_NAME IN ('ST_M_1', 'ST_X_MUTATOR')"
    );
    assert_eq!(
        sorted(&a, &parameters),
        [
            "ST_M_1,P,SEQUENCE,NULL,NULL,NULL,NULL,NULL,NULL",
            "ST_M_1,Q,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
            "ST_X_MUTATOR,ATTR,CONTENT,ANY,NULL,NULL,NULL,NULL,NULL",
        ]
    );
    let after = "IS_DERIVED_REFERENCE_ATTRIBUTE";
    assert_eq!(columns_of(&a, "ATTRIBUTES", (after, 9)), xml_columns_after(after));
    assert_eq!(columns_of(&a, "METHOD_SPECIFICATIONS", ("AS_LOCATOR", 9)), xml_columns_after("AS_LOCATOR"));
    assert_eq!(columns_of(&a, "METHOD_SPECIFICATION_PARAMETERS", ("DTD_IDENTIFIER", 9)), xml_columns_after("DTD_IDENTIFIER"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §20.10, §20.11, §21.2
/// Description 3, §21.3 Description 3, §12.7 SR8b] ROUTINES shows an XML
/// result type's modifiers and PARAMETERS an XML parameter's; a parameter
/// or result of an XML type is passed by its implicit mechanism, BY REF,
/// and another by none. An SQL routine has no associated string type or
/// XML option, so those columns are null [§12.7 SR1b].
#[test]
fn routines_and_parameters_20_10() {
    let db = described();
    let a = db.session("A");
    let routines = format!(
        "SELECT ROUTINE_NAME, DATA_TYPE, {XML_COLUMNS}, XML_CHAR_TYPE, XML_CHAR_SET_NAME, XML_CHAR_COLL_NAME, XML_CHAR_DTD_ID, \
         XML_OPTION, XML_RETURN_MECHANISM FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_NAME IN ('F', 'G', 'PR', 'X')"
    );
    assert_eq!(
        sorted(&a, &routines),
        [
            "F,XML,CONTENT,ANY,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,BY REF",
            "G,INTEGER,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
            "PR,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
            "X,USER-DEFINED,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
            "X,XML,CONTENT,ANY,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,BY REF",
        ]
    );
    let parameters = format!(
        "SELECT R.ROUTINE_NAME, P.PARAMETER_NAME, {}, P.XML_CHAR_TYPE, P.XML_CHAR_MAX_LEN, P.XML_CHAR_SET_NAME, \
         P.XML_OPTION, P.XML_PASSING_MECHANISM FROM INFORMATION_SCHEMA.PARAMETERS AS P JOIN INFORMATION_SCHEMA.ROUTINES AS R \
         USING (SPECIFIC_CATALOG, SPECIFIC_SCHEMA, SPECIFIC_NAME) WHERE R.ROUTINE_NAME IN ('F', 'G', 'PR')",
        XML_COLUMNS.replace("XML_", "P.XML_")
    );
    assert_eq!(
        sorted(&a, &parameters),
        [
            "F,P,DOCUMENT,UNTYPED,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,BY REF",
            "F,Q,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
            "G,Q,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL",
            "PR,P,SEQUENCE,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,BY REF",
        ]
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §20.10, §20.11, §20.15] ROUTINES
/// has an XML type's columns after DTD_IDENTIFIER, and the associated
/// string type's, the XML option and the XML return mechanism last, each
/// once; PARAMETERS has them all last, in the draft's order.
#[test]
fn routine_columns_in_their_places_20_11() {
    let db = described();
    let a = db.session("A");
    assert_eq!(columns_of(&a, "ROUTINES", ("DTD_IDENTIFIER", 9)), [xml_columns_after("DTD_IDENTIFIER"), owned(&["ROUTINE_BODY"])].concat());
    let string_type = [
        "XML_CHAR_TYPE",
        "XML_CHAR_MAX_LEN",
        "XML_CHAR_OCT_LEN",
        "XML_CHAR_SET_CAT",
        "XML_CHAR_SET_SCH",
        "XML_CHAR_SET_NAME",
        "XML_CHAR_COLL_CAT",
        "XML_CHAR_COLL_SCH",
        "XML_CHAR_COLL_NAME",
        "XML_CHAR_DTD_ID",
        "XML_OPTION",
    ];
    let last = [owned(&["SECURITY_TYPE"]), owned(&string_type), owned(&["XML_RETURN_MECHANISM"])].concat();
    assert_eq!(columns_of(&a, "ROUTINES", ("SECURITY_TYPE", 14)), last);
    let parameters = [xml_columns_after("DTD_IDENTIFIER"), owned(&string_type), owned(&["XML_PASSING_MECHANISM"])].concat();
    assert_eq!(columns_of(&a, "PARAMETERS", ("DTD_IDENTIFIER", 21)), parameters);
    let named = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.COLUMNS \
                 WHERE TABLE_SCHEMA = 'INFORMATION_SCHEMA' AND TABLE_NAME = 'ROUTINES' AND COLUMN_NAME = 'XML_CHAR_TYPE'";
    assert_eq!(query(&a, named), ["1"]);
}
