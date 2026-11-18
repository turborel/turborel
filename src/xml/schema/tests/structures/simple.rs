//! Tests of simple type definitions read from schema documents [Schema1
//! §3.14] [Schema2 §4.1.2]: the examples of the texts, facets, {final},
//! circular definitions, NOTATION, and simple content restricted by
//! facets.

use super::super::super::builtin::string_valid;
use super::*;
use crate::xml::Atomic;

/// The simple type definition of `schema` named `name`.
fn simple<'s>(schema: &'s Schema, name: &str) -> &'s SimpleTypeDef {
    let Some(TypeDef::Simple(Simple::Defined(id))) = schema.type_names.get(&Expanded::new(None, name)) else { panic!("no type {name}") };
    schema.simple(*id).unwrap()
}

/// `literal` read against simple type `s` of `schema` by String Valid
/// [Schema1 §3.14.4]: its atomic values, none if it is not valid.
fn read_as(schema: &Schema, s: Simple, literal: &str) -> Option<Vec<Atomic>> {
    string_valid(schema, s, literal, &[]).unwrap().map(|t| t.atoms)
}

/// Whether `literal` is valid for the type of `schema` named `name`.
fn valid_for(schema: &Schema, name: &str, literal: &str) -> bool {
    let Some(TypeDef::Simple(s)) = schema.type_names.get(&Expanded::new(None, name)).copied() else { panic!("no type {name}") };
    read_as(schema, s, literal).is_some()
}

/// [Schema1 §3.14] The example of a simple type definition. Its base is
/// written there `xs:number`, which XML Schema Part 2 has not; here the
/// datatype of its facets, `xs:decimal` [Schema2 §4.1.5].
const WATER: &str = r#"<xs:simpleType name="farenheitWaterTemp">
 <xs:restriction base="xs:decimal">
  <xs:fractionDigits value="2"/>
  <xs:minExclusive value="0.00"/>
  <xs:maxExclusive value="100.00"/>
 </xs:restriction>
</xs:simpleType>"#;

/// Verifies: [EXTENSION] [Schema1 §3.14.2, §3.14.4] [Schema2 §4.1.2.1,
/// §4.3.8.3, §4.3.9.3, §4.3.12.3] the example of [Schema1 §3.14] reads
/// into an atomic restriction of `xs:decimal`, and String Valid applies
/// its facets: a value at a bound it excludes, or of three fraction
/// digits, is not valid.
#[test]
fn the_farenheit_water_temp_example_reads() {
    let s = schema(WATER).unwrap();
    let t = simple(&s, "farenheitWaterTemp");
    assert_eq!((t.base, t.def.atomic(), t.final_), (Simple::Atomic(AtomicType::Decimal), Some(AtomicType::Decimal), Derivations::NONE));
    for ok in ["0.01", "50.5", " 99.99 "] {
        assert!(valid_for(&s, "farenheitWaterTemp", ok), "{ok}");
    }
    for bad in ["0", "100.00", "12.345", "warm"] {
        assert!(!valid_for(&s, "farenheitWaterTemp", bad), "{bad}");
    }
}

/// [Schema2 §4.1.2.3] The example of a union, the font sizes, its types
/// written with the prefix `xs`.
const SIZE: &str = r#"<xs:attribute name="size">
  <xs:simpleType>
    <xs:union>
      <xs:simpleType>
        <xs:restriction base="xs:positiveInteger">
          <xs:minInclusive value="8"/>
          <xs:maxInclusive value="72"/>
        </xs:restriction>
      </xs:simpleType>
      <xs:simpleType>
        <xs:restriction base="xs:NMTOKEN">
          <xs:enumeration value="small"/>
          <xs:enumeration value="medium"/>
          <xs:enumeration value="large"/>
        </xs:restriction>
      </xs:simpleType>
    </xs:union>
  </xs:simpleType>
</xs:attribute>"#;

