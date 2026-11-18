//! Tests of schemas read from several documents [Schema1 §4.2], and of
//! wildcards' namespace constraints [Schema1 §3.10].

use super::super::super::component::NamespaceConstraint;
use super::super::super::wildcard::namespace_constraint;
use super::*;

/// A schema document of target namespace `tns`, if any, of `body`.
fn document(tns: Option<&str>, body: &str) -> String {
    let target = tns.map(|t| format!(r#" targetNamespace="{t}" xmlns:t="{t}""#)).unwrap_or_default();
    format!("<xs:schema {XS}{target}>{body}</xs:schema>")
}

/// Verifies: [EXTENSION] [Schema1 §4.2.1 src-include.2.3, 3.2] a document
/// of no target namespace included in one of a namespace is read into
/// it: its components are of that namespace, and its references of no
/// namespace name them there.
#[test]
fn an_included_document_takes_its_includers_namespace() {
    let included = source(&document(None, r#"<xs:complexType name="c"/><xs:element name="e" type="c"/>"#), Some("lib.xsd"));
    let main = source(&document(Some("urn:m"), r#"<xs:include schemaLocation="lib.xsd"/><xs:element name="f" type="t:c"/>"#), None);
    let s = read(&main, &Library::new(vec![included])).unwrap();
    assert!(s.element_names.contains_key(&Expanded::new(Some("urn:m"), "e")));
    assert!(s.type_names.contains_key(&Expanded::new(Some("urn:m"), "c")));
}

/// Verifies: [EXTENSION] [Schema1 §4.2.1 src-include.2.1] a document of
/// another target namespace is not included.
#[test]
fn an_included_document_of_another_namespace_is_refused() {
    let included = source(&document(Some("urn:other"), r#"<xs:element name="e"/>"#), Some("lib.xsd"));
    let main = source(&document(Some("urn:m"), r#"<xs:include schemaLocation="lib.xsd"/>"#), None);
    let e = read(&main, &Library::new(vec![included])).unwrap_err();
    assert!(e.to_string().contains("src-include.2.1"), "{e}");
}

/// Verifies: [EXTENSION] [Schema1 §4.2.3, §3.15.3 src-resolve.4] an
/// imported namespace's components are named by their QNames; a
/// namespace not imported is refused; a schema location naming no
/// document adds nothing and is no error.
#[test]
fn imports_resolve_among_the_library() {
    let other = source(&document(Some("urn:o"), r#"<xs:complexType name="ot"/>"#), Some("o.xsd"));
    let main = |import: &str| {
        let body = format!(r#"{import}<xs:element name="e" xmlns:o="urn:o" type="o:ot"/>"#);
        source(&document(Some("urn:m"), &body), None)
    };
    let library = Library::new(vec![other]);
    let s = read(&main(r#"<xs:import namespace="urn:o" schemaLocation="nowhere.xsd"/>"#), &library).unwrap();
    assert!(s.type_names.contains_key(&Expanded::new(Some("urn:o"), "ot")));
    let e = read(&main(""), &library).unwrap_err();
    assert!(e.to_string().contains("src-resolve.4"), "{e}");
    let own = read(&main(r#"<xs:import namespace="urn:m"/>"#), &library).unwrap_err();
    assert!(own.to_string().contains("src-import.1.1"), "{own}");
}

/// Verifies: [EXTENSION] [Schema1 §4.2.2 src-redefine] the redefinition
/// example of [Schema1 §4.2.2]: v2 redefines personName of v1 by
/// extending it, so both v1's addressee and v2's author are of the
/// redefined type, whose base is v1's definition, now anonymous.
#[test]
fn the_redefinition_example_redefines_person_name() {
    let v1 = source(
        &document(
            None,
            r#"<xs:complexType name="personName"><xs:sequence>
            <xs:element name="title" minOccurs="0"/><xs:element name="forename" minOccurs="0" maxOccurs="unbounded"/>
            </xs:sequence></xs:complexType><xs:element name="addressee" type="personName"/>"#,
        ),
        Some("v1.xsd"),
    );
    let v2 = source(
        &document(
            None,
            r#"<xs:redefine schemaLocation="v1.xsd"><xs:complexType name="personName"><xs:complexContent>
            <xs:extension base="personName"><xs:sequence><xs:element name="generation" minOccurs="0"/></xs:sequence></xs:extension>
            </xs:complexContent></xs:complexType></xs:redefine><xs:element name="author" type="personName"/>"#,
        ),
        None,
    );
    let s = read(&v2, &Library::new(vec![v1])).unwrap();
    let redefined = s.type_names[&Expanded::new(None, "personName")];
    let type_of = |name: &str| s.element(s.element_names[&Expanded::new(None, name)]).unwrap().type_def;
    assert_eq!(type_of("addressee"), redefined);
    assert_eq!(type_of("author"), redefined);
    let TypeDef::Complex(id) = redefined else { panic!() };
    let def = s.complex(id).unwrap();
    assert_eq!(def.method, Method::Extension);
    let TypeDef::Complex(base) = def.base else { panic!() };
    assert!(s.complex(base).unwrap().name.is_none());
}

/// Verifies: [EXTENSION] [Schema1 §4.2.2 src-redefine.5, src-redefine]
/// a redefinition derives from what it redefines, and redefines a
/// definition the document has.
#[test]
fn redefinitions_are_checked() {
    let v1 = source(&document(None, r#"<xs:complexType name="a"/><xs:complexType name="b"/>"#), Some("v1.xsd"));
    let redefine = |body: &str| source(&document(None, &format!(r#"<xs:redefine schemaLocation="v1.xsd">{body}</xs:redefine>"#)), None);
    let library = Library::new(vec![v1]);
    let other_base =
        redefine(r#"<xs:complexType name="a"><xs:complexContent><xs:extension base="b"/></xs:complexContent></xs:complexType>"#);
    assert!(read(&other_base, &library).unwrap_err().to_string().contains("src-redefine.5"));
    let missing = redefine(r#"<xs:complexType name="c"><xs:complexContent><xs:extension base="c"/></xs:complexContent></xs:complexType>"#);
    assert!(read(&missing, &library).unwrap_err().to_string().contains("src-redefine"));
    let nowhere = source(&document(None, r#"<xs:redefine schemaLocation="none.xsd"><xs:complexType name="a"/></xs:redefine>"#), None);
    assert!(read(&nowhere, &library).unwrap_err().to_string().contains("src-redefine.1"));
}

/// Verifies: [EXTENSION] [Schema1 §4.2.2 src-redefine.6] a redefined
/// group refers to itself once, or restricts the group it redefines.
#[test]
fn redefined_groups_extend_or_restrict_themselves() {
    let v1 = source(
        &document(None, r#"<xs:group name="g"><xs:sequence><xs:element name="a" minOccurs="0"/></xs:sequence></xs:group>"#),
        Some("v1.xsd"),
    );
    let library = Library::new(vec![v1]);
    let redefine = |body: &str| source(&document(None, &format!(r#"<xs:redefine schemaLocation="v1.xsd">{body}</xs:redefine>"#)), None);
    read(&redefine(r#"<xs:group name="g"><xs:sequence><xs:group ref="g"/><xs:element name="b"/></xs:sequence></xs:group>"#), &library)
        .unwrap();
    read(&redefine(r#"<xs:group name="g"><xs:sequence><xs:element name="a"/></xs:sequence></xs:group>"#), &library).unwrap();
    let wider = redefine(r#"<xs:group name="g"><xs:sequence><xs:element name="c"/></xs:sequence></xs:group>"#);
    assert!(read(&wider, &library).unwrap_err().to_string().contains("src-redefine.6.2.2"));
    let twice = redefine(r#"<xs:group name="g"><xs:sequence><xs:group ref="g"/><xs:group ref="g"/></xs:sequence></xs:group>"#);
    assert!(read(&twice, &library).unwrap_err().to_string().contains("src-redefine.6.1.1"));
}

/// The set of namespaces `members`, none for absent.
fn set(members: &[Option<&str>]) -> NamespaceConstraint {
    let mut v: Vec<Option<Box<str>>> = members.iter().map(|m| m.map(Into::into)).collect();
    v.sort();
    NamespaceConstraint::Set(v)
}

/// Verifies: [EXTENSION] [Schema1 §3.10.2, §3.10.4, §3.10.6] namespace
/// constraints: as written, the namespaces they allow, and their
/// intensional subset [Schema1 §3.10.6 cos-ns-subset].
#[test]
fn namespace_constraints_combine_as_the_text_says() {
    let any = NamespaceConstraint::Any;
    let not_a = NamespaceConstraint::Not(Some("a".into()));
    let not_b = NamespaceConstraint::Not(Some("b".into()));
    assert_eq!(namespace_constraint(None, Some("t")), Some(any.clone()));
    assert_eq!(namespace_constraint(Some("##other"), Some("a")), Some(not_a.clone()));
    assert_eq!(namespace_constraint(Some("##targetNamespace ##local b"), Some("a")), Some(set(&[None, Some("a"), Some("b")])));
    assert_eq!(namespace_constraint(Some("##any b"), None), None);
    assert!(not_a.allows(Some("b")) && !not_a.allows(Some("a")) && !not_a.allows(None));
    assert!(set(&[None]).allows(None) && !set(&[None]).allows(Some("a")));
    assert!(set(&[Some("b")]).subset_of(&not_a) && !set(&[Some("a")]).subset_of(&not_a) && not_a.subset_of(&any));
    assert!(!not_a.subset_of(&not_b) && !any.subset_of(&not_a));
}

/// Verifies: [EXTENSION] [Schema1 §3.10.6 cos-aw-union, cos-aw-intersect]
/// the intensional union and intersection of namespace constraints, each
/// case of the text, those of two negations of different names not
/// expressible.
#[test]
fn namespace_constraints_unite_and_intersect() {
    let any = NamespaceConstraint::Any;
    let not_a = NamespaceConstraint::Not(Some("a".into()));
    let not_b = NamespaceConstraint::Not(Some("b".into()));
    assert_eq!(not_a.union(&set(&[Some("a")])), Some(any.clone()));
    assert_eq!(not_a.union(&set(&[Some("b")])), Some(not_a.clone()));
    assert_eq!(not_a.union(&not_b), None);
    assert_eq!(set(&[Some("a")]).union(&set(&[Some("b")])), Some(set(&[Some("a"), Some("b")])));
    assert_eq!(not_a.intersection(&set(&[Some("a"), Some("b")])), Some(set(&[Some("b")])));
    assert_eq!(any.intersection(&not_a), Some(not_a.clone()));
    assert_eq!(not_a.intersection(&not_b), None);
    assert!(not_a.overlaps(&not_b) && !set(&[Some("a")]).overlaps(&set(&[Some("b")])));
}
