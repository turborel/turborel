//! Tests of the document type declaration: entities, attribute lists,
//! element types, notations, parameter entities and conditional
//! sections.

use super::super::tests::{bad, content, doc};
use super::super::{Syntax, Whitespace};

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR7c] [XML §4.4.2, §4.6]
/// an internal entity's reference is its replacement text, read as
/// content, and a predefined entity's is its character, as data.
#[test]
fn internal_entities_are_expanded() {
    let text = "<!DOCTYPE a [<!ENTITY e 'x<b>&f;</b>y'><!ENTITY f '&lt;&#38;#38;'>]><a>&e;&amp;</a>";
    assert_eq!(doc(text), "<a>x<b><&</>y&</>");
    assert_eq!(doc("<!DOCTYPE a [<!ENTITY e 'a&#38;#60;b'>]><a>&e;</a>"), "<a>a<b</>");
    assert_eq!(doc("<!DOCTYPE a [<!ENTITY e \"'\">]><a x='&e;&quot;'/>"), "<a x=\"'\"\"></>");
    assert_eq!(doc("<!DOCTYPE a [<!ENTITY e 'one\ntwo'>]><a x='&e;'>&e;</a>"), "<a x=\"one two\">one\ntwo</>");
    assert_eq!(doc("<!DOCTYPE a [<!ENTITY e '1'><!ENTITY e '2'>]><a>&e;</a>"), "<a>1</>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR6a] [XML §4.1 WFC:
/// Entity Declared, WFC: No Recursion, WFC: Parsed Entity] [XML §3.1
/// WFC: No External Entity References, WFC: No < in Attribute Values]
/// [XML §4.3.2] references break no constraint, and an entity's
/// elements begin and end in it.
#[test]
fn entity_constraints() {
    assert!(bad("<a>&e;</a>", Syntax::Document).contains("WFC: Entity Declared"));
    assert!(bad("<!DOCTYPE a [<!ENTITY e '&e;'>]><a>&e;</a>", Syntax::Document).contains("WFC: No Recursion"));
    assert!(bad("<!DOCTYPE a [<!ENTITY e '&f;'><!ENTITY f '&e;'>]><a x='&e;'/>", Syntax::Document).contains("WFC: No Recursion"));
    let unparsed = "<!DOCTYPE a [<!NOTATION n SYSTEM 'n'><!ENTITY e SYSTEM 'x' NDATA n>]><a>&e;</a>";
    assert!(bad(unparsed, Syntax::Document).contains("WFC: Parsed Entity"));
    assert!(bad("<!DOCTYPE a [<!ENTITY e SYSTEM 'x'>]><a x='&e;'/>", Syntax::Document).contains("WFC: No External Entity References"));
    assert!(bad("<!DOCTYPE a [<!ENTITY e '<'>]><a x='&e;'/>", Syntax::Document).contains("WFC: No < in Attribute Values"));
    assert!(bad("<!DOCTYPE a [<!ENTITY e '<b>'>]><a>&e;</b></a>", Syntax::Document).contains("[XML §4.3.2]"));
    assert!(bad("<!DOCTYPE a [<!ENTITY e '</a>'>]><a>&e;", Syntax::Document).contains("[XML §4.3.2]"));
    assert!(bad("<!DOCTYPE a [<!ENTITY e '<!--x'>]><a>&e;--></a>", Syntax::Document).contains("in the entity it begins in"));
    assert!(bad("<!DOCTYPE a [<!ENTITY e 'x'>]><a>&e</a>", Syntax::Document).contains("[XML §4.1 [68]]"));
    assert!(bad("<!DOCTYPE a [<!ENTITY a:b 'x'>]><a/>", Syntax::Document).contains("has a colon"));
    assert!(bad("<!DOCTYPE a [<!ENTITY e 'x<b>'>]><a>&e;</a>", Syntax::Document).contains("in the entity 'e'"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR6a, GR7e] [XML §4.4.3,
/// §5.1] [XQueryDM §3.2] an external entity is not read, so its
/// reference, and one to an entity a declaration not read might declare,
/// cannot be expanded, which the data model requires.
#[test]
fn what_is_not_read_cannot_be_expanded() {
    assert!(bad("<!DOCTYPE a [<!ENTITY e SYSTEM 'e.xml'>]><a>&e;</a>", Syntax::Document).contains("[XQueryDM §3.2]"));
    let external = bad("<!DOCTYPE a SYSTEM 'a.dtd'><a>&e;</a>", Syntax::Document);
    assert!(external.contains("not declared in what turborel reads"), "{external}");
    assert_eq!(doc("<!DOCTYPE a PUBLIC '-//x//y' 'a.dtd' [<!ENTITY e 'v'>]><a>&e;</a>"), "<a>v</>");
    let skipped = "<!DOCTYPE a [<!ENTITY % p SYSTEM 'p'> %p; <!ENTITY e 'v'><!ATTLIST a x CDATA 'd'>]><a>&e;</a>";
    assert!(bad(skipped, Syntax::Document).contains("not declared in what turborel reads"));
    let processed = "<?xml version='1.0' standalone='yes'?><!DOCTYPE a [<!ENTITY % p SYSTEM 'p'> %p; <!ENTITY e 'v'><!ATTLIST a x CDATA 'd'>]><a>&e;</a>";
    assert_eq!(doc(processed), "<a x=\"d\">v</>");
    let in_parameter = "<?xml version='1.0' standalone='yes'?><!DOCTYPE a [<!ENTITY % p '<!ENTITY e \"v\">'> %p;]><a>&e;</a>";
    assert!(bad(in_parameter, Syntax::Document).contains("declared in a parameter entity"));
    assert_eq!(doc("<!DOCTYPE a [<!ENTITY % p '<!ENTITY e \"v\">'> %p;]><a>&e;</a>"), "<a>v</>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR7d] [XML §3.3.2,
/// §3.3.3] [XQueryDM §6.2.3, §6.3.3] declared defaults are added to the
/// elements that do not specify them; the first declaration of an
/// attribute binds; a value of a type but CDATA is a list of tokens; an
/// ID, IDREF or IDREFS says so, as `xml:id` does.
#[test]
fn attribute_lists() {
    let dtd = "<!DOCTYPE a [<!ATTLIST a d CDATA ' v ' f CDATA #FIXED 'w' i ID #IMPLIED r IDREFS #IMPLIED n NMTOKENS 'p  q' o (x|y) #REQUIRED><!ATTLIST a d CDATA 'other' t NOTATION (m) #IMPLIED>]>";
    assert_eq!(doc(&format!("{dtd}<a i=' k ' r=' k  l '/>")), "<a i=\"k\"#id r=\"k l\"#refs d=\" v \" f=\"w\" n=\"p q\"></>");
    assert_eq!(doc(&format!("{dtd}<a d='mine' n='z'/>")), "<a d=\"mine\" n=\"z\" f=\"w\"></>");
    assert_eq!(doc("<a xml:id=' k  l '/>"), "<a xml:id{http://www.w3.org/XML/1998/namespace}=\"k l\"#id></>");
    assert_eq!(doc("<!DOCTYPE a [<!ATTLIST a xmlns CDATA 'u'>]><a/>"), "<a{u}></>");
    assert_eq!(doc("<!DOCTYPE a [<!ENTITY e 'v'><!ATTLIST a x CDATA '&e;'>]><a/>"), "<a x=\"v\"></>");
    assert!(bad("<!DOCTYPE a [<!ATTLIST a x CDATA '&e;'><!ENTITY e 'v'>]><a/>", Syntax::Document).contains("WFC: Entity Declared"));
    assert!(bad("<!DOCTYPE a [<!ATTLIST a x BOGUS #IMPLIED>]><a/>", Syntax::Document).contains("attribute type"));
    assert!(bad("<!DOCTYPE a [<!ATTLIST a x (|y) #IMPLIED>]><a/>", Syntax::Document).contains("name token"));
    assert!(bad("<!DOCTYPE a [<!ATTLIST a x CDATA #FIXED>]><a/>", Syntax::Document).contains("white space"));
    assert!(bad("<!DOCTYPE a [<!ATTLIST a x CDATA 'v'y CDATA 'w'>]><a/>", Syntax::Document).contains("before each attribute definition"));
    assert!(bad("<!DOCTYPE a [<!ATTLIST a x NOTATION (a:b) #IMPLIED>]><a/>", Syntax::Document).contains("has a colon"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR2] [XML §3.2 \[45\],
/// \[46\], §3.2.1 \[47\] to \[50\], §3.2.2 \[51\], §4.7 \[82\],
/// \[83\]] element type declarations and notation
/// declarations are read and checked by their grammar.
#[test]
fn element_and_notation_declarations() {
    let good = "<!DOCTYPE a [<!ELEMENT a (b|(c,d+)*)?><!ELEMENT b EMPTY><!ELEMENT c ANY><!ELEMENT d (#PCDATA|b)*><!ELEMENT e (#PCDATA)><!ELEMENT f ( #PCDATA )*><!ELEMENT g (h)><!NOTATION n PUBLIC 'p'><!NOTATION m PUBLIC 'p' 's'><!NOTATION o SYSTEM 's'>]><a/>";
    assert_eq!(doc(good), "<a></>");
    assert!(bad("<!DOCTYPE a [<!ELEMENT a (b|c,d)>]><a/>", Syntax::Document).contains("mixes '|' and ','"));
    assert!(bad("<!DOCTYPE a [<!ELEMENT a (#PCDATA|b)>]><a/>", Syntax::Document).contains("'*'"));
    assert!(bad("<!DOCTYPE a [<!ELEMENT a (b c)>]><a/>", Syntax::Document).contains("'|', ',' or ')'"));
    assert!(bad("<!DOCTYPE a [<!ELEMENT a ((#PCDATA))>]><a/>", Syntax::Document).contains("requires a name"));
    assert!(bad("<!DOCTYPE a [<!ELEMENT a b>]><a/>", Syntax::Document).contains("'('"));
    assert!(bad("<!DOCTYPE a [<!NOTATION n PUBLIC 'p''s'>]><a/>", Syntax::Document).contains("white space before the system literal"));
    assert!(bad("<!DOCTYPE a [<!NOTATION n PUBLIC 'p{'>]><a/>", Syntax::Document).contains("[XML §2.3 [13]]"));
    assert!(bad("<!DOCTYPE a [<!NOTATION n SYSTEM 's>]><a/>", Syntax::Document).contains("no closing quote"));
    let deep = format!("<!DOCTYPE a [<!ELEMENT a {}b{}>]><a/>", "(".repeat(600), ")".repeat(600));
    assert_eq!(crate::xml::parse(&deep, Syntax::Document, Whitespace::Preserve).unwrap_err().sqlcode(), -900_i32);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR7c] [XML §2.8 \[28a\],
/// WFC: PEs in Internal Subset, WFC: PE Between Declarations, §3.4] a
/// parameter entity referenced between declarations is read as
/// declarations, its conditional sections included or ignored, each
/// ending in it; none is referenced within a declaration.
#[test]
fn parameter_entities_and_conditional_sections() {
    let dtd = "<!ENTITY % yes 'INCLUDE'><!ENTITY % decls '<![&#37;yes;[<!ENTITY e \"in\">]]><![IGNORE[<!ENTITY f \"out\"> <![ x ]]> ]]><!ENTITY f \"kept\">'>";
    assert_eq!(doc(&format!("<!DOCTYPE a [{dtd} %decls;]><a>&e;&f;</a>")), "<a>inkept</>");
    assert!(bad("<!DOCTYPE a [<!ENTITY e '%x;'>]><a/>", Syntax::Document).contains("WFC: PEs in Internal Subset"));
    let within = ["<!ELEMENT a %m;>", "<!ATTLIST a x %t; #IMPLIED>", "<!ENTITY e %v;>", "<!NOTATION n %id;>"];
    for declaration in within {
        let message = bad(&format!("<!DOCTYPE a [<!ENTITY % m '(b)'>{declaration}]><a/>"), Syntax::Document);
        assert!(message.contains("WFC: PEs in Internal Subset"), "{declaration}: {message}");
    }
    assert!(bad("<%a/>", Syntax::Document).contains("requires a name"));
    assert!(bad("<!DOCTYPE a [<!ENTITY % p '<![INCLUDE['> %p; ]]>]><a/>", Syntax::Document).contains("WFC: PE Between Declarations"));
    assert!(bad("<!DOCTYPE a [<![INCLUDE[]]>]><a/>", Syntax::Document).contains("markup declaration"));
    assert!(bad("<!DOCTYPE a [<!ENTITY % p '<![MAYBE[]]>'> %p;]><a/>", Syntax::Document).contains("INCLUDE or IGNORE"));
    assert!(bad("<!DOCTYPE a [<!ENTITY % p '<![IGNORE[ x'> %p;]><a/>", Syntax::Document).contains("no ']]>'"));
    assert!(bad("<!DOCTYPE a [<!ENTITY % p ']]>'> %p;]><a/>", Syntax::Document).contains("ends no conditional section"));
    assert!(bad("<!DOCTYPE a [<!ENTITY % p '&#37;p;'> %p;]><a/>", Syntax::Document).contains("WFC: No Recursion"));
    assert!(
        bad("<!DOCTYPE a [<!ENTITY % q SYSTEM 'q'><!ENTITY % p '<![&#37;q;[]]>'> %p;]><a/>", Syntax::Document).contains("does not read")
    );
    assert!(bad("<!DOCTYPE a [<!ENTITY % p SYSTEM 'p' NDATA n>]><a/>", Syntax::Document).contains("NDATA"));
    assert_eq!(doc("<!DOCTYPE a [ %undeclared; <!-- c --> <?p x?> ]><a/>"), "<a></>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR6a] [XML §2.8 \[22\],
/// \[28\], §4.6] a document type declaration comes once, before the
/// element; a predefined entity is declared, if at all, as its
/// character.
#[test]
fn the_document_type_declaration() {
    assert_eq!(doc("<!--c--> <!DOCTYPE a> <a/>"), "<!--c--><a></>");
    assert_eq!(content("<!DOCTYPE a>\n<a/>", Whitespace::Preserve), "\n<a></>");
    assert!(bad("<a/><!DOCTYPE a>", Syntax::Document).contains("in the prolog"));
    assert!(bad("<!DOCTYPE a><!DOCTYPE a><a/>", Syntax::Document).contains("in the prolog"));
    assert!(bad("<!DOCTYPE a [<!ENTITY e '<!DOCTYPE a>'>]><a>&e;</a>", Syntax::Document).contains("in the prolog"));
    assert!(bad("<!DOCTYPE a [", Syntax::Document).contains("no ']'"));
    assert!(bad("<!DOCTYPE a [<!BOGUS>]><a/>", Syntax::Document).contains("markup declaration"));
    assert!(bad("<!DOCTYPEa><a/>", Syntax::Document).contains("white space"));
    assert!(bad("<!DOCTYPE a:b:c><a/>", Syntax::Document).contains("qualified name"));
    let ok = "<!DOCTYPE a [<!ENTITY lt '&#38;#60;'><!ENTITY amp '&#38;#x26;'><!ENTITY gt '>'><!ENTITY quot '&#34;'><!ENTITY apos \"'\">]><a>&lt;&amp;&gt;</a>";
    assert_eq!(doc(ok), "<a><&></>");
    assert!(bad("<!DOCTYPE a [<!ENTITY lt '<'>]><a/>", Syntax::Document).contains("[XML §4.6]"));
    assert!(bad("<!DOCTYPE a [<!ENTITY gt 'x'>]><a/>", Syntax::Document).contains("[XML §4.6]"));
    assert!(bad("<!DOCTYPE a [<!ENTITY amp SYSTEM 'x'>]><a/>", Syntax::Document).contains("[XML §4.6]"));
}
