//! XML indexes read for queries of several predicates (RULES.md D106): a
//! predicate of any step, several of one, conjuncts of `and`, and queries
//! joined by AND, each a filter whose lookup's rows the index's rows are
//! among, the rows of each lookup intersected; a predicate that may raise
//! an error read by the lookups that give every row it raises of; the
//! primary answering a query whose every filter's anchor is fixed; and
//! steps of other axes.

use turborel::db::{Database, Session};
use turborel::error::SqlState;
use turborel::params::Params;

use crate::common::{fetched, rows};

/// The methods, as CREATE INDEX names them.
const METHODS: [&str; 4] = ["XML_PATH", "XML_VALUE", "XML_PROPERTY", "XML_PRIMARY"];

/// How many rows table T has.
const ROWS: usize = 300;

/// The names of the items, by row.
const NAMES: [&str; 3] = ["pen", "ink", "cap"];

/// The document of row `k`: an order of a customer of a tier, and two
/// items, each of a price, a name and a quantity; a gift in every
/// twentieth's first item, from the fourth, after its quantity, but before
/// its name in every fortieth's, from the twenty-fourth.
fn document(k: usize) -> String {
    let tier = if k.is_multiple_of(4) { "gold" } else { "silver" };
    let item = |n: usize| {
        let gift = if n == 0 && k % 20 == 4 { "<gift/>" } else { "" };
        let (first, last) = if k % 40 == 24 { (gift, "") } else { ("", gift) };
        let (price, name, qty) = ((k + n * 7) % 50, NAMES[(k + n) % NAMES.len()], (k + n) % 5);
        format!("<item price=\"{price}\">{first}<name>{name}</name><qty>{qty}</qty>{last}</item>")
    };
    format!("<order id=\"{k}\"><customer tier=\"{tier}\"/>{}{}</order>", item(0), item(1))
}

/// Schema A: table T (K INTEGER, X XML(DOCUMENT)) of `ROWS` rows.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML(DOCUMENT))").unwrap();
    let a = db.session("A");
    for k in 1..=ROWS {
        a.run(&format!("INSERT INTO T VALUES ({k}, XMLPARSE(DOCUMENT '{}' STRIP WHITESPACE))", document(k))).unwrap();
    }
    a.commit().unwrap();
    db
}

/// The physical plan of `query`, as EXPLAIN shows it.
fn plan(a: &Session, query: &str) -> String {
    let explained = a.explain(query, &Params::new()).unwrap();
    explained.split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// The keys of T's rows `condition` holds of, in order.
fn keys(a: &Session, condition: &str) -> Vec<String> {
    rows(a, &format!("SELECT K FROM T WHERE {condition} ORDER BY K")).unwrap()
}

/// Conditions of T's rows of several predicates, each with what EXPLAIN
/// shows of the XML_PATH index's query of it, and whether XML_PRIMARY
/// answers it where it reads it.
const CONDITIONS: [(&str, &str, bool); 10] = [
    ("XMLEXISTS('/order[@id = \"76\"]/item[name = \"ink\"][qty]' PASSING BY REF X)", "/order[@id = '76']/item[qty]/name = 'ink'", true),
    ("XMLEXISTS('/order[customer/@tier = \"gold\"]/item[gift]' PASSING BY REF X)", "/order[customer/@tier = 'gold']/item/gift", true),
    ("XMLEXISTS('//order[@id = \"7\" and customer]/item' PASSING BY REF X)", "//order[@id = '7'][customer]/item", true),
    ("XMLEXISTS('/order[customer/@tier = \"gold\"]//gift' PASSING BY REF X)", "/order[customer/@tier = 'gold']//gift", true),
    ("XMLEXISTS('//item[name = \"pen\" and qty = \"4\"][gift]' PASSING BY VALUE X)", "//item[qty = '4'][gift]/name = 'pen'", true),
    ("XMLEXISTS('//order[.//gift]//item' PASSING BY REF X)", "//order[.//gift]//item", false),
    (
        "XMLEXISTS('/order[customer/@tier = \"gold\"]/item[@price > 46]' PASSING BY REF X)",
        "/order[customer/@tier = 'gold']/item/@price > 46",
        false,
    ),
    (
        "XMLEXISTS('/order[item/@price > 47]/customer[@tier = \"gold\"]' PASSING BY REF X)",
        "/order[item/@price > 47]/customer/@tier = 'gold'",
        false,
    ),
    (
        "XMLEXISTS('//item[name = \"cap\"]' PASSING BY REF X) AND XMLEXISTS('/order/customer[@tier = \"gold\"]' PASSING BY REF X)",
        "//item/name = 'cap' and /order/customer/@tier = 'gold'",
        false,
    ),
    (
        "XMLQUERY('/order[@id = \"20\"]/item[name = \"cap\"][qty]' PASSING BY REF X NULL ON EMPTY) IS NOT NULL",
        "/order[@id = '20']/item[qty]/name = 'cap'",
        false,
    ),
];

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §8.4, §6.18]
/// [XQuery §3.2.2, §3.6] RULES.md D34, D106: of each XML index method,
/// every condition of several predicates gives the rows a full read
/// gives: its lookups' rows are fetched and checked, or the primary
/// answers it.
#[test]
fn each_index_gives_the_rows_of_a_full_read() {
    let db = database();
    let a = db.session("A");
    let full: Vec<Vec<String>> = CONDITIONS.iter().map(|(c, _, _)| keys(&a, c)).collect();
    assert!(full.iter().all(|k| !k.is_empty()), "{full:?}");
    a.commit().unwrap();
    for method in METHODS {
        a.run(&format!("CREATE INDEX I ON T (X) USING {method}")).unwrap();
        for ((condition, _, _), expected) in CONDITIONS.iter().zip(&full) {
            assert_eq!(&keys(&a, condition), expected, "{method}: {condition}");
        }
        a.commit().unwrap();
        a.run("DROP INDEX I").unwrap();
    }
}

