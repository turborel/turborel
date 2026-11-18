//! XML indexes read by the planner (RULES.md D86): XMLEXISTS in WHERE and
//! ON, XMLQUERY NULL ON EMPTY IS NOT NULL and XMLTABLE's row pattern,
//! each of a path reduced to a lookup, the rows it gives fetched and
//! checked, so the results are those of a full read; a conjunct that can
//! raise an error keeping the full read (D15).

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};
use turborel::params::Params;
use turborel::value::Value;

use crate::common::{fetched, rows};

/// The methods, as CREATE INDEX names them.
const METHODS: [&str; 3] = ["XML_PATH", "XML_VALUE", "XML_PROPERTY"];

/// How many rows table T has.
const ROWS: usize = 240;

/// The names of the items, by row.
const NAMES: [&str; 4] = ["pen", "ink", "cap", "it's"];

/// The document of row `k`: an order, of no item every tenth, of a note
/// every seventh, in a namespace every eleventh, else of an item of a
/// name, a price as an attribute and an element, a quantity, and every
/// thirtieth a gift.
fn document(k: usize) -> String {
    let (price, name) = (k % 50, NAMES[k % NAMES.len()].replace('\'', "''"));
    if k.is_multiple_of(10) {
        format!("<order id=\"{k}\"/>")
    } else if k.is_multiple_of(7) {
        format!("<order id=\"{k}\"><note>{name}</note></order>")
    } else if k.is_multiple_of(11) {
        format!("<p:order xmlns:p=\"urn:p\" id=\"{k}\"><p:item><p:name>{name}</p:name></p:item></p:order>")
    } else {
        let gift = if k % 30 == 1 { format!("<gift>{name}</gift>") } else { String::new() };
        format!(
            "<order id=\"{k}\"><item price=\"{price}\"><name>{name}</name><price>{price}.5</price><qty>{}</qty>{gift}</item></order>",
            k % 3
        )
    }
}

/// Schema A: table T (K INTEGER, X XML(DOCUMENT), S XML) of `ROWS` rows,
/// S each row's document as XML(SEQUENCE), and its null row 0.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML(DOCUMENT), S XML)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (0, NULL, NULL)").unwrap();
    for k in 1..ROWS {
        let d = document(k);
        a.run(&format!("INSERT INTO T VALUES ({k}, XMLPARSE(DOCUMENT '{d}' STRIP WHITESPACE), XMLPARSE(DOCUMENT '{d}' STRIP WHITESPACE))"))
            .unwrap();
    }
    a.commit().unwrap();
    db
}

