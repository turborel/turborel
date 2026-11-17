//! Tests of parsing: documents and content, characters and line ends,
//! white space, markup and namespaces. The document type declaration's
//! are in `dtd/tests.rs`, decoding's in `encoding/tests.rs`.

use super::*;
use crate::error::SqlState;
use crate::xml::tests::shape;

/// The tree of the document `text`, written as text.
pub(super) fn doc(text: &str) -> String {
    shape(&parse(text, Syntax::Document, Whitespace::Preserve).unwrap())
}

/// The tree of the content `text` under `whitespace`, written as text.
pub(super) fn content(text: &str, whitespace: Whitespace) -> String {
    shape(&parse(text, Syntax::Content, whitespace).unwrap())
}

/// The tree of the document the binary string `bytes` encodes, written
/// as text.
pub(super) fn shape_of_bytes(bytes: &[u8]) -> String {
    shape(&parse_bytes(bytes, Syntax::Document, Whitespace::Preserve).unwrap())
}

/// The message of the exception `text` raises under `syntax`, which is
/// invalid XML document or content as `syntax` is.
pub(super) fn bad(text: &str, syntax: Syntax) -> String {
    let error = parse(text, syntax, Whitespace::Preserve).unwrap_err();
    let state = match syntax {
        Syntax::Document => SqlState::INVALID_XML_DOCUMENT,
        Syntax::Content => SqlState::INVALID_XML_CONTENT,
    };
    assert_eq!(error.sqlstate(), state, "{error}");
    error.to_string()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR6a i] [XML §2.1 \[1\]] a
