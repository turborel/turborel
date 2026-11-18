//! XMLTABLE [ISO/IEC 9075-14:2006 draft §7.1]: the rows of an XQuery row
//! pattern, each column of its own pattern, as a table primary of FROM.

use turborel::db::Database;
use turborel::error::{Code, SqlState};

use crate::common::{fetched, rows};

/// Schema A: table T of an INTEGER K and an XML X, two orders of items.
fn orders() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML)").unwrap();
    let a = db.session("A");
    a.run(
        "INSERT INTO T VALUES \
         (1, XMLPARSE(DOCUMENT '<order><item><name>pen</name><price>10</price></item><item><name>ink</name></item></order>' STRIP WHITESPACE)), \
         (2, XMLPARSE(DOCUMENT '<order><item><name>cap</name><price>5.5</price></item></order>' STRIP WHITESPACE))",
    )
    .unwrap();
    a.commit().unwrap();
    db
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §7.1 SR4, GR1] XMLTABLE of each
/// row of T, lateral: a row for each item of its row pattern, each column
/// its pattern's value cast to its type, the column's name its pattern if
/// it has no PATH, its default of an empty value, else the null value;
/// FOR ORDINALITY the item's place.
#[test]
fn rows_of_a_row_pattern_7_1() {
    let a = orders().session("A");
    let q = "SELECT T.K, I.N, I.\"name\", I.PRICE FROM T, XMLTABLE('/order/item' PASSING BY REF T.X \
             COLUMNS N FOR ORDINALITY, \"name\" VARCHAR(10), PRICE DECIMAL(5,2) DEFAULT 0 PATH 'price') AS I ORDER BY T.K, I.N";
    assert_eq!(rows(&a, q).unwrap(), ["1,1,pen,10.00", "1,2,ink,0.00", "2,1,cap,5.50"]);
    let q = "SELECT I.V FROM XMLTABLE('1 to 3' COLUMNS V INTEGER PATH '.') AS I";
    assert_eq!(rows(&a, q).unwrap(), ["1", "2", "3"]);
    let q = "SELECT * FROM T, XMLTABLE('//none' PASSING BY REF X COLUMNS V INTEGER PATH '.') AS I";
    assert_eq!(rows(&a, q).unwrap().len(), 0);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §7.1 SR4e ii 2, SR4c] a column
/// of XML(SEQUENCE) of its passing mechanism or the argument list's,
/// another XML type its value as a document copied; a derived column
/// list naming the columns; the arguments passed as variables.
#[test]
fn xml_columns_and_variables_7_1() {
    let a = orders().session("A");
    let q = "SELECT XMLSERIALIZE(CONTENT J.ITEM AS VARCHAR(80)), J.C FROM T, XMLTABLE('/order/item[price > $min]' PASSING BY REF X, 6 AS \"min\" \
             COLUMNS ITEM XML(SEQUENCE) PATH '.', DOC XML(CONTENT) PATH 'name') AS J (ITEM, C)";
    assert_eq!(rows(&a, q).unwrap(), ["<item><name>pen</name><price>10</price></item>,<name>pen</name>"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §7.1 SR4a, §11.3] XMLNAMESPACES
/// of XMLTABLE bind the prefixes of its row and column patterns.
#[test]
fn namespaces_7_1() {
    let a = orders().session("A");
    let q = "SELECT I.V FROM XMLTABLE(XMLNAMESPACES('urn:p' AS \"p\"), 'declare default element namespace \"urn:p\"; <r xmlns=\"urn:p\"><v>1</v><v>2</v></r>/v' \
             COLUMNS V INTEGER PATH '.') AS I";
    assert_eq!(rows(&a, q).unwrap(), ["1", "2"]);
    let q = "SELECT I.V FROM XMLTABLE(XMLNAMESPACES('urn:p' AS \"p\"), '<p:r xmlns:p=\"urn:p\"><p:v>3</p:v></p:r>' COLUMNS V INTEGER PATH 'p:v') AS I";
    assert_eq!(rows(&a, q).unwrap(), ["3"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §7.1 SR4d, SR4e ii 2] the syntax
/// rules of XMLTABLE, each SQLCODE -103: two ordinality columns; a
/// passing mechanism of a column not XML(SEQUENCE); a column of
/// XML(SEQUENCE) of no mechanism and no argument list; and the errors of
/// its evaluation: an attribute made a document SQLSTATE 10000.
#[test]
fn rules_7_1() {
    let a = orders().session("A");
    for sql in [
        "SELECT * FROM XMLTABLE('1' COLUMNS A FOR ORDINALITY, B FOR ORDINALITY) AS I",
        "SELECT * FROM XMLTABLE('1' COLUMNS A INTEGER BY REF PATH '.') AS I",
        "SELECT * FROM XMLTABLE('1' COLUMNS A XML(SEQUENCE) PATH '.') AS I",
    ] {
        assert_eq!(fetched(&a, sql).expect_err(sql).code, Code::SyntaxRule, "{sql}");
    }
    let attribute = "SELECT * FROM XMLTABLE('<a b=\"1\"/>' COLUMNS B INTEGER PATH '@b') AS I";
    assert_eq!(fetched(&a, attribute).expect_err(attribute).sqlstate(), SqlState::XQUERY_ERROR);
    let data = "SELECT * FROM XMLTABLE('<a b=\"1\"/>' COLUMNS B INTEGER PATH 'data(@b)') AS I";
    assert_eq!(rows(&a, data).unwrap(), ["1"]);
}
