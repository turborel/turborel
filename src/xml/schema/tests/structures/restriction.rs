//! Tests of Particle Valid (Restriction) [Schema1 §3.9.6]: each case of
//! its table, met and broken, as a complex type restricting another's
//! content model.

use super::*;

/// The schema of type `b` of content `base` and type `r` restricting it
/// with content `restricted`, both of `mixed`.
fn pair(base: &str, restricted: &str, mixed: bool) -> Result<Schema> {
    schema(&format!(
        r#"<xs:element name="head" type="xs:int"/><xs:element name="m1" type="xs:int" substitutionGroup="head"/>
        <xs:element name="m2" type="xs:int" substitutionGroup="head"/><xs:element name="g" type="xs:int"/>
        <xs:complexType name="b" mixed="{mixed}">{base}</xs:complexType>
        <xs:complexType name="r" mixed="{mixed}"><xs:complexContent><xs:restriction base="b">{restricted}</xs:restriction></xs:complexContent></xs:complexType>"#
    ))
}

/// Each `(base, restricted)` of `cases` reads.
fn allowed(cases: &[(&str, &str)]) {
    for (base, restricted) in cases {
        pair(base, restricted, false).unwrap_or_else(|e| panic!("{restricted} of {base}: {e}"));
    }
}

/// Each `(base, restricted, case)` of `cases` is refused, SQLSTATE 22000,
/// by derivation-ok-restriction.5.3 for the case named.
fn broken(cases: &[(&str, &str, &str)]) {
    for (base, restricted, case) in cases {
        let e = pair(base, restricted, false).unwrap_err();
        assert_eq!(e.sqlstate(), SqlState::DATA_EXCEPTION, "{e}");
        let message = e.to_string();
        assert!(message.contains("derivation-ok-restriction.5.3") && message.contains(case), "{case}: {message}");
    }
}

/// A sequence of `body`.
fn seq(body: &str) -> String {
    format!("<xs:sequence>{body}</xs:sequence>")
}