/// document has one element, and nothing but comments, processing
/// instructions and white space beside it: else invalid XML document.
#[test]
fn a_document_has_one_element() {
    assert_eq!(doc(" <a/> <!--c--> "), "<a></><!--c-->");
    assert!(bad("", Syntax::Document).contains("has an element"));
    assert!(bad("<!--c-->", Syntax::Document).contains("has an element"));
    assert!(bad("<a/><b/>", Syntax::Document).contains("one element"));
    assert!(bad("t<a/>", Syntax::Document).contains("outside its element"));
    assert!(bad("<a/>&amp;", Syntax::Document).contains("outside its element"));
    assert!(bad("<a/><![CDATA[ ]]>", Syntax::Document).contains("outside its element"));
    assert!(bad("<a/>&#32;", Syntax::Document).contains("outside its element"));
    assert!(bad("<a></a>t", Syntax::Document).contains("[ISO/IEC 9075-14:2006 draft §10.16 GR6a i]"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR4, GR6a ii, GR7a i]
/// [XML §4.3.2 \[78\]] content is a document or a well-formed external
/// parsed entity: any elements, text and references, its document
/// node's children what the content production makes.
#[test]
fn content_is_a_document_or_an_entity() {
    assert_eq!(content("", Whitespace::Preserve), "");
    assert_eq!(content("t<a/>u<b/>&amp;", Whitespace::Preserve), "t<a></>u<b></>&");
    assert_eq!(content("<?xml version='1.0' encoding='UTF-8'?><a/><b/>", Whitespace::Preserve), "<a></><b></>");
    assert_eq!(content("<?xml encoding='UTF-8'?>x", Whitespace::Preserve), "x");
    assert_eq!(content("<?xml version='1.0'?>\n<a/>\n", Whitespace::Preserve), "\n<a></>\n");
    let both = bad("<?xml version='1.0'?><a/><b/>", Syntax::Content);
    assert!(both.contains("one element") && both.contains("has an encoding"), "{both}");
    assert!(bad("<?xml version='1.0' encoding='UTF-8' standalone='no'?>t", Syntax::Content).contains("no standalone"));
    assert!(bad("<!DOCTYPE a><a/><b/>", Syntax::Content).contains("no document type declaration"));
    assert!(bad("<?xml encoding='UTF-8'?><a/>", Syntax::Document).contains("has a version"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR2, GR3] [XML §2.8 \[23\]
/// to \[26\], §2.9 \[32\]] the XML declaration begins the text, names
/// version 1.0 or 1.1, then an encoding and a standalone declaration,
/// each in its place.
#[test]
fn the_xml_declaration() {
    assert_eq!(doc("<?xml version=\"1.1\" encoding='x-y_1.2' standalone='yes' ?><a/>"), "<a></>");
    assert_eq!(doc("<?xml version = '1.0' standalone=\"no\"?><a/>"), "<a></>");
    assert!(bad("<?xml version='1.2'?><a/>", Syntax::Document).contains("'1.0' or '1.1'"));
    assert!(bad("<?xml version='1.0' standalone='maybe'?><a/>", Syntax::Document).contains("'yes' or 'no'"));
    assert!(bad("<?xml version='1.0' encoding='9x'?><a/>", Syntax::Document).contains("no encoding name"));
    assert!(bad("<?xml version='1.0' standalone='yes' encoding='UTF-8'?><a/>", Syntax::Document).contains("'?>'"));
    assert!(bad("<?xml version='1.0'standalone='yes'?><a/>", Syntax::Document).contains("'?>'"));
    assert!(bad("<?xml version='1.0?><a/>", Syntax::Document).contains("end with '"));
    assert!(bad("<?xml version=1.0?><a/>", Syntax::Document).contains("quoted value"));
    assert!(bad(" <?xml version='1.0'?><a/>", Syntax::Document).contains("target 'xml' is reserved"));
    assert!(bad("<a><?XmL x?></a>", Syntax::Document).contains("target 'xml' is reserved"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR2, GR3] [XML §2.11]
/// line ends are line feeds: a carriage return and line feed, or a
/// carriage return alone; of XML 1.1 also NEL, a carriage return and NEL,
/// and the line separator, which XML 1.0 keeps as characters.
#[test]
fn line_ends_are_normalized() {
    assert_eq!(doc("<a>1\r\n2\r3\n4</a>"), "<a>1\n2\n3\n4</>");
    assert_eq!(doc("<a>1\u{85}2\u{2028}3</a>"), "<a>1\u{85}2\u{2028}3</>");
    assert_eq!(doc("<?xml version='1.1'?><a>1\u{85}2\r\u{85}3\u{2028}4\r</a>"), "<a>1\n2\n3\n4\n</>");
    assert_eq!(doc("<a x='1\r\n2'/>"), "<a x=\"1 2\"></>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR2, GR3] [XML 1.0 §2.2
/// \[2\]] [XML 1.1 §2.2 \[2a\]] [XML §4.1 WFC: Legal Character] a character
/// of neither version is never in a text; XML 1.1 writes its restricted
/// characters only as references, which XML 1.0 has none of.
#[test]
fn characters_by_version() {
    assert!(bad("<a>\u{1}</a>", Syntax::Document).contains("#x1 may not be written in XML 1.0"));
    assert!(bad("<?xml version='1.1'?><a>\u{1}</a>", Syntax::Document).contains("in XML 1.1"));
    assert!(bad("<a>\u{FFFE}</a>", Syntax::Document).contains("line 1, column 4"));
    assert_eq!(doc("<?xml version='1.1'?><a>&#x1;&#127;</a>"), "<a>\u{1}\u{7F}</>");
    assert!(bad("<a>&#x1;</a>", Syntax::Document).contains("WFC: Legal Character"));
    assert!(bad("<?xml version='1.1'?><a>&#0;</a>", Syntax::Document).contains("WFC: Legal Character"));
    assert!(bad("<a>&#xD800;</a>", Syntax::Document).contains("#xD800"));
    assert!(bad("<a>&#99999999999;</a>", Syntax::Document).contains("too large"));
    assert!(bad("<a>&#x;</a>", Syntax::Document).contains("[XML §4.1 [66]]"));
    assert!(bad("<a>&#12</a>", Syntax::Document).contains("[XML §4.1 [66]]"));
    assert_eq!(doc("<a>&#x10FFFF;&#65;&#x41;</a>"), "<a>\u{10FFFF}AA</>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR8] PRESERVE WHITESPACE
/// keeps every text node; STRIP WHITESPACE removes those of white space
/// alone from the document node and from potentially
/// whitespace-strippable elements, and keeps any with other text.
#[test]
fn white_space_is_stripped_as_the_option_says() {
    let text = " <a> <b> x </b>\n<c>&#32;</c><d><![CDATA[ ]]></d></a> ";
    assert_eq!(content(text, Whitespace::Preserve), " <a> <b> x </>\n<c> </><d> </></> ");
    assert_eq!(content(text, Whitespace::Strip), "<a><b> x </><c></><d></></>");
    assert_eq!(content("<a> <!--c--> </a>", Whitespace::Strip), "<a><!--c--></>");
    let tree = parse(" <a> </a> ", Syntax::Document, Whitespace::Preserve).unwrap();
    assert_eq!(shape(&tree), "<a> </>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR8a] [XML §2.10] an
/// element is potentially whitespace-strippable at the top unless its
/// `xml:space` is preserve, wherever its `xml:space` is default, and
/// within one that is unless its `xml:space` is preserve.
#[test]
fn xml_space_decides_what_is_strippable() {
    let text = "<a xml:space='preserve'> <b> </b><c xml:space='default'> <d> </d></c></a>";
    assert_eq!(
        content(text, Whitespace::Strip),
        "<a xml:space{http://www.w3.org/XML/1998/namespace}=\"preserve\"> <b> </><c xml:space{http://www.w3.org/XML/1998/namespace}=\"default\"><d></></></>"
    );
    let nested = "<a> <b xml:space='preserve'> <c> </c></b></a>";
    assert_eq!(content(nested, Whitespace::Strip), "<a><b xml:space{http://www.w3.org/XML/1998/namespace}=\"preserve\"> <c> </></></>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR9] [XML §2.5 \[15\], §2.6
/// \[16\], \[17\], §2.7 \[18\], §2.4 \[14\]] comments hold no `--`; a processing
/// instruction's content follows white space after its target; a CDATA
/// section is text; character data holds no `]]>`.
#[test]
fn comments_instructions_and_cdata() {
    assert_eq!(doc("<a><!-- c - d --><?t?><?t  x ?><![CDATA[<&]]>y]</a>"), "<a><!-- c - d --><?t ?><?t x ?><&y]</>");
    assert!(bad("<a><!-- a -- b --></a>", Syntax::Document).contains("no '--'"));
    assert!(bad("<a><!-- a ---></a>", Syntax::Document).contains("no '--'"));
    assert!(bad("<a><!-- a </a>", Syntax::Document).contains("no '--' to end it"));
    assert_eq!(doc("<a><?t-x?></a>"), "<a><?t-x ?></>");
    assert!(bad("<a><?a:b x?></a>", Syntax::Document).contains("has a colon"));
    assert!(bad("<a><?tx</a>", Syntax::Document).contains("white space"));
    assert!(bad("<a><![CDATA[x</a>", Syntax::Document).contains("']]>'"));
    assert!(bad("<a>x]]>y</a>", Syntax::Document).contains("no ']]>'"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR9] [XML §3 WFC: Element
/// Type Match, §3.1 \[40\] to \[44\]] an end tag matches the start tag open
/// last; every element ends; tags are written as XML writes them.
#[test]
fn tags_match_and_end() {
    assert_eq!(doc("<a ><b\n/></a\t>"), "<a><b></></>");
    assert!(bad("<a></b>", Syntax::Document).contains("WFC: Element Type Match"));
    assert!(bad("<a>", Syntax::Document).contains("no end tag"));
    assert!(bad("</a>", Syntax::Document).contains("ends no element"));
    assert!(bad("<a></a", Syntax::Document).contains("'>'"));
    assert!(bad("< a/>", Syntax::Document).contains("requires a name"));
    assert!(bad("<a x='1'y='2'/>", Syntax::Document).contains("white space before each attribute"));
    assert!(bad("<!a/>", Syntax::Document).contains("requires a name"));
    let place = bad("<a>\n  <b>\n</a>", Syntax::Document);
    assert!(place.contains("at line 3, column 5"), "{place}");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR9] [XML §3.1 WFC:
/// Unique Att Spec, WFC: No < in Attribute Values, §3.3.3] an attribute
/// is specified once; its value has no '<' and is normalized: white
/// space a space, each reference its character.
#[test]
fn attribute_values() {
    assert_eq!(doc("<a x=' 1\t2\n' y=\"&#10;&lt;&quot;'\"/>"), "<a x=\" 1 2 \" y=\"\n<\"'\"></>");
    assert!(bad("<a x='1' x='2'/>", Syntax::Document).contains("WFC: Unique Att Spec"));
    assert!(bad("<a x='<'/>", Syntax::Document).contains("WFC: No < in Attribute Values"));
    assert!(bad("<a x='1/>", Syntax::Document).contains("no closing quote"));
    assert!(bad("<a x=1/>", Syntax::Document).contains("quoted attribute value"));
    assert!(bad("<a x/>", Syntax::Document).contains("'='"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR2, GR9] [Namespaces 1.0
/// §2, §4, §5.1, §5.2] an element's name is in its prefix's namespace,
/// or the default one if it has none; an attribute's without a prefix is
/// in none; declarations are no attributes and are scoped to their
/// element.
#[test]
fn names_are_resolved_by_their_namespaces() {
    let text = "<a xmlns='d' xmlns:p='u'><p:b p:x='1' y='2'/><c xmlns=''/><xml:e xml:lang='en'/></a>";
    assert_eq!(
        doc(text),
        "<a{d}><p:b{u} p:x{u}=\"1\" y=\"2\"></><c></><xml:e{http://www.w3.org/XML/1998/namespace} xml:lang{http://www.w3.org/XML/1998/namespace}=\"en\"></></>"
    );
    let tree = parse(text, Syntax::Document, Whitespace::Preserve).unwrap();
    let a = tree.children(crate::xml::NodeId::ROOT).next().unwrap();
    assert_eq!(tree.declared(a).len(), 2);
    assert_eq!(tree.attributes(a).count(), 0);
    assert!(bad("<p:a/>", Syntax::Document).contains("NSC: Prefix Declared"));
    assert!(bad("<a><p:b xmlns:p='u'/><p:c/></a>", Syntax::Document).contains("'p' of 'p:c' is not declared"));
    assert!(bad("<a p:x='1'/>", Syntax::Document).contains("[Namespaces 1.0 §4 NSC: Prefix Declared]"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR2, GR3] [Namespaces 1.0
/// §2, §3 \[6\], §5.3] [Namespaces 1.1 §4 \[6\], §5, §6.1, §6.3] names are
/// qualified names; no tag has two attributes of one expanded name; XML
/// 1.0 binds no prefix to an empty name, which XML 1.1 undeclares it
/// with.
#[test]
fn qualified_names_and_their_declarations() {
    assert!(bad("<a:b:c/>", Syntax::Document).contains("[Namespaces 1.0 §3 [6]]"));
    assert!(bad("<?xml version='1.1'?><a:/>", Syntax::Document).contains("[Namespaces 1.1 §4 [6]]"));
    assert!(bad("<a xmlns:p='u' xmlns:q='u' p:x='1' q:x='2'/>", Syntax::Document).contains("[Namespaces 1.0 §5.3]"));
    assert_eq!(doc("<a xmlns='u' xmlns:p='u' x='1' p:x='2'/>"), "<a{u} x=\"1\" p:x{u}=\"2\"></>");
    assert!(bad("<a xmlns:p=''/>", Syntax::Document).contains("no namespace name [Namespaces 1.0 §2]"));
    assert_eq!(doc("<?xml version='1.1'?><a xmlns:p='u'><b xmlns:p=''/></a>"), "<a><b></></>");
    let undeclared = bad("<?xml version='1.1'?><a xmlns:p='u'><b xmlns:p=''><p:c/></b></a>", Syntax::Document);
    assert!(undeclared.contains("[Namespaces 1.1 §5 NSC: Prefix Declared]"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR2, GR3] [Namespaces 1.0
/// §2, §4] [Namespaces 1.1 §3 NSC: Reserved Prefixes and Namespace Names]
/// xml is bound to its namespace alone and xmlns is never declared; no
/// other prefix is bound to theirs; other prefixes beginning with xml are
/// no error.
#[test]
fn reserved_prefixes() {
    assert!(bad("<a xmlns:xml='other'/>", Syntax::Document).contains("xml is bound to"));
    assert_eq!(doc("<a xmlns:xml='http://www.w3.org/XML/1998/namespace'/>"), "<a></>");
    assert!(bad("<a xmlns:xmlns='u'/>", Syntax::Document).contains("never declared"));
    assert!(
        bad("<?xml version='1.1'?><a xmlns:p='http://www.w3.org/XML/1998/namespace'/>", Syntax::Document)
            .contains("[Namespaces 1.1 §3 NSC: Reserved Prefixes and Namespace Names]")
    );
    assert!(bad("<a xmlns='http://www.w3.org/2000/xmlns/'/>", Syntax::Document).contains("no prefix but xml or xmlns"));
    assert!(bad("<xmlns:a/>", Syntax::Document).contains("names no element or attribute"));
    assert!(bad("<a xmlns:1='u'/>", Syntax::Document).contains("is no prefix"));
    assert_eq!(doc("<a xmlns:xmlfoo='u'><xmlfoo:b/></a>"), "<a><xmlfoo:b{u}></></>");
}
