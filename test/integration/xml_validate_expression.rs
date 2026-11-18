//! XQuery's validate expression [XQuery §3.13] evaluated by XMLQUERY
//! [ISO/IEC 9075-14:2006 draft §6.18], against the in-scope schema
//! definitions [§10.20 SR2e]: the registered XML Schemas the current user
//! may use, of each namespace the one of the least name.

use turborel::db::Session;
use turborel::error::SqlError;

use crate::xml_validate::{one, registered};

/// The value of XQuery `query` of the context item the document `xml`,
/// serialized.
fn value(s: &Session, query: &str, xml: &str) -> String {
    one(s, &format!("XMLSERIALIZE(CONTENT {} AS VARCHAR(200))", xmlquery(query, xml)))
}

/// XMLQUERY of `query`, its context item the document `xml`.
fn xmlquery(query: &str, xml: &str) -> String {
    let query = query.replace('\'', "''");
    format!("XMLQUERY('{query}' PASSING BY REF XMLPARSE(DOCUMENT '{xml}' PRESERVE WHITESPACE) RETURNING SEQUENCE EMPTY ON EMPTY)")
}

/// The XQuery error code `query` of the context item `xml` fails with, as
/// its message names it, after SQLSTATE 10000; rolled back.
fn code(s: &Session, query: &str, xml: &str) -> String {
    let sql = format!("VALUES ({})", xmlquery(query, xml));
    let e: SqlError = crate::common::fetched(s, &sql).expect_err(&sql);
    s.rollback().unwrap();
    assert_eq!(e.sqlstate().as_str(), "10000", "{sql}: {e:?}");
    e.msg.split_whitespace().find(|w| w.starts_with("err:")).map(|w| w.trim_end_matches(':').to_owned()).unwrap_or(e.msg)
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.20 SR2e] [XQuery §3.13] a
/// document node or an element node validated, strict by default, against
/// the registered XML Schema of its element's namespace, of no namespace
/// too: a new node, of no parent, whose elements are typed, so their typed
/// values are numbers.
#[test]
fn strict_validation_types_its_result_3_13() {
    let db = registered();
    let a = db.session("A");
    assert_eq!(value(&a, "data((validate { . })/n) + 1", "<n>7</n>"), "8");
    assert_eq!(value(&a, "data((validate strict { . })/n) instance of xs:integer", "<n>7</n>"), "true");
    assert_eq!(value(&a, "data(/n) instance of xs:integer", "<n>7</n>"), "false", "unvalidated");
    assert_eq!(value(&a, "validate { . } instance of document-node(element(n, xs:integer))", "<n>7</n>"), "true");
    assert_eq!(value(&a, "let $n := validate { n } return ($n instance of element(n, xs:integer), count($n/..))", "<n>7</n>"), "true 0");
    let order = r#"<order xmlns="urn:o"><qty>2</qty></order>"#;
    assert_eq!(value(&a, "declare namespace o = \"urn:o\"; data((validate { . })/o:order/o:qty) * 2", order), "4");
    assert_eq!(value(&a, "validate { . }", order), order);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQuery §3.13] what
