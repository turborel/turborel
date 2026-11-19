//! Tests of the permanently registered XML Schemas' components.

use super::*;
use crate::xml::Node;
use crate::xml::schema::{INVALID, Mode, UNDECLARED, validate};

/// What validating `text` by the permanently registered XML Schema of
/// `namespace`, strictly, gives: `VALID`, or its failure's code.
fn validated(namespace: &str, text: &str) -> &'static str {
    let schema = components(namespace).unwrap();
    let node = Node::root(parse(text, Syntax::Document, Whitespace::Preserve).unwrap());
    validate(&schema, &node, Mode::Strict).unwrap().map_or_else(|f| f.code, |_| VALID)
}

/// What `validated` gives of a valid document.
const VALID: &str = "valid";

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.6 Table 1] [Schema1 Appendix
/// A, §3.4.7, §3.14.7] [Schema2 Appendix A] the components of each
/// permanently registered XML Schema are read from its documents, the
/// Schema for Schemas with the datatypes it includes and the XML namespace
/// it imports, its definitions of the built-in names the built-in ones;
/// of another namespace there are none.
#[test]
fn each_permanent_schema_reads() {
    for namespace in [XS_NAMESPACE, XSI_NAMESPACE, SQLXML_NAMESPACE] {
        assert!(components(namespace).is_some(), "{namespace}");
    }
    assert!(components("urn:x").is_none());
    let xs = components(XS_NAMESPACE).unwrap();
    for element in ["schema", "element", "complexType", "simpleType", "restriction", "minInclusive", "annotation"] {
        assert!(xs.element_names.contains_key(&super::super::Expanded::new(Some(XS_NAMESPACE), element)), "{element}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 Appendix A,
/// §5.1] a schema document is valid by the Schema for Schemas; one with an
/// attribute its element does not take, a top-level element of an
/// occurrence, or children out of their order is not; an element of no
/// declaration of the namespace is undeclared.
#[test]
fn schema_documents_validate_by_the_schema_for_schemas() {
    let xs = r#"xmlns:xs="http://www.w3.org/2001/XMLSchema""#;
    let valid = format!(
        r#"<xs:schema {xs} targetNamespace="urn:o" elementFormDefault="qualified" xml:lang="en"><xs:annotation><xs:documentation>orders</xs:documentation></xs:annotation><xs:simpleType name="qty"><xs:restriction base="xs:integer"><xs:minInclusive value="1"/></xs:restriction></xs:simpleType><xs:element name="order"><xs:complexType><xs:sequence><xs:element name="qty" type="xs:integer" maxOccurs="unbounded"/></xs:sequence><xs:attribute name="id" type="xs:ID"/></xs:complexType><xs:key name="k"><xs:selector xpath="qty"/><xs:field xpath="."/></xs:key></xs:element></xs:schema>"#
    );
    assert_eq!(validated(XS_NAMESPACE, &valid), VALID);
    for invalid in [
        format!(r#"<xs:schema {xs}><xs:element name="a" colour="red"/></xs:schema>"#),
        format!(r#"<xs:schema {xs}><xs:element name="a" minOccurs="0"/></xs:schema>"#),
        format!(r#"<xs:schema {xs}><xs:element name="a"/><xs:import namespace="urn:x"/></xs:schema>"#),
        format!(r#"<xs:schema {xs} elementFormDefault="sometimes"/>"#),
    ] {
        assert_eq!(validated(XS_NAMESPACE, &invalid), INVALID, "{invalid}");
    }
    assert_eq!(validated(XS_NAMESPACE, &format!("<xs:nothing {xs}/>")), UNDECLARED);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §22.1, §4.2.6] [Schema1 §3.2.6,
/// §3.2.7] an `sqlxml:sqltype` of SQL/XML's XML Schema is valid of a kind
/// it lists and not of another; the XML Schema instance namespace declares
/// no element.
#[test]
fn sql_xml_and_instance_schemas_validate() {
    let sqlxml = r#"xmlns:sqlxml="http://standards.iso.org/iso/9075/2003/sqlxml""#;
    assert_eq!(validated(SQLXML_NAMESPACE, &format!(r#"<sqlxml:sqltype {sqlxml} kind="PREDEFINED" name="INTEGER"/>"#)), VALID);
    assert_eq!(validated(SQLXML_NAMESPACE, &format!(r#"<sqlxml:sqltype {sqlxml} kind="TABLE"/>"#)), INVALID);
    let xsi = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;
    assert_eq!(validated(XSI_NAMESPACE, &format!("<xsi:type {xsi}/>")), UNDECLARED);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.6, §21.6, §21.7] the
/// namespaces, global element declarations and determinism the catalog's
/// constants give each permanently registered XML Schema, which the
/// Information Schema shows, are those of its components.
#[test]
fn the_catalogs_constants_match_the_components() {
    for p in &crate::catalog::xml_schemas::PERMANENT {
        let schema = components(p.target_namespace).unwrap();
        for namespace in p.namespaces {
            let mut listed: Vec<&str> = namespace.elements.to_vec();
            listed.sort_unstable();
            let mut declared: Vec<String> =
                schema.element_names.keys().filter(|e| e.uri.as_deref() == Some(namespace.uri)).map(|e| e.local.to_string()).collect();
            declared.sort_unstable();
            assert_eq!(declared, listed, "{}", namespace.uri);
            assert_eq!(schema.namespace_non_deterministic(Some(namespace.uri)), !namespace.deterministic, "{}", namespace.uri);
        }
    }
}