/// Verifies: [EXTENSION] [XQuery §3.2.2] [Pal et al. §3.1.2, §3.3]
/// RULES.md D34, D106: EXPLAIN shows the index read for each condition,
/// its predicates as filters of their steps, queries joined by AND both;
/// XML_PRIMARY answers each whose every comparison is with a string and
/// every filter's anchor is fixed, the condition no filter's, and checks
/// the rows of the others, or answers one of queries joined by AND; a
/// query whose own lookup it counts past its bound, of every order's
/// items, is read whole, as each row it holds would be sought.
#[test]
fn explain_shows_each_shape_read() {
    let db = database();
    let a = db.session("A");
    a.run("CREATE INDEX I ON T (X) USING XML_PATH").unwrap();
    for (condition, shown, _) in CONDITIONS {
        let found = plan(&a, &format!("SELECT K FROM T WHERE {condition}"));
        assert!(found.contains(&format!("IndexScan A.T using I [xml_path]: T.X has {shown}")), "{condition}: {found}");
    }
    a.commit().unwrap();
    a.run("DROP INDEX I").unwrap();
    a.run("CREATE INDEX P ON T (X) USING XML_PRIMARY").unwrap();
    let every = plan(&a, &format!("SELECT K FROM T WHERE {}", CONDITIONS[2].0));
    assert!(!every.contains("IndexScan"), "{every}");
    for (condition, _, answered) in CONDITIONS {
        let found = plan(&a, &format!("SELECT K FROM T WHERE {condition}"));
        // Of queries joined by AND, the primary may answer one, the other
        // left to the filter.
        // A query the primary answers is answered where it is read; a full
        // read of small values may cost less.
        if answered && found.contains("IndexScan") {
            assert!(found.contains("answered by the index") && !found.contains("Filter"), "{condition}: {found}");
        } else if !answered && !condition.contains(") AND ") {
            assert!(!found.contains("answered by the index"), "{condition}: {found}");
        }
    }
}

/// Verifies: [EXTENSION] [XQuery §3.5.2] RULES.md D15, D106: a predicate
/// of a step before the last compared with a number fails the query of a
/// value it cannot cast, err:FORG0001, with each method's index as with a
/// full read, the lookup of that predicate giving the row though no other
/// lookup of the query does; a row of no such value holds or not alike.
#[test]
fn a_predicate_that_may_raise_is_read_by_its_lookup() {
    let db = database();
    let a = db.session("A");
    let query = "SELECT K FROM T WHERE XMLEXISTS('/order[item/@price > 47]/customer[@tier = \"bronze\"]' PASSING BY REF X)";
    assert_eq!(fetched(&a, query).unwrap(), 0);
    a.run("INSERT INTO T VALUES (1000, XMLPARSE(DOCUMENT '<order><item price=\"n/a\"/></order>' STRIP WHITESPACE))").unwrap();
    a.commit().unwrap();
    assert_eq!(fetched(&a, query).unwrap_err().sqlstate(), SqlState::XQUERY_ERROR);
    a.rollback().unwrap();
    for method in METHODS {
        a.run(&format!("CREATE INDEX I ON T (X) USING {method}")).unwrap();
        let read = plan(&a, query).contains("IndexScan");
        assert!(read || method == "XML_PRIMARY", "{method}: {}", plan(&a, query));
        let error = fetched(&a, query).unwrap_err();
        assert_eq!(error.sqlstate(), SqlState::XQUERY_ERROR, "{method}: {error}");
        assert!(error.msg.contains("err:FORG0001"), "{error}");
        a.rollback().unwrap();
        a.run("DROP INDEX I").unwrap();
    }
}

