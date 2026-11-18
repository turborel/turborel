//! Tests of constructors [XQuery §3.7].

use super::super::super::testing::{fails_with, run};

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.7.1,
/// §3.7.1.3] a direct element of attributes and content: enclosed atomic
/// values a text a space apart, adjacent text one, nodes copied, a
/// document node its children, attributes of the content before the rest.
#[test]
fn direct_elements() {
    assert_eq!(run("<a>{1, 2, 3}</a>"), "<a>1 2 3</a>");
    assert_eq!(run("<c>{1}{2}{3}</c>, <b>{1, '2', '3'}</b>"), "<c>123</c><b>1 2 3</b>");
    assert_eq!(run("<fact>I saw <howmany>{5 + 3}</howmany> cats.</fact>"), "<fact>I saw <howmany>8</howmany> cats.</fact>");
    assert_eq!(run("<chapter ref='[{1, 5 to 7, 9}]'/>"), "<chapter ref=\"[1 5 6 7 9]\"/>");
    assert_eq!(run("<a>{attribute b {1}, 'x'}</a>, <a>{<b x='1'/>/@x}</a>"), "<a b=\"1\">x</a><a x=\"1\"/>");
    assert_eq!(run("<a>{document {<b/>, 'c'}}</a>"), "<a><b/>c</a>");
    assert_eq!(run("let $b := <b><c/></b> return (<a>{$b}</a>/b is $b, count(<a>{$b}</a>/b/c))"), "false 1");
    assert_eq!(run("<a x=' y\tz'/>/@x/string()"), " y z");
    assert_eq!(run("<a xml:id=' p  q '/>/@xml:id/string()"), "p q");
    assert!(fails_with("<a>{'x', attribute b {1}}</a>", "err:XQTY0024"));
    assert!(fails_with("<a b='1'>{attribute b {2}}</a>", "err:XQDY0025"));
    assert!(fails_with("<a p:b='1' q:b='2' xmlns:p='u' xmlns:q='u'/>", "err:XQST0040"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.7.1.4,
/// §4.3] boundary whitespace is stripped by the default policy and kept by
/// `preserve`; whitespace a reference or an enclosed expression makes is
/// kept by either.
#[test]
fn boundary_whitespace() {
    assert_eq!(run("<a>  {'abc'}  </a>"), "<a>abc</a>");
    assert_eq!(run("declare boundary-space preserve; <a>  {'abc'}  </a>"), "<a>  abc  </a>");
    assert_eq!(run("<a> z {'abc'}</a>, <a>&#x20;{'abc'}</a>, <a>{'  '}</a>"), "<a> z abc</a><a> abc</a><a>  </a>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.7.1.2,
/// §3.7.4] namespace declaration attributes bind prefixes in the static
/// context and on the element; a name's namespace is bound on the element
/// that needs it, the prolog's prefix used; an element in no namespace
/// within a default namespace undeclares it.
#[test]
fn namespaces_of_constructed_elements() {
    assert_eq!(run("<a xmlns='u'><b/></a>"), "<a xmlns=\"u\"><b/></a>");
    assert_eq!(run("declare namespace p = 'u'; <p:a/>"), "<p:a xmlns:p=\"u\"/>");
    assert_eq!(
        run("declare namespace p = 'u'; <p:a q:b='1' xmlns:q='v' xmlns:r='w'/>"),
        "<p:a xmlns:q=\"v\" xmlns:r=\"w\" xmlns:p=\"u\" q:b=\"1\"/>"
    );
    assert_eq!(run("<a xmlns='u'>{element b {}}</a>"), "<a xmlns=\"u\"><b/></a>");
    assert_eq!(run("<a xmlns='u'><b xmlns=''/></a>"), "<a xmlns=\"u\"><b xmlns=\"\"/></a>");
    assert_eq!(run("<a xmlns='u'>{<b xmlns=''/>}</a>"), "<a xmlns=\"u\"><b xmlns=\"\"/></a>");
    assert!(fails_with("<p:a/>", "err:XPST0008"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.7.3] the
/// computed constructors: an element's name fixed or of a string,
/// resolved in the static context \[err:XQDY0074\]; an attribute named
/// xmlns err:XQDY0044; text of no value no node; a comment of two
/// hyphens err:XQDY0072; a processing instruction named xml
/// err:XQDY0064, holding '?>' err:XQDY0026, of no NCName err:XQDY0041.
#[test]
fn computed_constructors() {
    assert_eq!(run("element a { attribute b { 1 + 1 }, element c { 'd' } }"), "<a b=\"2\"><c>d</c></a>");
    assert_eq!(run("element { concat('x', 'y') } { 1 }"), "<xy>1</xy>");
    assert_eq!(run("text { 'a', 'b' }, count(text { () }), comment { 'c' }, processing-instruction p { '  q' }"), "a b0<!--c--><?p q?>");
    assert_eq!(run("document { <a/>, 'b' }"), "<a/>b");
    assert_eq!(run("<!--x-->, <?p q?>"), "<!--x--><?p q?>");
    assert!(fails_with("element { 'p:a' } {}", "err:XQDY0074"));
    assert!(fails_with("element { 1, 2 } {}", "err:XPTY0004"));
    assert!(fails_with("attribute xmlns { 1 }", "err:XQDY0044"));
    assert!(fails_with("comment { 'a--b' }", "err:XQDY0072"));
    assert!(fails_with("processing-instruction { 'XmL' } {}", "err:XQDY0064"));
    assert!(fails_with("processing-instruction p { '?>' }", "err:XQDY0026"));
    assert!(fails_with("processing-instruction { '1a' } {}", "err:XQDY0041"));
    assert!(fails_with("document { attribute a {1} }", "err:XPTY0004"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.7.1.3,
/// §4.6, §4.9] construction mode strip makes no copied attribute an ID
/// but xml:id; copy-namespaces no-preserve keeps the bindings a copied
/// element's names use.
#[test]
fn construction_and_copy_namespaces_modes() {
    let copy = "<x xmlns:p='u' xmlns:q='v'><p:y/></x>/*";
    assert_eq!(run(&format!("<a>{{{copy}}}</a>")), "<a><p:y xmlns:p=\"u\" xmlns:q=\"v\"/></a>");
    assert_eq!(run(&format!("declare copy-namespaces no-preserve, inherit; <a>{{{copy}}}</a>")), "<a><p:y xmlns:p=\"u\"/></a>");
    assert_eq!(run("declare construction strip; count(<a>{<b/>}</a>/b)"), "1");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQuery §3.13] a
/// validate expression, strict, of no element declared in scope
/// err:XQDY0084; its operand not one document or element node
/// err:XQTY0030; a document of two elements err:XQDY0061.
#[test]
fn validate() {
    assert!(fails_with("validate { <a/> }", "err:XQDY0084"));
    assert!(fails_with("validate strict { document { <a/> } }", "err:XQDY0084"));
    assert!(fails_with("validate { 1 }", "err:XQTY0030"));
    assert!(fails_with("validate { document { <a/>, <b/> } }", "err:XQDY0061"));
}
