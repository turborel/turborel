//! XMLQUERY and XMLEXISTS [ISO/IEC 9075-14:2006 draft §6.18, §8.4]: an
//! XQuery expression evaluated of an SQL statement's values, passed as its
//! context item and variables.

use turborel::db::{Database, Session};
use turborel::error::{Code, Result, SqlState};
use turborel::xml::{Atomic, AtomicType, Item, QName, Sequence};
use turborel::xquery::{Context, ItemKind, Occurrence, SchemaType, SeqType, XQueryFunction};

use crate::common::{fetched, rows};

/// Schema A: table T of an INTEGER K, an XML X and a VARCHAR S, its rows
/// two orders and one null.
fn orders() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML, S VARCHAR(10))").unwrap();
    let a = db.session("A");
    a.run(
        "INSERT INTO T VALUES \
         (1, XMLPARSE(DOCUMENT '<order id=\"7\"><item price=\"10\">pen</item><item price=\"25\">ink</item></order>' STRIP WHITESPACE), 'pen'), \
         (2, XMLPARSE(DOCUMENT '<order id=\"8\"><item price=\"5\">cap</item></order>' STRIP WHITESPACE), 'ink'), \
         (3, NULL, NULL)",
    )
    .unwrap();
    a.commit().unwrap();
    db
}

/// The SQLSTATE `sql` fails with.
fn state(a: &Session, sql: &str) -> SqlState {
    fetched(a, sql).expect_err(sql).sqlstate()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR5, SR10, GR1, GR4] a