/// Conditions of T's rows of steps of other axes, each with what EXPLAIN
/// shows of the XML_PATH index's query of it, and whether XML_PRIMARY
/// answers it where it reads it.
const AXES: [(&str, &str, bool); 12] = [
    ("XMLEXISTS('/order/item/../customer[@tier = \"gold\"]' PASSING BY REF X)", "/order[item]/customer/@tier = 'gold'", true),
    ("XMLEXISTS('/order/item/gift/ancestor::item' PASSING BY REF X)", "/order/item/gift", true),
    ("XMLEXISTS('//*/self::gift' PASSING BY REF X)", "//gift", true),
    (
        "XMLEXISTS('/order[@id = \"35\"]/item[name = \"cap\"]/@price/..' PASSING BY REF X)",
        "/order[@id = '35']/item[@price]/name = 'cap'",
        true,
    ),
    ("XMLEXISTS('//gift/..' PASSING BY REF X)", "//gift", true),
    ("XMLEXISTS('//gift/ancestor::order' PASSING BY REF X)", "//gift/ancestor::order", true),
    ("XMLEXISTS('//name/following-sibling::gift' PASSING BY REF X)", "//name/following-sibling::gift", true),
    ("XMLEXISTS('//name/preceding-sibling::gift' PASSING BY REF X)", "//name/preceding-sibling::gift", true),
    ("XMLEXISTS('//gift/following-sibling::qty' PASSING BY REF X)", "//gift/following-sibling::qty", true),
    ("XMLEXISTS('//gift/preceding::customer' PASSING BY REF X)", "//gift/preceding::customer", true),
    ("XMLEXISTS('//customer[@tier = \"gold\"]/following::gift' PASSING BY REF X)", "//customer[@tier = 'gold']/following::gift", true),
    ("XMLEXISTS('//item[gift]/name/text()' PASSING BY REF X)", "//item[gift]/name", false),
];

/// Verifies: [EXTENSION] [XQuery §3.2.1.1] [Pal et al. §2.2, §3.1.2]
/// RULES.md D34, D106: of each XML index method, every condition of steps
/// of the self, parent, ancestor, following and preceding axes, and of a
/// kind test, gives the rows a full read gives, the index read for it,
/// EXPLAIN showing its query; XML_PRIMARY answers those the path, its
/// predicates and its relation select exactly, a relation of a gift by
/// the nodes' ORDPATHs, and checks the rows of the others.
#[test]
fn other_axes_give_the_rows_of_a_full_read() {
    let db = database();
    let a = db.session("A");
    let full: Vec<Vec<String>> = AXES.iter().map(|(c, _, _)| keys(&a, c)).collect();
    assert!(full.iter().all(|k| !k.is_empty()), "{full:?}");
    a.commit().unwrap();
    for method in METHODS {
        a.run(&format!("CREATE INDEX I ON T (X) USING {method}")).unwrap();
        for ((condition, shown, answered), expected) in AXES.iter().zip(&full) {
            assert_eq!(&keys(&a, condition), expected, "{method}: {condition}");
            let found = plan(&a, &format!("SELECT K FROM T WHERE {condition}"));
            if method == "XML_PATH" {
                assert!(found.contains(&format!("using I [xml_path]: T.X has {shown}")), "{condition}: {found}");
            }
            if method == "XML_PRIMARY" && found.contains("IndexScan") {
                assert_eq!(found.contains("answered by the index"), *answered, "{condition}: {found}");
            }
            // Of a relation of few nodes, the primary answers by ORDPATHs.
            if method == "XML_PRIMARY" && condition.contains("//gift/") && shown.contains("::") {
                assert!(found.contains("answered by the index"), "{condition}: {found}");
            }
        }
        a.commit().unwrap();
        a.run("DROP INDEX I").unwrap();
    }
}
