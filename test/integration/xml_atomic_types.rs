//! User-defined atomic types in XMLQUERY [ISO/IEC 9075-14:2006 draft
//! §6.18]: the simple types of the atomic variety of an imported schema
//! [XQuery §4.10] as atomic item types [XQuery §2.5.4.2], cast targets
//! and constructor functions [XQuery §3.12.3, §3.12.5], and the types of
//! the typed values a validation gives [XQueryDM §3.3.1.2].

use turborel::db::Database;

use crate::xml_schema_import::{code, value};
use crate::xml_validate::{one, registered};

/// A schema document of target namespace `urn:t`: `small` restricting
/// `xs:int`, `tiny` restricting it, `code` and `three` of patterns, `qn`
/// restricting `xs:QName`, a list `smalls` of `small`, a union `either`
/// of `small` and `code`, and an element `r` of them.
const TYPES: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:t="urn:t" targetNamespace="urn:t" elementFormDefault="qualified"><xs:simpleType name="small"><xs:restriction base="xs:int"><xs:maxInclusive value="9"/></xs:restriction></xs:simpleType><xs:simpleType name="tiny"><xs:restriction base="t:small"><xs:maxInclusive value="3"/></xs:restriction></xs:simpleType><xs:simpleType name="code"><xs:restriction base="xs:string"><xs:pattern value="[A-Z]{3}"/></xs:restriction></xs:simpleType><xs:simpleType name="three"><xs:restriction base="xs:integer"><xs:pattern value="[0-9]{3}"/></xs:restriction></xs:simpleType><xs:simpleType name="qn"><xs:restriction base="xs:QName"><xs:pattern value="t:.*"/></xs:restriction></xs:simpleType><xs:simpleType name="smalls"><xs:list itemType="t:small"/></xs:simpleType><xs:simpleType name="either"><xs:union memberTypes="t:small t:code"/></xs:simpleType><xs:element name="r"><xs:complexType><xs:sequence><xs:element name="s" type="t:small"/><xs:element name="l" type="t:smalls"/><xs:element name="e" type="t:either" maxOccurs="unbounded"/></xs:sequence><xs:attribute name="c" type="t:code"/></xs:complexType></xs:element></xs:schema>"#;

/// The prolog that imports TYPES, its prefix `t`.
const IMPORT: &str = "import schema namespace t = \"urn:t\"; ";

/// A document TYPES validates.
const DOC: &str = r#"<r xmlns="urn:t" c="ABC"><s>5</s><l>1 2 3</l><e>4</e><e>XYZ</e></r>"#;