/// query of a column passed BY REF as the context item, RETURNING
/// SEQUENCE and CONTENT; the null value of a null context item; NULL ON
/// EMPTY the null value of the empty sequence, EMPTY ON EMPTY the empty
/// sequence.
#[test]
fn query_of_a_context_item_6_18() {
    let a = orders().session("A");
    let q = "SELECT K, XMLSERIALIZE(CONTENT XMLQUERY('//item[@price > 8]/text()' PASSING BY REF X RETURNING SEQUENCE BY REF EMPTY ON EMPTY) AS VARCHAR(40)) FROM T ORDER BY K";
    assert_eq!(rows(&a, q).unwrap(), ["1,penink", "2,", "3,NULL"]);
    let content = "SELECT XMLSERIALIZE(CONTENT XMLQUERY('<total>{count(//item)}</total>' PASSING BY REF X NULL ON EMPTY) AS VARCHAR(40)) FROM T WHERE K = 1";
    assert_eq!(rows(&a, content).unwrap(), ["<total>2</total>"]);
    let count =
        "SELECT K, XMLCAST(XMLQUERY('count(//item)' PASSING BY VALUE X RETURNING SEQUENCE NULL ON EMPTY) AS INTEGER) FROM T ORDER BY K";
    assert_eq!(rows(&a, count).unwrap(), ["1,2", "2,1", "3,NULL"]);
    let empty = "SELECT K, XMLQUERY('//none' PASSING BY REF X RETURNING SEQUENCE NULL ON EMPTY) IS NULL FROM T WHERE K = 1";
    assert_eq!(rows(&a, empty).unwrap(), ["1,TRUE"]);
    let document = "SELECT XMLSERIALIZE(CONTENT XMLQUERY('<n>{data(/order/@id)}</n>' PASSING BY REF X EMPTY ON EMPTY) AS VARCHAR(20)) FROM T WHERE K = 2";
    assert_eq!(rows(&a, document).unwrap(), ["<n>8</n>"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR5a, GR1b, §10.21] SQL
/// values passed as variables, each cast to XML(SEQUENCE) as XMLCAST casts
/// it, a null value the empty sequence; a variable's name the identifier
/// AS gives, a regular identifier in upper case.
#[test]
fn variables_6_18() {
    let a = orders().session("A");
    let q = "SELECT XMLCAST(XMLQUERY('$n + 1, $\"low\", count($E)' PASSING BY REF 41 AS n, 'x' AS \"low\", S AS E RETURNING SEQUENCE EMPTY ON EMPTY) AS VARCHAR(30)) FROM T WHERE K = 3";
    assert!(fetched(&a, q).is_err(), "more than one item is no SQL value");
    let q = "SELECT XMLSERIALIZE(CONTENT XMLQUERY('($N + 1, $low, count($E))' PASSING BY REF 41 AS n, 'x' AS \"low\", S AS E EMPTY ON EMPTY) AS VARCHAR(30)) FROM T WHERE K = 3";
    assert_eq!(rows(&a, q).unwrap(), ["42 x 0"]);
    let q = "SELECT XMLCAST(XMLQUERY('$D instance of xs:date' PASSING BY REF DATE '2026-11-17' AS d RETURNING SEQUENCE EMPTY ON EMPTY) AS BOOLEAN) FROM T WHERE K = 1";
    assert_eq!(rows(&a, q).unwrap(), ["TRUE"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR4, SR5, §11.3] the
/// syntax rules of XMLQUERY, each SQLCODE -103: a returning mechanism of
/// RETURNING CONTENT; RETURNING SEQUENCE of neither a mechanism nor an
/// argument list; two context items; a variable twice; a passing
/// mechanism of a value not XML.
#[test]
fn syntax_rules_6_18() {
    let a = orders().session("A");
    for sql in [
        "SELECT XMLQUERY('1' RETURNING CONTENT BY REF EMPTY ON EMPTY) FROM T",
        "SELECT XMLQUERY('1' RETURNING SEQUENCE EMPTY ON EMPTY) FROM T",
        "SELECT XMLQUERY('1' PASSING BY REF X, X EMPTY ON EMPTY) FROM T",
        "SELECT XMLQUERY('1' PASSING BY REF 1 AS A, 2 AS A EMPTY ON EMPTY) FROM T",
        "SELECT XMLQUERY('1' PASSING BY REF 1 AS A BY VALUE EMPTY ON EMPTY) FROM T",
    ] {
        assert_eq!(fetched(&a, sql).expect_err(sql).code, Code::SyntaxRule, "{sql}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3, GR1a ii 2 B III] an
/// XQuery error, static or dynamic, is SQLSTATE 10000; a context item of
/// more than one item 2200V.
#[test]
fn errors_6_18() {
    let a = orders().session("A");
    assert_eq!(state(&a, "SELECT XMLQUERY('$undeclared' EMPTY ON EMPTY) FROM T"), SqlState::XQUERY_ERROR);
    assert_eq!(state(&a, "SELECT XMLQUERY('1 idiv 0' EMPTY ON EMPTY) FROM T"), SqlState::XQUERY_ERROR);
    assert_eq!(state(&a, "SELECT XMLQUERY('(' EMPTY ON EMPTY) FROM T"), SqlState::XQUERY_ERROR);
    let many = "SELECT XMLQUERY('.' PASSING BY REF XMLQUERY('1, 2' RETURNING SEQUENCE BY REF EMPTY ON EMPTY) EMPTY ON EMPTY) FROM T";
    assert_eq!(state(&a, many), SqlState::INVALID_XQUERY_CONTEXT_ITEM);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §8.4 GR2] XMLEXISTS is True if
/// its query's value is not empty, False if it is, Unknown of a null
/// context item; as a search condition and a value.
#[test]
fn exists_8_4() {
    let a = orders().session("A");
    assert_eq!(rows(&a, "SELECT K FROM T WHERE XMLEXISTS('//item[. = $s]' PASSING BY REF X, S AS \"s\") ORDER BY K").unwrap(), ["1"]);
    assert_eq!(
        rows(&a, "SELECT K, XMLEXISTS('/order[@id = 8]' PASSING BY VALUE X) FROM T ORDER BY K").unwrap(),
        ["1,FALSE", "2,TRUE", "3,NULL"]
    );
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T WHERE NOT XMLEXISTS('()')").unwrap(), ["3"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR1b i 2, §11.5, §10.18]
/// an XML value passed BY REF is its nodes themselves, so a node below
/// another keeps its parent; BY VALUE a copy, each node of it a tree of
/// its own; and a returning mechanism likewise.
#[test]
fn passing_mechanisms_11_5() {
    let a = orders().session("A");
    let q = |m: &str, r: &str| {
        format!(
            "SELECT XMLCAST(XMLQUERY('count($i/..)' PASSING {m} XMLQUERY('/order/item[1]' PASSING BY REF X RETURNING SEQUENCE {r} EMPTY ON EMPTY) AS \"i\" RETURNING SEQUENCE BY REF EMPTY ON EMPTY) AS INTEGER) FROM T WHERE K = 1"
        )
    };
    assert_eq!(rows(&a, &q("BY REF", "BY REF")).unwrap(), ["1"]);
    assert_eq!(rows(&a, &q("BY VALUE", "BY REF")).unwrap(), ["0"]);
    assert_eq!(rows(&a, &q("BY REF", "BY VALUE")).unwrap(), ["0"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.20 SR2d, §6.14 SR1] the
/// namespaces an XMLNAMESPACES in scope declares are the query's, a
/// default its default element namespace.
#[test]
fn namespaces_in_scope_10_20() {
    let a = orders().session("A");
    let q = "SELECT XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"p:r\", XMLNAMESPACES('urn:p' AS \"p\"), \
             XMLQUERY('<p:a/>' EMPTY ON EMPTY)) AS VARCHAR(80)) FROM T WHERE K = 1";
    assert_eq!(rows(&a, q).unwrap(), ["<p:r xmlns:p=\"urn:p\"><p:a/></p:r>"]);
}

/// A plug-in's function: `{urn:example}twice($x as xs:integer) as
/// xs:integer`.
struct Twice {
    name: QName,
    parameters: [SeqType; 1],
    result: SeqType,
}

impl XQueryFunction for Twice {
    fn name(&self) -> &QName {
        &self.name
    }
    fn parameters(&self) -> &[SeqType] {
        &self.parameters
    }
    fn result(&self) -> &SeqType {
        &self.result
    }
    fn call(&self, _: &Context<'_>, arguments: Vec<Sequence>) -> Result<Sequence> {
        let n: i128 = match arguments.first().and_then(|s| s.items().first()) {
            Some(Item::Atomic(a)) => a.lexical.parse().unwrap(),
            _ => 0,
        };
        Ok(Sequence::new(vec![Item::Atomic(Atomic { kind: AtomicType::Integer, lexical: (n * 2).to_string().into() })]))
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §6.18] [XQuery
/// §3.1.5, §4.15] a function a plug-in registers is called by its name and
/// arity, and declared `external`, its argument converted to its type; one
/// of a name and arity taken is refused with SQLCODE -910.
#[test]
fn plug_in_functions_extend_the_library() {
    let db = orders();
    let integer = SeqType::Of(ItemKind::Atomic(SchemaType::Atomic(AtomicType::Integer)), Occurrence::One);
    let name = QName { prefix: None, uri: Some("urn:example".into()), local: "twice".into() };
    let twice = || Twice { name: name.clone(), parameters: [integer.clone()], result: integer.clone() };
    db.register_xquery_function(Box::new(twice())).unwrap();
    assert_eq!(db.register_xquery_function(Box::new(twice())).unwrap_err().code, Code::Api);
    let a = db.session("A");
    let q = "SELECT XMLCAST(XMLQUERY('declare namespace e = \"urn:example\"; e:twice(<a>21</a>)' RETURNING SEQUENCE BY REF EMPTY ON EMPTY) AS INTEGER) FROM T WHERE K = 1";
    assert_eq!(rows(&a, q).unwrap(), ["42"], "RETURNING SEQUENCE BY REF of no argument list");
    let q = "SELECT XMLCAST(XMLQUERY('declare namespace e = \"urn:example\"; e:twice(<a>21</a>)' EMPTY ON EMPTY) AS INTEGER) FROM T WHERE K = 1";
    assert_eq!(rows(&a, q).unwrap(), ["42"]);
    let q = "SELECT XMLCAST(XMLQUERY('declare namespace e = \"urn:example\"; declare function e:twice($x as xs:integer) as xs:integer external; e:twice(2)' EMPTY ON EMPTY) AS INTEGER) FROM T WHERE K = 1";
    assert_eq!(rows(&a, q).unwrap(), ["4"]);
}