/// validate raises, SQLSTATE 10000: a node not valid err:XQDY0027; in
/// strict mode an element of no top-level declaration, or of a namespace
/// no registered XML Schema in scope has, err:XQDY0084; an operand not one
/// document or element node err:XQTY0030; a document node of two elements
/// err:XQDY0061.
#[test]
fn what_validate_raises_3_13() {
    let db = registered();
    let a = db.session("A");
    assert_eq!(code(&a, "validate { . }", "<n>one</n>"), "err:XQDY0027");
    assert_eq!(code(&a, "validate { . }", r#"<order xmlns="urn:o"><qty>2</qty><qty>3</qty></order>"#), "err:XQDY0027");
    assert_eq!(code(&a, "validate strict { . }", "<m>1</m>"), "err:XQDY0084");
    assert_eq!(code(&a, "validate { . }", r#"<x xmlns="urn:x"/>"#), "err:XQDY0084");
    assert_eq!(code(&a, "validate { 1 }", "<n>1</n>"), "err:XQTY0030");
    assert_eq!(code(&a, "validate { (n, n) }", "<n>1</n>"), "err:XQTY0030");
    assert_eq!(code(&a, "validate { n/text() }", "<n>1</n>"), "err:XQTY0030");
    assert_eq!(code(&a, "validate lax { document { <n>1</n>, <n>2</n> } }", "<n>1</n>"), "err:XQDY0061");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.20 SR2e] [XQuery §3.13]
/// [Schema1 §5.2 item 3] lax validation: an
/// element of no top-level declaration, of a namespace with or without a
/// registered XML Schema in scope, is laxly assessed, annotated
/// `xs:anyType`, its untyped children too, with no error; a child that is
/// declared is validated by its declaration, typed if valid, err:XQDY0027
/// if not.
#[test]
fn lax_validation_3_13() {
    let db = registered();
    let a = db.session("A");
    let anything = r#"<x xmlns="urn:x" a="1"><y>2</y></x>"#;
    assert_eq!(value(&a, "validate lax { . }", anything), anything);
    let typed = "declare namespace x = \"urn:x\"; let $v := validate lax { . } return ($v/x:x instance of element(*, xs:anyType), data($v/x:x/x:y) instance of xdt:untypedAtomic, data($v/x:x/@a) instance of xdt:untypedAtomic)";
    assert_eq!(value(&a, typed, anything), "true true true");
    let nested = "let $v := validate lax { . } return ($v/m instance of element(m, xs:anyType), data($v/m/n) instance of xs:integer)";
    assert_eq!(value(&a, nested, "<m><n>5</n></m>"), "true true");
    assert_eq!(code(&a, "validate lax { . }", "<m><n>five</n></m>"), "err:XQDY0027");
    assert_eq!(value(&a, "data((validate lax { . })/n) + 1", "<n>7</n>"), "8", "declared, so strictly");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.20 SR2e, §11.6 AR1]
/// implementation-defined: a registered XML Schema the current user may
/// not use is not in scope, so strict validation by it is err:XQDY0084 and
/// lax validation types nothing; of two of one namespace, the one of the
/// least name is in scope.
#[test]
fn which_schemas_are_in_scope_10_20() {
    let db = registered();
    let b = db.session("B");
    assert_eq!(code(&b, "validate { . }", "<n>7</n>"), "err:XQDY0084");
    assert_eq!(value(&b, "data((validate lax { . })/n) instance of xdt:untypedAtomic", "<n>7</n>"), "true");
    let a = db.session("A");
    let strings = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="n" type="xs:string"/></xs:schema>"#;
    a.run(&format!("CREATE XML SCHEMA Q AS '{strings}'")).unwrap();
    a.run(&format!("CREATE XML SCHEMA B.N AS '{strings}'")).unwrap_err();
    a.rollback().unwrap();
    a.run(&format!("CREATE XML SCHEMA Q AS '{strings}'")).unwrap();
    a.commit().unwrap();
    assert_eq!(code(&a, "validate { . }", "<n>one</n>"), "err:XQDY0027", "A.P, not A.Q");
    b.run(&format!("CREATE XML SCHEMA N AS '{strings}'")).unwrap();
    b.commit().unwrap();
    assert_eq!(value(&b, "data((validate { . })/n) instance of xs:string", "<n>one</n>"), "true", "B.N, B's own");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.7.1.3,
/// §3.13, §4.6] an element validated then copied by a constructor keeps
/// its type annotation, and so its typed value, in construction mode
/// preserve, the default, the constructed element itself `xs:anyType`,
/// not `xdt:untyped`; in strip the copy is `xdt:untyped`.
#[test]
fn construction_modes_of_validated_copies_3_7_1_3() {
    let db = registered();
    let a = db.session("A");
    let copy = "let $v := validate { n } return <w>{$v}</w>";
    let checks = |prolog: &str| {
        format!(
            "{prolog}({copy})/n instance of element(n, xs:integer), ({copy})/n instance of element(n, xdt:untyped), data(({copy})/n) instance of xs:integer"
        )
    };
    assert_eq!(value(&a, &checks(""), "<n>7</n>"), "true false true");
    assert_eq!(value(&a, &format!("({copy}) instance of element(w, xdt:untyped)"), "<n>7</n>"), "false");
    assert_eq!(value(&a, &checks("declare construction strip; "), "<n>7</n>"), "false true false");
}

/// The schema document of target namespace urn:t, of global elements
/// `sizes`, a list of `xs:integer`; `mark`, of empty content; and
/// `maybe`, a nillable `xs:integer`.
const TYPINGS: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:t" elementFormDefault="qualified"><xs:element name="sizes"><xs:simpleType><xs:list itemType="xs:integer"/></xs:simpleType></xs:element><xs:element name="mark"><xs:complexType/></xs:element><xs:element name="maybe" type="xs:integer" nillable="true"/></xs:schema>"#;

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, §6.20] [XQuery
/// §2.4.2, §3.13] [XQueryDM §3.3.1.2, §5.15, §6.2.4] the typed value of a
/// validated element, as fn:data gives it: of a list type, an atomic
/// value of its item type for each item; of empty content, and of a
/// nilled element, the empty sequence; by the validate expression and by
/// XMLVALIDATE alike.
#[test]
fn typed_values_of_lists_and_empty_content_3_13() {
    let db = registered();
    let a = db.session("A");
    a.run(&format!("CREATE XML SCHEMA T AS '{TYPINGS}'")).unwrap();
    a.commit().unwrap();
    let t = "declare namespace t = \"urn:t\"; ";
    let sizes = r#"<sizes xmlns="urn:t"> 1  20 300 </sizes>"#;
    assert_eq!(value(&a, &format!("{t}data((validate {{ . }})/t:sizes)"), sizes), "1 20 300");
    assert_eq!(
        value(&a, &format!("{t}sum(data((validate {{ . }})/t:sizes)), data((validate {{ . }})/t:sizes) instance of xs:integer+"), sizes),
        "321 true"
    );
    assert_eq!(value(&a, &format!("{t}empty(data((validate {{ . }})/t:mark))"), r#"<mark xmlns="urn:t"/>"#), "true");
    let nilled = r#"<maybe xmlns="urn:t" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:nil="true"/>"#;
    assert_eq!(value(&a, &format!("{t}empty(data((validate {{ . }})/t:maybe)), nilled((validate {{ . }})/t:maybe)"), nilled), "true true");
    let validated = format!(
        "XMLSERIALIZE(CONTENT XMLQUERY('declare namespace t = \"urn:t\"; count(data(/t:sizes))' PASSING BY REF XMLVALIDATE(DOCUMENT XMLPARSE(DOCUMENT '{sizes}' PRESERVE WHITESPACE) ACCORDING TO XMLSCHEMA ID A.T) EMPTY ON EMPTY) AS VARCHAR(10))"
    );
    assert_eq!(one(&a, &validated), "3");
}