/// The physical plan of `query`, as EXPLAIN shows it.
fn plan(a: &Session, query: &str) -> String {
    let explained = a.explain(query, &Params::new()).unwrap();
    explained.split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// The conditions on T's rows the tests ask of each index: paths, their
/// predicates and comparisons, of every shape a lookup takes, and some
/// that reduce to none.
const CONDITIONS: [&str; 23] = [
    "XMLEXISTS('/order/item' PASSING BY REF X)",
    "XMLEXISTS('//name[. = \"pen\"]' PASSING BY REF X)",
    "XMLEXISTS('//item[@price > 40]' PASSING BY REF X)",
    "XMLEXISTS('//item[price >= 45]' PASSING BY VALUE X)",
    "XMLEXISTS('//item[price = 25.5]' PASSING BY REF X)",
    "XMLEXISTS('//item[price != 5.5]' PASSING BY REF X)",
    "XMLEXISTS('//item[3 > qty]' PASSING BY REF X)",
    "XMLEXISTS('//@id[. = \"77\"]' PASSING BY REF X)",
    "XMLEXISTS('/order/*/name' PASSING BY REF X)",
    "XMLEXISTS('//item[name]' PASSING BY REF X)",
    "XMLEXISTS('//item[qty eq \"2\"]' PASSING BY REF X)",
    "XMLEXISTS('//item[gift eq \"ink\"]' PASSING BY REF X)",
    "XMLEXISTS('//item[@price < 3.5]' PASSING BY REF X)",
    "XMLEXISTS('//name[. = \"it''s\"]' PASSING BY REF X)",
    "XMLEXISTS('declare namespace p = \"urn:p\"; /p:order/p:item' PASSING BY REF X)",
    "XMLEXISTS('//note' PASSING BY REF X) AND K > 100",
    "XMLEXISTS('//item[@price > 40]' PASSING BY REF X) OR K < 3",
    "XMLEXISTS('//item' PASSING BY REF X) AND XMLEXISTS('//note' PASSING BY REF X)",
    "XMLQUERY('//note' PASSING BY REF X RETURNING SEQUENCE BY REF NULL ON EMPTY) IS NOT NULL",
    "XMLQUERY('//note' PASSING BY REF X NULL ON EMPTY) IS NOT NULL",
    "XMLEXISTS('//item[1]' PASSING BY REF X)",
    "XMLEXISTS('//nothing' PASSING BY REF X)",
    "XMLEXISTS('//item' PASSING BY REF S)",
];

/// The keys of T's rows `condition` holds of, in order.
fn keys(a: &Session, condition: &str) -> Vec<String> {
    rows(a, &format!("SELECT K FROM T WHERE {condition} ORDER BY K")).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §8.4, §6.18] RULES.md
/// D34, D86: of each XML index method, every condition gives the rows a
/// full read gives: a lookup's rows are fetched and checked, so an index
/// changes no result.
#[test]
fn each_index_gives_the_rows_of_a_full_read() {
    let db = database();
    let a = db.session("A");
    let full: Vec<Vec<String>> = CONDITIONS.iter().map(|c| keys(&a, c)).collect();
    assert!(full.iter().filter(|k| !k.is_empty()).count() > 15, "{full:?}");
    a.commit().unwrap();
    for method in METHODS {
        a.run(&format!("CREATE INDEX I ON T (X) USING {method}")).unwrap();
        for (condition, expected) in CONDITIONS.iter().zip(&full) {
            assert_eq!(&keys(&a, condition), expected, "{method}: {condition}");
        }
        a.commit().unwrap();
        a.run("DROP INDEX I").unwrap();
    }
}

/// Verifies: [EXTENSION] RULES.md D34, D86: EXPLAIN shows the XML index
/// read for a condition it finds the rows of, by its method, the path and
/// comparison its lookup selects by of the column; a full read for a
/// condition of no lookup, or of a column of XML(SEQUENCE).
#[test]
fn explain_shows_the_index_read() {
    let db = database();
    let a = db.session("A");
    a.run("CREATE INDEX I ON T (X) USING XML_PATH").unwrap();
    let shown = plan(&a, "SELECT K FROM T WHERE XMLEXISTS('//item[@price > 47]' PASSING BY REF X)");
    assert!(shown.contains("IndexScan A.T using I [xml_path]: T.X has //item/@price > 47"), "{shown}");
    assert!(shown.contains("Filter XMLEXISTS(T.X)"), "the conjunct still checks each row: {shown}");
    for (condition, lookup) in [
        ("XMLEXISTS('//item[@price > 47]' PASSING BY VALUE X)", "//item/@price > 47"),
        ("XMLEXISTS('//item[gift eq \"ink\"]' PASSING BY REF X)", "//item/gift"),
        ("XMLQUERY('//note[. = \"cap\"]' PASSING BY REF X RETURNING SEQUENCE NULL ON EMPTY) IS NOT NULL", "//note = 'cap'"),
        ("XMLQUERY('//note[. = \"cap\"]' PASSING BY REF X NULL ON EMPTY) IS NOT NULL", "//note = 'cap'"),
    ] {
        let shown = plan(&a, &format!("SELECT K FROM T WHERE {condition}"));
        assert!(shown.contains(&format!("T.X has {lookup}")), "{condition}: {shown}");
    }
    for condition in
        ["XMLEXISTS('//item[1]' PASSING BY REF X)", "XMLEXISTS('//item' PASSING BY REF S)", "XMLEXISTS('//item = 1' PASSING BY REF X)"]
    {
        let shown = plan(&a, &format!("SELECT K FROM T WHERE {condition}"));
        assert!(!shown.contains("IndexScan"), "{condition}: {shown}");
    }
}

/// The rows of `query`, without an index on T (X) and then with one of
/// each method, all alike; the plan of the last.
fn alike_with_each_index(a: &Session, query: &str) -> (Vec<String>, String) {
    let full = rows(a, query).unwrap();
    a.commit().unwrap();
    let mut last = String::new();
    for method in METHODS {
        a.run(&format!("CREATE INDEX I ON T (X) USING {method}")).unwrap();
        assert_eq!(rows(a, query).unwrap(), full, "{method}: {query}");
        last = plan(a, query);
        a.commit().unwrap();
        a.run("DROP INDEX I").unwrap();
    }
    (full, last)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §7.1 SR4h] RULES.md
/// D86: XMLTABLE joined to the rows of T has its row pattern an implied
/// filter of them, which an index finds the rows of, each checked by
/// XMLTABLE itself, not by a filter; a LEFT join, which keeps every row,
/// has none; the results are a full read's, with a filter of the
/// table's own rows too.
#[test]
fn xmltable_reads_the_rows_of_its_row_pattern() {
    let db = database();
    let a = db.session("A");
    let inner =
        "SELECT T.K, I.N FROM T, XMLTABLE('//item[@price > 46]' PASSING BY REF T.X COLUMNS N VARCHAR(10) PATH 'name') AS I ORDER BY T.K";
    let (found, _) = alike_with_each_index(&a, inner);
    assert_eq!(found.len(), 7, "{found:?}");
    let filtered = "SELECT T.K, I.N FROM T, XMLTABLE('//item[@price > 46]' PASSING BY REF T.X COLUMNS N VARCHAR(10) PATH 'name') AS I WHERE T.K > 100 ORDER BY T.K";
    assert_eq!(alike_with_each_index(&a, filtered).0.len(), 4);
    a.run("CREATE INDEX I ON T (X) USING XML_PATH").unwrap();
    let shown = plan(&a, inner);
    assert!(shown.contains("IndexScan A.T using I [xml_path]: T.X has //item/@price > 46"), "{shown}");
    assert!(!shown.contains("Filter"), "the implied filter checks no row: {shown}");
    let left = "SELECT T.K, I.N FROM T LEFT JOIN XMLTABLE('//note[. = \"pen\"]' PASSING BY REF T.X COLUMNS N VARCHAR(10) PATH '.') AS I ON TRUE ORDER BY T.K";
    assert!(!plan(&a, left).contains("IndexScan"), "{}", plan(&a, left));
    a.commit().unwrap();
    a.run("DROP INDEX I").unwrap();
    let (kept, _) = alike_with_each_index(&a, left);
    assert_eq!(kept.len(), ROWS);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.5 GR1c] RULES.md D86:
/// XMLEXISTS in a join's ON condition, of one operand's column, finds that
/// operand's rows by an index, as in WHERE, of an inner join and of the
/// inner operand of a LEFT join; the results are a full read's.
#[test]
fn on_conditions_read_by_an_index() {
    let db = database();
    let a = db.session("A");
    let queries = [
        "SELECT U.K, V.K FROM T AS U JOIN T AS V ON U.K = V.K AND XMLEXISTS('//note[. = \"pen\"]' PASSING BY REF V.X) ORDER BY U.K",
        "SELECT U.K, V.K FROM T AS U LEFT JOIN T AS V ON U.K = V.K AND XMLEXISTS('//note[. = \"ink\"]' PASSING BY REF V.X) ORDER BY U.K",
        "SELECT U.K, V.K FROM T AS U LEFT JOIN T AS V ON U.K = V.K AND XMLEXISTS('//item[@price >= 48]' PASSING BY REF V.X) ORDER BY U.K",
    ];
    for query in queries {
        alike_with_each_index(&a, query);
    }
    a.run("CREATE INDEX I ON T (X) USING XML_PATH").unwrap();
    for query in &queries[..2] {
        let shown = plan(&a, query);
        assert!(shown.contains("IndexScan A.T using I [xml_path]: V.X has"), "{query}: {shown}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQuery
/// §3.5.2] RULES.md D15, D86: a row whose value a numeric comparison
/// cannot cast fails the query, err:FORG0001, with an index as without,
/// as the lookup gives it; a conjunct that can raise an error keeps the
/// full read, and raises it.
#[test]
fn errors_are_raised_as_by_a_full_read() {
    let db = database();
    let a = db.session("A");
    a.run("CREATE INDEX I ON T (X) USING XML_VALUE").unwrap();
    let divided = "SELECT K FROM T WHERE XMLEXISTS('//item[@price > 47]' PASSING BY REF X) AND 100 / (K - 5) > 0";
    assert!(!plan(&a, divided).contains("IndexScan"), "{}", plan(&a, divided));
    assert_eq!(fetched(&a, divided).unwrap_err().sqlstate(), SqlState::DIVISION_BY_ZERO);
    a.run("INSERT INTO T VALUES (1000, XMLPARSE(DOCUMENT '<order><item price=\"n/a\"/></order>' STRIP WHITESPACE), NULL)").unwrap();
    let numeric = "SELECT K FROM T WHERE XMLEXISTS('//item[@price > 47]' PASSING BY REF X)";
    assert!(plan(&a, numeric).contains("IndexScan"), "{}", plan(&a, numeric));
    let error = fetched(&a, numeric).unwrap_err();
    assert_eq!(error.sqlstate(), SqlState::XQUERY_ERROR, "{error}");
    assert!(error.msg.contains("err:FORG0001"), "{error}");
    let strings =
        "SELECT K FROM T WHERE XMLEXISTS('/order[@id = \"77\"]' PASSING BY REF X) AND XMLEXISTS('//item[@price > 47]' PASSING BY REF X)";
    assert_eq!(fetched(&a, strings).unwrap_err().sqlstate(), SqlState::XQUERY_ERROR, "{}", plan(&a, strings));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §6.18 GR1a ii 2 B
/// III] RULES.md D15, D86: a column of XML(SEQUENCE) is read whole, as a
/// value of other than one node fails its query as its context item,
/// SQLSTATE 2200V, which a lookup would not give; and its index is not
/// read for a query of another column.
#[test]
fn a_sequence_column_is_read_whole() {
    let db = database();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (2000, NULL, XMLQUERY('<a/>, <b/>' RETURNING SEQUENCE BY REF EMPTY ON EMPTY))").unwrap();
    a.commit().unwrap();
    a.run("CREATE INDEX J ON T (S) USING XML_PATH").unwrap();
    let query = "SELECT K FROM T WHERE XMLEXISTS('//item[@price > 48]' PASSING BY REF S)";
    assert!(!plan(&a, query).contains("IndexScan"), "{}", plan(&a, query));
    assert_eq!(fetched(&a, query).unwrap_err().sqlstate(), SqlState::INVALID_XQUERY_CONTEXT_ITEM);
    let other = "SELECT K FROM T WHERE XMLEXISTS('//item[@price > 48]' PASSING BY REF X)";
    assert!(!plan(&a, other).contains("IndexScan"), "an index of another column: {}", plan(&a, other));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.6] RULES.md D86: a
/// prepared statement reads an index by its XMLEXISTS whatever values its
/// parameters are given, its rows a full read's of each.
#[test]
fn prepared_statements_read_by_an_index() {
    let db = database();
    let a = db.session("A");
    let query = "SELECT K FROM T WHERE XMLEXISTS('//item[@price > 40]' PASSING BY REF X) AND K > ? ORDER BY K";
    let fetch_all = |values: &[Value]| {
        let p = a.prepare(query).unwrap();
        let mut opened = a.open_prepared(&p, values).unwrap();
        let mut out = Vec::new();
        while let Some(row) = opened.fetch().unwrap() {
            out.push(row[0].to_string());
        }
        out
    };
    let bounds = [Value::Exact(turborel::value::Decimal::new(0, 0)), Value::Exact(turborel::value::Decimal::new(150, 0))];
    let full: Vec<Vec<String>> = bounds.iter().map(|v| fetch_all(std::slice::from_ref(v))).collect();
    a.commit().unwrap();
    a.run("CREATE INDEX I ON T (X) USING XML_PATH").unwrap();
    for (value, expected) in bounds.iter().zip(&full) {
        assert_eq!(&fetch_all(std::slice::from_ref(value)), expected);
    }
    assert!(full[0].len() > full[1].len() && !full[1].is_empty(), "{full:?}");
    assert_eq!(fetched(&a, "SELECT K FROM T WHERE XMLEXISTS('(' PASSING BY REF X)").unwrap_err().code, Code::Data);
}