/// Verifies: [EXTENSION] [Schema2 §4.1.2.1, §4.1.2.2, §4.1.2.3] [Schema1
/// §3.14.2] the examples of derivation by restriction (Sku), by list
/// (listOfFloat) and by union (the font sizes) read: a list's and a
/// union's base is the simple ur-type, a union validates by the first
/// member that validates a literal, its value of that member's type.
#[test]
fn the_examples_of_restriction_list_and_union_read() {
    let body = format!(
        r"<xs:simpleType name='Sku'><xs:restriction base='xs:string'><xs:pattern value='\d{{3}}-[A-Z]{{2}}'/></xs:restriction></xs:simpleType>
        <xs:simpleType name='listOfFloat'><xs:list itemType='xs:float'/></xs:simpleType>{SIZE}"
    );
    let s = schema(&body).unwrap();
    assert!(valid_for(&s, "Sku", "123-AB") && !valid_for(&s, "Sku", "123-ab"));
    assert_eq!(simple(&s, "listOfFloat").base, Simple::AnySimple);
    let floats = s.type_names[&Expanded::new(None, "listOfFloat")];
    let TypeDef::Simple(floats) = floats else { panic!() };
    let items = read_as(&s, floats, " 1.5  2 3e2 ").unwrap();
    assert!(items.len() == 3 && items.iter().all(|a| a.kind == AtomicType::Float));
    assert!(!valid_for(&s, "listOfFloat", "1.5 two"));
    let size = s.attribute(s.attribute_names[&Expanded::new(None, "size")]).unwrap().type_def;
    let Simple::Defined(union) = size else { panic!("{size:?}") };
    let def = s.simple(union).unwrap();
    assert_eq!((def.base, def.members.len(), def.name.is_none()), (Simple::AnySimple, 2, true));
    assert_eq!(read_as(&s, size, "12").unwrap()[0].kind, AtomicType::PositiveInteger);
    assert_eq!(read_as(&s, size, " large ").unwrap()[0].kind, AtomicType::NmToken);
    for bad in ["7", "huge", "73"] {
        assert!(read_as(&s, size, bad).is_none(), "{bad}");
    }
}