/// Verifies: [EXTENSION] [Schema1 §3.9.6 rcase-NameAndTypeOK] an element
/// restricts an element of its name whose occurrence range holds its own,
/// no more nillable, keeping a fixed value, its identity constraints
/// among the base's, blocking at least what the base blocks, and of a
/// type derived by restriction; each clause broken is named.
#[test]
fn name_and_type_ok_clauses_are_checked() {
    allowed(&[
        (&seq(r#"<xs:element ref="g" minOccurs="0"/>"#), &seq(r#"<xs:element ref="g"/>"#)),
        (&seq(r#"<xs:element ref="g"/>"#), &seq(r#"<xs:element ref="g"/>"#)),
        (&seq(r#"<xs:element name="a" type="xs:int" fixed="1"/>"#), &seq(r#"<xs:element name="a" type="xs:short" fixed="1"/>"#)),
        (&seq(r#"<xs:element name="a" nillable="true"/>"#), &seq(r#"<xs:element name="a" block="extension"/>"#)),
    ]);
    let key = r#"<xs:unique name="u"><xs:selector xpath="x"/><xs:field xpath="@y"/></xs:unique>"#;
    broken(&[
        (&seq(r#"<xs:element name="a"/>"#), &seq(r#"<xs:element name="b"/>"#), "rcase-NameAndTypeOK.1"),
        (&seq(r#"<xs:element name="a"/>"#), &seq(r#"<xs:element name="a" nillable="true"/>"#), "rcase-NameAndTypeOK.2"),
        (&seq(r#"<xs:element name="a"/>"#), &seq(r#"<xs:element name="a" maxOccurs="2"/>"#), "rcase-NameAndTypeOK.3"),
        (
            &seq(r#"<xs:element name="a" type="xs:int" fixed="1"/>"#),
            &seq(r#"<xs:element name="a" type="xs:int"/>"#),
            "rcase-NameAndTypeOK.4",
        ),
        (
            &seq(r#"<xs:element name="a" type="xs:int" fixed="1"/>"#),
            &seq(r#"<xs:element name="a" type="xs:int" fixed="2"/>"#),
            "rcase-NameAndTypeOK.4",
        ),
        (&seq(r#"<xs:element name="a"/>"#), &seq(&format!(r#"<xs:element name="a">{key}</xs:element>"#)), "rcase-NameAndTypeOK.5"),
        (&seq(r#"<xs:element name="a" block="extension"/>"#), &seq(r#"<xs:element name="a"/>"#), "rcase-NameAndTypeOK.6"),
        (&seq(r#"<xs:element name="a" type="xs:int"/>"#), &seq(r#"<xs:element name="a" type="xs:string"/>"#), "rcase-NameAndTypeOK.7"),
    ]);
}

/// Verifies: [EXTENSION] [Schema1 §3.9.6 rcase-NSCompat, rcase-NSSubset,
/// rcase-NSRecurseCheckCardinality] an element restricts a wildcard
/// allowing its namespace, a wildcard one allowing a superset of its
/// namespaces, and a group a wildcard whose range holds the group's
/// total range; each broken is named.
#[test]
fn wildcard_bases_are_checked() {
    let any = |ns: &str, max: &str| seq(&format!(r#"<xs:any namespace="{ns}" maxOccurs="{max}" processContents="lax"/>"#));
    allowed(&[
        (&any("##any", "1"), &seq(r#"<xs:element name="a"/>"#)),
        (&any("##any", "1"), &any("##local", "1")),
        (&any("##any", "unbounded"), &seq(r#"<xs:element name="a"/><xs:element name="b"/>"#)),
    ]);
    broken(&[
        (&any("##other", "1"), &seq(r#"<xs:element name="a"/>"#), "rcase-NSCompat"),
        (&any("##local", "1"), &any("##any", "1"), "NSSubset"),
        (&any("##any", "1"), &any("##any", "2"), "NSSubset"),
        (&any("##any", "1"), &seq(r#"<xs:element name="a"/><xs:element name="b"/>"#), "NSRecurseCheckCardinality"),
        (&seq(r#"<xs:element name="a"/>"#), &any("##any", "1"), "the table forbids"),
    ]);
}

/// Verifies: [EXTENSION] [Schema1 §3.9.6 rcase-Recurse] a sequence
/// restricts a sequence, and an all group an all group, by an
/// order-preserving mapping leaving only emptiable particles of the base
/// unmapped, within its range; each clause broken is named.
#[test]
fn recurse_is_checked() {
    let abc = seq(r#"<xs:element name="a"/><xs:element name="b" minOccurs="0"/><xs:element name="c"/>"#);
    allowed(&[
        (&abc, &seq(r#"<xs:element name="a"/><xs:element name="c"/>"#)),
        (
            r#"<xs:all><xs:element name="a"/><xs:element name="b" minOccurs="0"/></xs:all>"#,
            r#"<xs:all><xs:element name="a"/><xs:element name="b"/></xs:all>"#,
        ),
    ]);
    broken(&[
        (&abc, r#"<xs:sequence maxOccurs="2"><xs:element name="a"/><xs:element name="c"/></xs:sequence>"#, "rcase-Recurse.1"),
        (&abc, &seq(r#"<xs:element name="a"/><xs:element name="d"/>"#), "rcase-Recurse.2"),
        (&abc, &seq(r#"<xs:element name="c"/><xs:element name="a"/>"#), "rcase-Recurse.2.2"),
        (&abc, &seq(r#"<xs:element name="a"/><xs:element name="b"/>"#), "rcase-Recurse.2.2"),
    ]);
}

/// Verifies: [EXTENSION] [Schema1 §3.9.6 rcase-RecurseLax] a choice
/// restricts a choice by an order-preserving mapping within its range;
/// a member of a substitution group restricts its head, a choice of the
/// group's members [§3.9.6 clause 2.1].
#[test]
fn recurse_lax_and_substitution_groups_are_checked() {
    let abc = r#"<xs:choice><xs:element name="a"/><xs:element name="b"/><xs:element name="c"/></xs:choice>"#;
    allowed(&[
        (abc, r#"<xs:choice><xs:element name="a"/><xs:element name="c"/></xs:choice>"#),
        (&seq(r#"<xs:element ref="head"/>"#), &seq(r#"<xs:element ref="m2"/>"#)),
    ]);
    broken(&[
        (abc, r#"<xs:choice><xs:element name="c"/><xs:element name="a"/></xs:choice>"#, "rcase-RecurseLax.2"),
        (abc, r#"<xs:choice maxOccurs="3"><xs:element name="a"/><xs:element name="c"/></xs:choice>"#, "rcase-RecurseLax.1"),
        (&seq(r#"<xs:element ref="head"/>"#), &seq(r#"<xs:element ref="g"/>"#), "rcase-RecurseLax.2"),
        (abc, &seq(r#"<xs:element name="a"/><xs:element name="b"/>"#), "rcase-MapAndSum.2"),
    ]);
}

/// Verifies: [EXTENSION] [Schema1 §3.9.6 rcase-RecurseUnordered,
/// rcase-MapAndSum] a sequence restricts an all group by a mapping in any
/// order leaving only emptiable particles unmapped, and a choice when
/// each of its particles restricts one of the choice's and its range
/// times its length is within the choice's; a choice restricting a
/// sequence is forbidden by the table.
#[test]
fn recurse_unordered_and_map_and_sum_are_checked() {
    let all = r#"<xs:all><xs:element name="a"/><xs:element name="b"/><xs:element name="c" minOccurs="0"/></xs:all>"#;
    let choice = r#"<xs:choice maxOccurs="unbounded"><xs:element name="a"/><xs:element name="b"/></xs:choice>"#;
    allowed(&[
        (all, &seq(r#"<xs:element name="b"/><xs:element name="a"/>"#)),
        (choice, &seq(r#"<xs:element name="b"/><xs:element name="a"/>"#)),
    ]);
    broken(&[
        (all, r#"<xs:sequence maxOccurs="2"><xs:element name="a"/><xs:element name="b"/></xs:sequence>"#, "rcase-RecurseUnordered.1"),
        (all, &seq(r#"<xs:element name="a"/><xs:element name="d"/>"#), "rcase-RecurseUnordered.2"),
        (all, &seq(r#"<xs:element name="a"/><xs:element name="c"/>"#), "rcase-RecurseUnordered.2.3"),
        (choice, &seq(r#"<xs:element name="a"/><xs:element name="d"/>"#), "rcase-MapAndSum.1"),
        (
            &seq(r#"<xs:element name="a"/><xs:element name="b"/>"#),
            r#"<xs:choice><xs:element name="a"/><xs:element name="b"/></xs:choice>"#,
            "the table forbids",
        ),
    ]);
}

/// Verifies: [EXTENSION] [Schema1 §3.9.6 cos-particle-restrict.2]
/// [Schema1 §3.4.6 derivation-ok-restriction.5] a content model of only
/// empty groups restricts a base whose content model is emptiable, and
/// is refused for one that is not; one whose base has only empty groups
/// may not add elements; an empty mixed restriction of a base that is
/// not emptiable is refused by clause 5.
#[test]
fn restricting_to_or_from_nothing_is_checked() {
    let empty = seq("<xs:sequence/>");
    pair(&seq(r#"<xs:element name="a" minOccurs="0"/>"#), &empty, false).unwrap();
    for (base, restricted, case) in [
        (seq(r#"<xs:element name="a"/>"#), empty.clone(), "the base is not emptiable"),
        (empty.clone(), seq(r#"<xs:element name="a"/>"#), "nothing is restricted to content"),
    ] {
        let message = pair(&base, &restricted, false).unwrap_err().to_string();
        assert!(message.contains(case), "{case}: {message}");
    }
    let message = pair(&seq(r#"<xs:element name="a"/>"#), "<xs:sequence/>", true).unwrap_err().to_string();
    assert!(message.contains("derivation-ok-restriction.5]"), "{message}");
}
