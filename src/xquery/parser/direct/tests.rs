//! Tests of the direct constructors: elements and their attributes,
//! content and its boundary whitespace, comments, processing
//! instructions and CDATA sections, the lexical states they are read in,
//! and the static errors of their attributes.

use crate::xml::Version;
use crate::xquery::tests::{body, code, code_by, syntax_error, written};
use crate::xquery::{AttributeValuePart, Constructor, DirContent, DirElement, DirPi, Expr, QName, boundary_whitespace, parse};

/// The element the direct element constructor `text` makes.
fn element(text: &str) -> DirElement {
    let Constructor::Element(element) = constructor(text) else { panic!("{text} is no element constructor") };
    element
}

/// The constructor the query body `text` is.
fn constructor(text: &str) -> Constructor {
    let Expr::Constructor(constructor) = body(text) else { panic!("{text} is no constructor") };
    *constructor
}

/// Characters of content.
fn chars(text: &str) -> DirContent {
    DirContent::Chars(text.to_owned())
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.7.1, §3.7.1.1]
/// [XQuery Appendix A.1 \[96\] to \[100\]] a direct element: its QName
/// right after `<`, attributes each after whitespace, with whitespace
/// allowed around `=`, their values in either quote; `/>` or content and
/// an end tag.
#[test]
fn elements_and_attributes() {
    let e = element("<p:a b = \"1\" c='2' p:d=\"\"/>");
    assert_eq!(e.name, QName { prefix: Some("p".to_owned()), local: "a".to_owned() });
    assert_eq!(e.attributes.len(), 3);
    assert_eq!(e.attributes[0].value, vec![AttributeValuePart::Chars("1".to_owned())]);
    assert_eq!(e.attributes[2].value, Vec::new());
    assert_eq!(e.content, Vec::new());
    assert_eq!(element("<a></a>"), element("<a/>"));
    assert_eq!(element("<a\n\tb='1'\n/>").attributes.len(), 1);
    for bad in ["< a/>", "<a b=\"1\"c=\"2\"/>", "<a/ >", "<a b/>", "<a b=1/>", "<a =\"1\"/>", "<a b=\"1", "<a", "<1/>", "<a b='1'"] {
        assert_eq!(code(bad), "XPST0003", "{bad}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.7.1.1] [XQuery
/// Appendix A.1 \[98\] to \[100\], \[146\], \[147\]] [XQuery Appendix A.2.1
/// \[149\], \[150\]] an attribute value's parts: characters, its quote
/// doubled for one, `{{` and `}}` for braces, references, and enclosed
/// expressions; no `<`, and no `}` alone.
#[test]
fn attribute_values() {
    let value = |text: &str| element(text).attributes.remove(0).value;
    assert_eq!(value("<a b=\"x\"\"y''{{}}\"/>"), vec![AttributeValuePart::Chars("x\"y''{}".to_owned())]);
    assert_eq!(value("<a b='x\"\"''y'/>"), vec![AttributeValuePart::Chars("x\"\"'y".to_owned())]);
    assert_eq!(
        value("<a b=\"x&amp;&#32;{1}y\"/>"),
        vec![
            AttributeValuePart::Chars("x".to_owned()),
            AttributeValuePart::Reference('&'),
            AttributeValuePart::Reference(' '),
            AttributeValuePart::Enclosed(body("1")),
            AttributeValuePart::Chars("y".to_owned()),
        ]
    );
    assert_eq!(value("<a b=\"{'&quot;', \"}\"}\"/>").len(), 1, "an enclosed expression is read as tokens");
    assert!(syntax_error("<a b=\"<\"/>").contains("holds no '<'"));
    assert!(syntax_error("<a b=\"}\"/>").contains("written '}}'"));
    assert_eq!(code("<a b=\"{}\"/>"), "XPST0003");
    assert_eq!(code("<a b=\"&bogus;\"/>"), "XPST0003");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.7.1] [XQuery
/// Appendix A.1 \[96\]] an end tag repeats its start tag's name exactly,
/// its prefix included, whitespace allowed before its `>`.
#[test]
fn end_tags_match() {
    assert_eq!(element("<a></a >").name.local, "a");
    assert_eq!(element("<p:a></p:a>").name.prefix.as_deref(), Some("p"));
    assert!(syntax_error("<a></b>").contains("the end tag has 'b', and the start tag 'a'"));
    assert!(syntax_error("<p:a></a>").contains("the start tag 'p:a'"));
    assert!(syntax_error("<a></>").contains("the end tag has no name"));
    assert!(syntax_error("<a>").contains("has no end tag"));
    assert_eq!(code("<a></a"), "XPST0003");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.7.1.3] [XQuery
/// Appendix A.1 \[101\], \[102\]] [XQuery Appendix A.2.1 \[148\]] content:
/// characters, `{{` and `}}`, references, CDATA sections, enclosed
/// expressions and nested constructors, in order; a `}` alone, or an
/// `&` that begins no reference, is a syntax error.
#[test]
fn element_content() {
    let e = element("<a>x{{y}}&lt;<![CDATA[<&{]]>{1}<b/><!--c--><?p i?>z</a>");
    assert_eq!(
        e.content,
        vec![
            chars("x{y}"),
            DirContent::Reference('<'),
            DirContent::CData("<&{".to_owned()),
            DirContent::Enclosed(body("1")),
            DirContent::Element(element("<b/>")),
            DirContent::Comment("c".to_owned()),
            DirContent::Pi(DirPi { target: "p".to_owned(), contents: "i".to_owned() }),
            chars("z"),
        ]
    );
    assert!(syntax_error("<a>}</a>").contains("written '}}'"));
    assert_eq!(code("<a>&</a>"), "XPST0003");
    assert_eq!(code("<a>{}</a>"), "XPST0003");
    assert_eq!(code("<a>{1</a>"), "XPST0003");
    assert_eq!(code("<a><![CDATA[x</a>"), "XPST0003");
    assert_eq!(code("<![CDATA[x]]>"), "XPST0003");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.1.3,
/// grammar-note: comments] [XQuery Appendix A.2.4.2] no comment is
/// recognized in a direct constructor: in content it is text, and in a
/// start tag a syntax error; within an enclosed expression it is one.
#[test]
fn no_comments_in_direct_constructors() {
    assert_eq!(element("<eg> (: an example:) </eg>").content, vec![chars(" (: an example:) ")]);
    assert_eq!(code("<eg (: an example:)>{$i//title}</eg>"), "XPST0003");
    assert_eq!(element("<eg>{ (: a comment :) 1 }</eg>").content, vec![DirContent::Enclosed(body("1"))]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.2]
/// the lexer switches state at a direct constructor and back at each
/// enclosed expression: quotes and apostrophes in content are text, a
/// string in an enclosed expression may hold the attribute's quote, and
/// `<` is a constructor where an operand is and a comparison where an
/// operator is.
#[test]
fn constructors_switch_lexical_states() {
    assert_eq!(element("<a>don't \"quote\"</a>").content, vec![chars("don't \"quote\"")]);
    assert_eq!(element("<a b=\"{'&quot;}'}\"/>").attributes[0].value, vec![AttributeValuePart::Enclosed(body("'\"}'"))]);
    assert!(matches!(body("<a/> < <b/>"), Expr::Comparison(_)));
    assert!(matches!(body("1 <2"), Expr::Comparison(_)));
    assert!(matches!(body("<a>{<b>{1}</b>}</a>"), Expr::Constructor(_)));
    assert_eq!(written("<a>{1}</a>, <b>{2}</b>"), "<a>{1}</a>, <b>{2}</b>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.7.1.4]
/// boundary whitespace is whitespace alone delimited by the content's
/// start or end, a direct constructor or an enclosed expression, as the
/// section's examples show; whitespace beside characters, a reference or
/// a CDATA section is not, nor a character a reference stands for.
#[test]
fn boundary_whitespace_as_the_text_defines_it() {
    let boundary = |text: &str| {
        let content = element(text).content;
        (0..content.len()).filter(|&i| boundary_whitespace(&content, i)).collect::<Vec<_>>()
    };
    assert_eq!(boundary("<a>  {\"abc\"}  </a>"), vec![0, 2]);
    assert_eq!(boundary("<cat>\n   <breed>{$b}</breed>\n   <color>{$c}</color>\n</cat>"), vec![0, 2, 4]);
    assert_eq!(boundary("<a> z {\"abc\"}</a>"), Vec::<usize>::new());
    assert_eq!(boundary("<a>&#x20;{\"abc\"}</a>"), Vec::<usize>::new());
    assert_eq!(boundary("<a>{\"  \"}</a>"), Vec::<usize>::new());
    assert_eq!(boundary("<a> </a>"), vec![0]);
    assert_eq!(boundary("<a> <!--c--> <?p?> </a>"), vec![0, 2, 4]);
    assert_eq!(boundary("<a> <![CDATA[ ]]> </a>"), Vec::<usize>::new());
    assert_eq!(boundary("<a> &amp; </a>"), Vec::<usize>::new());
    assert_eq!(boundary("<a> {{ </a>"), Vec::<usize>::new());
    assert!(!boundary_whitespace(&[], 0));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.7.2] [XQuery
/// Appendix A.1 \[103\] to \[106\]] direct comments, with no `--` and no
/// `-` last; direct processing instructions, their target an `NCName`
/// other than `xml` in any case, then whitespace and their contents.
#[test]
fn direct_comments_and_processing_instructions() {
    assert_eq!(body("<!-- a - b -->"), Expr::Constructor(Box::new(Constructor::Comment(" a - b ".to_owned()))));
    assert_eq!(body("<!---->"), Expr::Constructor(Box::new(Constructor::Comment(String::new()))));
    assert_eq!(body("<!---a-->"), Expr::Constructor(Box::new(Constructor::Comment("-a".to_owned()))));
    for bad in ["<!-- a--b -->", "<!--a--->", "<!-- open", "<!- x -->"] {
        assert_eq!(code(bad), "XPST0003", "{bad}");
    }
    assert_eq!(constructor("<?t?>"), Constructor::Pi(DirPi { target: "t".to_owned(), contents: String::new() }));
    assert_eq!(constructor("<?t-x   c ?>"), Constructor::Pi(DirPi { target: "t-x".to_owned(), contents: "c ".to_owned() }));
    assert!(syntax_error("<?xml c?>").contains("not 'xml' in any case"));
    assert!(syntax_error("<?XmL?>").contains("not 'xml' in any case"));
    assert!(syntax_error("<?t#?>").contains("followed by whitespace or '?>'"));
    assert_eq!(code("<? t?>"), "XPST0003");
    assert_eq!(code("<?t open"), "XPST0003");
    assert_eq!(written("<?xml-stylesheet x?>"), "<?xml-stylesheet x?>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.7.1.1,
/// §3.7.1.2] [XQuery Appendix F] the static errors of a direct element's
/// attributes that their names show: two attributes of one name
/// \[err:XQST0040\], two namespace declaration attributes of one name
/// \[err:XQST0071\], one whose value holds an enclosed expression
/// \[err:XQST0022\], one that declares `xml` or `xmlns` or binds another
/// prefix to `xml`'s namespace \[err:XQST0070\], and one that undeclares
/// a prefix by XML 1.0's rules \[err:XQST0085\].
#[test]
fn attribute_static_errors() {
    assert_eq!(code("<a b='1' b='2'/>"), "XQST0040");
    assert_eq!(code("<a p:b='1' p:b='2'/>"), "XQST0040");
    assert!(parse("<a p:b='1' q:b='2'/>", Version::V11).is_ok(), "told apart once prefixes are resolved");
    assert_eq!(code("<a xmlns:p='u' xmlns:p='v'/>"), "XQST0071");
    assert_eq!(code("<a xmlns='u' xmlns='v'/>"), "XQST0071");
    assert_eq!(code("<a xmlns:p='{1}'/>"), "XQST0022");
    assert_eq!(code("<a xmlns='u{1}'/>"), "XQST0022");
    assert_eq!(code("<a xmlns:xml='http://www.w3.org/XML/1998/namespace'/>"), "XQST0070");
    assert_eq!(code("<a xmlns:xmlns='u'/>"), "XQST0070");
    assert_eq!(code("<a xmlns:p='http://www.w3.org/XML/1998/namespace'/>"), "XQST0070");
    assert_eq!(code_by("<a xmlns:p=''/>", Version::V10), "XQST0085");
    assert!(parse("<a xmlns:p=''/>", Version::V11).is_ok());
    assert!(parse("<a xmlns='' xmlns:p='u&amp;v' b='{1}'/>", Version::V10).is_ok());
    let error = parse("<a\n b='1' b='2'/>", Version::V11).unwrap_err();
    assert!(error.msg.contains("at line 2, column 8"), "{error}");
}