/// Verifies: [EXTENSION] [Schema2 §4.1.3, §4.1.5, §4.3.7.4, §4.3, Appendix
/// A] [Schema1 §3.14.1, §3.14.3 src-simple-type.2, .3] facets and
/// derivations a schema document gets wrong are refused: a facet that
/// applies not, a count no count, a bound past its base's, a fixed facet
/// changed, a facet given twice, a base and a `<simpleType>` both, a list
/// of a list, a union of nothing, the simple ur-type or a complex type as
/// a base.
#[test]
fn facets_and_derivations_are_checked() {
    let restricting = |base: &str, facets: &str| {
        format!(
            r#"<xs:simpleType name="b"><xs:restriction base="xs:decimal"><xs:maxInclusive value="10" fixed="true"/><xs:minInclusive value="0"/></xs:restriction></xs:simpleType>
        <xs:simpleType name="t"><xs:restriction base="{base}">{facets}</xs:restriction></xs:simpleType>"#
        )
    };
    let cases = [
        (restricting("xs:decimal", r#"<xs:maxLength value="3"/>"#), "§4.1.5"),
        (restricting("xs:string", r#"<xs:length value="x"/>"#), "Appendix A"),
        (restricting("xs:string", r#"<xs:whiteSpace value="tight"/>"#), "Appendix A"),
        (restricting("b", r#"<xs:minInclusive value="-1"/>"#), "§4.3.10.4"),
        (restricting("b", r#"<xs:maxInclusive value="5"/>"#), "fixed by the base type"),
        (restricting("xs:string", r#"<xs:length value="1"/><xs:length value="2"/>"#), "§4.1.3"),
        (restricting("xs:anySimpleType", ""), "§3.14.1"),
        (r#"<xs:complexType name="c"/><xs:simpleType name="t"><xs:restriction base="c"/></xs:simpleType>"#.to_owned(), "no simple type"),
        (
            r#"<xs:simpleType name="t"><xs:restriction base="xs:int"><xs:simpleType><xs:restriction base="xs:int"/></xs:simpleType></xs:restriction></xs:simpleType>"#.to_owned(),
            "src-simple-type.2",
        ),
        (
            r#"<xs:simpleType name="t"><xs:list itemType="xs:int"><xs:simpleType><xs:restriction base="xs:int"/></xs:simpleType></xs:list></xs:simpleType>"#.to_owned(),
            "src-simple-type.3",
        ),
        (r#"<xs:simpleType name="t"><xs:list itemType="xs:NMTOKENS"/></xs:simpleType>"#.to_owned(), "list of atomic"),
        (r#"<xs:simpleType name="t"><xs:union/></xs:simpleType>"#.to_owned(), "§4.1.3"),
        (r#"<xs:simpleType name="t"><xs:restriction base="xs:int"/><xs:list itemType="xs:int"/></xs:simpleType>"#.to_owned(), "Appendix A"),
        (r#"<xs:element name="e"><xs:simpleType name="t"><xs:restriction base="xs:int"/></xs:simpleType></xs:element>"#.to_owned(), "no attribute name"),
    ];
    for (body, rule) in cases {
        assert!(refused(&body).contains(rule), "{body}");
    }
    let s = schema(&restricting("b", r#"<xs:maxInclusive value="10"/><xs:minExclusive value="2"/>"#)).unwrap();
    assert!(valid_for(&s, "t", "10") && !valid_for(&s, "t", "2"));
}

/// The schema of `body` in a schema document whose `<schema>` has
/// finalDefault `default`.
fn with_final_default(default: &str, body: &str) -> Result<Schema> {
    read(&source(&format!(r#"<xs:schema {XS} finalDefault="{default}">{body}</xs:schema>"#), None), &Library::default())
}

/// Verifies: [EXTENSION] [Schema1 §3.14.2 {final}, §3.14.1, §3.14.6
/// st-props-correct.3] [Schema1 §3.4.6 cos-ct-extends.2.2] a simple type's
/// {final}, from its final or its document's finalDefault, `#all` being
/// extension, restriction, list and union, prevents the derivations it
/// names, and no other.
#[test]
fn final_prevents_derivation() {
    let base = |final_: &str| format!(r#"<xs:simpleType name="s" final="{final_}"><xs:restriction base="xs:int"/></xs:simpleType>"#);
    let restriction = r#"<xs:simpleType name="r"><xs:restriction base="s"><xs:maxInclusive value="3"/></xs:restriction></xs:simpleType>"#;
    let list = r#"<xs:simpleType name="l"><xs:list itemType="s"/></xs:simpleType>"#;
    let union = r#"<xs:simpleType name="u"><xs:union memberTypes="s xs:date"/></xs:simpleType>"#;
    let extension = r#"<xs:complexType name="c"><xs:simpleContent><xs:extension base="s"/></xs:simpleContent></xs:complexType>"#;
    assert!(refused(&(base("restriction") + restriction)).contains("st-props-correct.3"));
    assert!(refused(&(base("list") + list)).contains("§3.14.1"));
    assert!(refused(&(base("union") + union)).contains("§3.14.1"));
    assert!(refused(&(base("#all") + extension)).contains("cos-ct-extends.2.2"));
    assert!(refused(&base("extension")).contains("Appendix A"));
    schema(&(base("list union") + restriction + extension)).unwrap();
    schema(&(base("restriction") + list + union)).unwrap();
    let plain = r#"<xs:simpleType name="s"><xs:restriction base="xs:int"/></xs:simpleType>"#;
    assert!(with_final_default("restriction", &format!("{plain}{restriction}")).unwrap_err().to_string().contains("st-props-correct.3"));
    assert!(with_final_default("#all", &format!("{plain}{list}")).unwrap_err().to_string().contains("§3.14.1"));
    with_final_default("extension", &format!("{plain}{list}{union}{restriction}")).unwrap();
}

/// Verifies: [EXTENSION] [Schema1 §3.14.6 st-props-correct.2] [Schema1
/// §3.14.3 src-simple-type.4] circular simple type definitions are
/// refused: two restricting each other, a union of itself, a list of a
/// restriction of itself.
#[test]
fn circular_simple_types_are_refused() {
    let cases = [
        r#"<xs:simpleType name="a"><xs:restriction base="b"/></xs:simpleType><xs:simpleType name="b"><xs:restriction base="a"/></xs:simpleType>"#,
        r#"<xs:simpleType name="u"><xs:union memberTypes="xs:int u"/></xs:simpleType>"#,
        r#"<xs:simpleType name="l"><xs:list itemType="m"/></xs:simpleType><xs:simpleType name="m"><xs:restriction base="l"/></xs:simpleType>"#,
    ];
    for body in cases {
        assert!(refused(body).contains("st-props-correct.2"), "{body}");
    }
}

/// Verifies: [EXTENSION] [Schema2 §3.2.19] [Schema1 §3.15.3] NOTATION is
/// used through a type restricting it by enumeration, whose values are
/// QNames whose prefixes are in scope; a restriction of it without an
/// enumeration, and the built-in named as a type, are refused.
#[test]
fn notation_is_used_through_an_enumeration() {
    let s = schema(
        r#"<xs:notation name="jpeg" public="image/jpeg"/>
        <xs:simpleType name="formats"><xs:restriction base="xs:NOTATION"><xs:enumeration value="jpeg"/><xs:enumeration value="p:png"/></xs:restriction></xs:simpleType>
        <xs:attribute name="f" type="formats"/>"#,
    )
    .unwrap();
    assert!(valid_for(&s, "formats", "jpeg") && !valid_for(&s, "formats", "gif"));
    let Some(TypeDef::Simple(formats)) = s.type_names.get(&Expanded::new(None, "formats")).copied() else { panic!() };
    assert!(string_valid(&s, formats, "p:png", &[(Some("p"), "urn:p")]).unwrap().is_some());
    assert!(string_valid(&s, formats, "p:png", &[]).unwrap().is_none());
    let bare = r#"<xs:simpleType name="n"><xs:restriction base="xs:NOTATION"/></xs:simpleType>"#;
    assert!(refused(bare).contains("§3.2.19"));
    assert!(refused(r#"<xs:attribute name="a" type="xs:NOTATION"/>"#).contains("§3.2.19"));
}

/// [Schema1 §3.4.2] A price: a decimal of an attribute, and complex
/// types restricting its simple content.
const PRICE: &str = r#"<xs:complexType name="price"><xs:simpleContent><xs:extension base="xs:decimal">
    <xs:attribute name="currency" type="xs:token"/></xs:extension></xs:simpleContent></xs:complexType>
    <xs:complexType name="cheap"><xs:simpleContent><xs:restriction base="price"><xs:maxInclusive value="10"/></xs:restriction></xs:simpleContent></xs:complexType>
    <xs:complexType name="whole"><xs:simpleContent><xs:restriction base="price">
    <xs:simpleType><xs:restriction base="xs:integer"/></xs:simpleType><xs:minInclusive value="0"/></xs:restriction></xs:simpleContent></xs:complexType>"#;

/// Verifies: [EXTENSION] [Schema1 §3.4.2 {content type} clause 1.1, 1.2]
/// [Schema1 §3.4.6 derivation-ok-restriction.5.1.1] a restriction of
/// simple content restricts its base's content type, or its own
/// `<simpleType>`, by its facets; a `<simpleType>` that is no restriction
/// of the base's content, or a facet that applies not, is refused.
#[test]
fn simple_content_restricts_by_facets() {
    let s = schema(PRICE).unwrap();
    let ContentType::Simple(cheap) = complex(&s, "cheap").content else { panic!() };
    let Simple::Defined(id) = cheap else { panic!("{cheap:?}") };
    assert_eq!(s.simple(id).unwrap().base, Simple::Atomic(AtomicType::Decimal));
    assert!(read_as(&s, cheap, "9.5").is_some() && read_as(&s, cheap, "11").is_none());
    let ContentType::Simple(whole) = complex(&s, "whole").content else { panic!() };
    let Simple::Defined(restricted) = whole else { panic!("{whole:?}") };
    let Simple::Defined(inner) = s.simple(restricted).unwrap().base else { panic!() };
    assert_eq!(s.simple(inner).unwrap().base, Simple::Atomic(AtomicType::Integer));
    assert!(read_as(&s, whole, "3").is_some() && read_as(&s, whole, "-3").is_none() && read_as(&s, whole, "2.5").is_none());
    let unrelated = PRICE.replace(
        r#"<xs:restriction base="xs:integer"/></xs:simpleType><xs:minInclusive value="0"/>"#,
        r#"<xs:restriction base="xs:string"/></xs:simpleType>"#,
    );
    assert!(refused(&unrelated).contains("derivation-ok-restriction.5"));
    let inapplicable = PRICE.replace(r#"<xs:maxInclusive value="10"/>"#, r#"<xs:maxLength value="10"/>"#);
    assert!(refused(&inapplicable).contains("§4.1.5"));
}

/// Verifies: [EXTENSION] [Schema1 §3.14.6 Type Derivation OK (Simple)
/// 2.2.4] [Schema1 §3.3.6 e-props-correct.3] a type derived from a member
/// of a union is validly derived from the union, so an element of it may
/// be in the substitution group of an element of the union; one derived
/// from no member is not.
#[test]
fn types_derive_from_the_unions_they_are_members_of() {
    let body = |member: &str| {
        format!(
            r#"<xs:simpleType name="u"><xs:union memberTypes="xs:int xs:date"/></xs:simpleType>
            <xs:element name="h" type="u"/><xs:element name="m" type="{member}" substitutionGroup="h"/>"#
        )
    };
    let s = schema(&body("xs:short")).unwrap();
    let u = s.type_names[&Expanded::new(None, "u")];
    let short = TypeDef::Simple(Simple::Atomic(AtomicType::Short));
    assert!(type_derives(&s, short, u, Derivations::NONE) && !type_derives(&s, short, u, Derivations::RESTRICTION));
    assert!(refused(&body("xs:string")).contains("e-props-correct.3"));
}

/// Verifies: [EXTENSION] [Schema1 §3.2.6 a-props-correct.2, .3] [Schema1
/// §3.3.6 e-props-correct.2, .4] value constraints are valid for the
/// defined types they are of: a default past a bound, and a value
/// constraint of a type restricting `xs:ID`, are refused.
#[test]
fn value_constraints_of_defined_types_are_checked() {
    let small = r#"<xs:simpleType name="small"><xs:restriction base="xs:int"><xs:maxInclusive value="5"/></xs:restriction></xs:simpleType>
        <xs:simpleType name="key"><xs:restriction base="xs:ID"><xs:pattern value="k\d+"/></xs:restriction></xs:simpleType>"#;
    let cases = [
        (r#"<xs:attribute name="a" type="small" default="6"/>"#, "a-props-correct.2"),
        (r#"<xs:attribute name="a" type="key" fixed="k1"/>"#, "a-props-correct.3"),
        (r#"<xs:element name="e" type="small" default="6"/>"#, "e-props-correct.2"),
        (r#"<xs:element name="e" type="key" default="k1"/>"#, "e-props-correct.4"),
    ];
    for (body, rule) in cases {
        assert!(refused(&format!("{small}{body}")).contains(rule), "{body}");
    }
    let s = schema(&format!(r#"{small}<xs:element name="e" type="small" default="5"/>"#)).unwrap();
    let e = s.element(s.element_names[&Expanded::new(None, "e")]).unwrap();
    assert_eq!(e.value.as_ref().map(|v| v.value.canonical()), Some("5".to_owned()));
}

/// `n` simple types, each restricting the next, the last `xs:int`: `n`
/// and one read within one another.
fn chain(n: usize) -> String {
    let mut parts: Vec<String> = (0..n)
        .zip(1_usize..)
        .map(|(i, next)| format!(r#"<xs:simpleType name="t{i}"><xs:restriction base="t{next}"/></xs:simpleType>"#))
        .collect();
    parts.push(format!(r#"<xs:simpleType name="t{n}"><xs:restriction base="xs:int"/></xs:simpleType>"#));
    parts.concat()
}

/// Verifies: [DESIGN] RULES.md D18, R-CF-6: simple type definitions read
/// within one another, each the base of the one before, are read
/// `MAX_CHAIN` deep, within half of a thread's default stack; one more is
/// SQLCODE -900, never a stack overflow.
#[test]
fn chains_of_simple_types_are_bounded() {
    const HALF_DEFAULT_STACK: usize = 1024 * 1024;
    let deepest = std::thread::Builder::new()
        .name("schema-chain".to_owned())
        .stack_size(HALF_DEFAULT_STACK)
        .spawn(|| {
            let s = schema(&chain(MAX_CHAIN.saturating_sub(1))).unwrap();
            valid_for(&s, "t0", "7") && !valid_for(&s, "t0", "seven")
        })
        .unwrap();
    assert!(deepest.join().unwrap());
    assert_eq!(schema(&chain(MAX_CHAIN)).unwrap_err().sqlcode(), -900_i32);
}