/// A database of `registered`'s, A also registering TYPES as A.TYPES.
fn database() -> Database {
    let db = registered();
    let a = db.session("A");
    a.run(&format!("CREATE XML SCHEMA TYPES AS '{TYPES}'")).unwrap();
    a.commit().unwrap();
    db
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §2.5.4.2,
/// §3.12.1] a simple type of the atomic variety of an imported schema is
/// an atomic item type: a value of it matches it, the types it restricts
/// and the built-in types above them; a value of a type restricting it
/// matches it; a value of a built-in type, or of a type it restricts,
/// does not; a list type is no atomic item type, err:XPST0051.
#[test]
fn user_defined_types_are_atomic_item_types_2_5_4_2() {
    let db = database();
    let a = db.session("A");
    let cases = [
        ("t:small(5) instance of t:small", "true"),
        ("t:small(5) instance of xs:int", "true"),
        ("t:small(5) instance of xs:integer", "true"),
        ("t:small(5) instance of xdt:anyAtomicType", "true"),
        ("t:small(5) instance of t:tiny", "false"),
        ("t:small(5) instance of xs:short", "false"),
        ("t:tiny(2) instance of t:small", "true"),
        ("5 instance of t:small", "false"),
        ("xs:int(5) instance of t:small", "false"),
        ("t:code(\"ABC\") instance of xs:string", "true"),
        ("t:code(\"ABC\") instance of t:small", "false"),
    ];
    for (query, expected) in cases {
        assert_eq!(value(&a, &format!("{IMPORT}{query}"), DOC), expected, "{query}");
    }
    assert_eq!(code(&a, &format!("{IMPORT}1 instance of t:smalls"), DOC), "err:XPST0051");
    assert_eq!(code(&a, &format!("{IMPORT}1 instance of t:nosuch"), DOC), "err:XPST0051");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQueryDM §3.3.1.2] the
/// typed value of a node a validation typed is of its user-defined type:
/// an element's and an attribute's of an atomic type, a list's items of
/// its item type, each item of a union of the member type that validated
/// it; an unvalidated node's untyped.
#[test]
fn typed_values_keep_user_defined_types_3_3_1_2() {
    let db = database();
    let a = db.session("A");
    let typed = |path: &str, t: &str| {
        format!("{IMPORT}let $r := (validate {{ . }})/t:r return every $v in data({path}) satisfies $v instance of {t}")
    };
    let cases = [
        ("$r/t:s", "t:small", "true"),
        ("$r/@c", "t:code", "true"),
        ("$r/t:l", "t:small", "true"),
        ("$r/t:e[1]", "t:small", "true"),
        ("$r/t:e[2]", "t:code", "true"),
        ("$r/t:e[2]", "t:small", "false"),
        ("/t:r/t:s", "t:small", "false"),
    ];
    for (path, t, expected) in cases {
        assert_eq!(value(&a, &typed(path, t), DOC), expected, "{path} of {t}");
    }
    let untyped = format!("{IMPORT}data(/t:r/t:s) instance of xdt:untypedAtomic");
    assert_eq!(value(&a, &untyped, DOC), "true");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.12.3, §3.12.4,
/// §3.12.5] [XQueryFO §17.2, §17.4] a cast to a user-defined atomic type
/// checks its facets: of a string, its whitespace collapsed and its
/// patterns matched before it is cast; of another value, as cast to the
/// built-in type, its patterns matched against its canonical form; one
/// the facets refuse err:FORG0001, and not castable; a cast from it to a
/// built-in type gives a value of that type alone; the constructor
/// function is the cast.
#[test]
fn casts_to_user_defined_types_check_facets_3_12_3() {
    let db = database();
    let a = db.session("A");
    let cases = [
        ("\"5\" cast as t:small", "5"),
        ("t:small(\" 7 \")", "7"),
        ("\"007\" cast as t:three", "7"),
        ("\"10\" castable as t:small", "false"),
        ("\"9\" castable as t:small", "true"),
        ("t:tiny(t:small(2)) instance of t:tiny", "true"),
        ("(t:small(5) cast as xs:int) instance of t:small", "false"),
        ("(t:small(5) cast as xs:int) instance of xs:int", "true"),
        ("t:small(4) + 1", "5"),
        ("t:small(()) instance of empty-sequence()", "true"),
    ];
    for (query, expected) in cases {
        assert_eq!(value(&a, &format!("{IMPORT}{query}"), DOC), expected, "{query}");
    }
    let refused = ["\"10\" cast as t:small", "t:code(\"abc\")", "7 cast as t:three", "t:tiny(t:small(5))", "\"x\" cast as t:small"];
    for query in refused {
        assert_eq!(code(&a, &format!("{IMPORT}{query}"), DOC), "err:FORG0001", "{query}");
    }
    assert_eq!(code(&a, &format!("{IMPORT}1 cast as t:smalls"), DOC), "err:XPST0080");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.12.3, §3.12.5]
/// a cast to a type derived from `xs:QName` is of a string literal alone,
/// err:XPST0083 of another, its prefix resolved by the statically known
/// namespaces, err:FONS0004 of one unbound, its facets checked,
/// err:FORG0001.
#[test]
fn casts_to_types_derived_from_qname_3_12_3() {
    let db = database();
    let a = db.session("A");
    assert_eq!(value(&a, &format!("{IMPORT}(\"t:a\" cast as t:qn) instance of t:qn"), DOC), "true");
    assert_eq!(value(&a, &format!("{IMPORT}namespace-uri-from-QName(t:qn(\"t:a\"))"), DOC), "urn:t");
    assert_eq!(value(&a, &format!("{IMPORT}\"xs:a\" castable as t:qn"), DOC), "false");
    assert_eq!(code(&a, &format!("{IMPORT}\"xs:a\" cast as t:qn"), DOC), "err:FORG0001");
    assert_eq!(code(&a, &format!("{IMPORT}\"u:a\" cast as t:qn"), DOC), "err:FONS0004");
    assert_eq!(code(&a, &format!("{IMPORT}let $n := \"t:a\" return $n cast as t:qn"), DOC), "err:XPST0083");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.1.5, §3.12.6,
/// §4.15] a user-defined atomic type in a sequence type: treat as, and a
/// function's parameter and result, an untyped argument cast to it by the
/// function conversion rules and checked against its facets; a value of
/// a built-in type not converted, err:XPTY0004.
#[test]
fn sequence_types_of_user_defined_types_3_1_5() {
    let db = database();
    let a = db.session("A");
    let f = format!("{IMPORT}declare function local:f($x as t:small) as t:small {{ $x }}; ");
    assert_eq!(value(&a, &format!("{IMPORT}t:small(3) treat as t:small"), DOC), "3");
    assert_eq!(code(&a, &format!("{IMPORT}3 treat as t:small"), DOC), "err:XPDY0050");
    assert_eq!(value(&a, &format!("{f}local:f(t:small(3))"), DOC), "3");
    assert_eq!(value(&a, &format!("{f}local:f(xdt:untypedAtomic(\"4\")) instance of t:small"), DOC), "true");
    assert_eq!(code(&a, &format!("{f}local:f(xdt:untypedAtomic(\"12\"))"), DOC), "err:FORG0001");
    assert_eq!(code(&a, &format!("{f}local:f(3)"), DOC), "err:XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18, §10.2 GR1b] RULES.md D106
/// an atomic value of a user-defined type stored in a column of
/// XML(SEQUENCE) keeps its type, so a query importing its schema reads it
/// back of that type.
#[test]
fn stored_atomic_values_keep_their_types() {
    let db = database();
    let a = db.session("A");
    a.run("CREATE TABLE V (X XML(SEQUENCE))").unwrap();
    let made = format!("XMLQUERY('{IMPORT}(t:small(5), 6)' RETURNING SEQUENCE BY VALUE EMPTY ON EMPTY)");
    a.run(&format!("INSERT INTO V VALUES ({made})")).unwrap();
    a.commit().unwrap();
    let read = |t: &str| {
        let query = format!("{IMPORT}string-join(for $v in $x return string($v instance of {t}), \" \")");
        format!("XMLCAST(XMLQUERY('{query}' PASSING BY REF X AS \"x\" RETURNING SEQUENCE EMPTY ON EMPTY) AS VARCHAR(20))")
    };
    assert_eq!(one(&a, &format!("(SELECT {} FROM V)", read("t:small"))), "true false");
    assert_eq!(one(&a, &format!("(SELECT {} FROM V)", read("xs:int"))), "true false");
}
