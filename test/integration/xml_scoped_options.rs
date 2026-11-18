//! SQL/XML's `<XML lexically scoped options>` [ISO/IEC 9075-14:2006 draft
//! §11.3]: XMLNAMESPACES and XMLBINARY of a `<with clause>` [§7.2], of
//! DELETE, INSERT, MERGE and UPDATE [§14.3 to §14.7], of a generation
//! expression, a CHECK and an assertion [§12.1, §12.2, §12.4], and of a
//! compound statement [§15.1], each in scope of what it is written in.

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;

use crate::common::{Scratch, code, fetched, os, rows, sorted};

/// X'0FB7' in base64 [Schema2 §3.2.16].
const BASE64: &str = "D7c=";

/// XMLELEMENT "p" of PHOTO, serialized.
const ELEMENT: &str = "XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"p\", PHOTO) AS VARCHAR(100))";

/// The DDL of schema A: table E of an INTEGER ID and a VARBINARY(4)
/// PHOTO, and table X of an INTEGER ID and a VARCHAR(100) V.
const SCHEMA: &str = "CREATE SCHEMA AUTHORIZATION A \
     CREATE TABLE E (ID INTEGER, PHOTO VARBINARY(4)) CREATE TABLE X (ID INTEGER, V VARCHAR(100))";

/// Schema A, E of the row (1, X'0FB7').
fn photos() -> Database {
    let db = Database::new();
    db.schema(SCHEMA).unwrap();
    let a = db.session("A");
    a.run("INSERT INTO E VALUES (1, X'0FB7')").unwrap();
    a.commit().unwrap();
    db
}

/// The one row of `sql`.
fn one(a: &Session, sql: &str) -> String {
    let found = rows(a, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"));
    assert_eq!(found.len(), 1, "{sql}");
    found.into_iter().next().unwrap()
}

/// The SQLCODE of the statement `sql`, which fails.
fn refused(a: &Session, sql: &str) -> Code {
    a.run(sql).map_err(|e| e.code).expect_err(sql)
}

/// `<p>` of X'0FB7' in `encoding`'s text.
fn p(encoding: &str) -> String {
    format!("<p>{encoding}</p>")
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §7.2 SR3, §11.3, §6.14 GR1, GR2,
/// §6.15 GR1, GR2, §6.6 SR1, SR2, GR3, GR4k iii, §9.5 GR8b ii] [XQueryFO
/// §17.1.7] XMLBINARY of a query: XMLELEMENT, XMLFOREST and XMLCAST
/// write a binary string in base64 of BASE64, USING or not, in hex of
/// HEX; outside any scope, in hex [§6.6 SR1b]; XMLCAST to XML(SEQUENCE)
/// gives an `xs:base64Binary`, and of XML to a binary string reads
/// base64, text that is not base64 an XQuery error.
#[test]
fn query_level_binary_encoding_7_2() {
    let db = photos();
    let a = db.session("A");
    assert_eq!(one(&a, &format!("WITH XMLBINARY USING BASE64 SELECT {ELEMENT} FROM E")), p(BASE64));
    assert_eq!(one(&a, &format!("WITH XMLBINARY BASE64 SELECT {ELEMENT} FROM E")), p(BASE64));
    assert_eq!(one(&a, &format!("WITH XMLBINARY USING HEX SELECT {ELEMENT} FROM E")), p("0FB7"));
    assert_eq!(one(&a, &format!("SELECT {ELEMENT} FROM E")), p("0FB7"));
    let forest = "XMLSERIALIZE(CONTENT XMLFOREST(PHOTO, ID) AS VARCHAR(100))";
    assert_eq!(one(&a, &format!("WITH XMLBINARY BASE64 SELECT {forest} FROM E")), "<PHOTO>D7c=</PHOTO><ID>1</ID>");
    let attribute = "XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"p\", XMLATTRIBUTES(PHOTO AS \"b\")) AS VARCHAR(100))";
    assert_eq!(one(&a, &format!("WITH XMLBINARY BASE64 SELECT {attribute} FROM E")), "<p b=\"D7c=\"/>");
    for target in ["XML(CONTENT)", "XML(SEQUENCE)"] {
        let cast = format!("XMLSERIALIZE(CONTENT XMLCAST(PHOTO AS {target}) AS VARCHAR(100))");
        assert_eq!(one(&a, &format!("WITH XMLBINARY BASE64 SELECT {cast} FROM E")), BASE64, "{target}");
        assert_eq!(one(&a, &format!("SELECT {cast} FROM E")), "0FB7", "{target}");
    }
    let array = "XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"p\", ARRAY[PHOTO]) AS VARCHAR(100))";
    assert_eq!(
        one(&a, &format!("WITH XMLBINARY BASE64 SELECT {array} FROM E")),
        "<p><element>D7c=</element></p>",
        "[§9.8 GR9j] an array's elements"
    );
    let back = "XMLCAST(XMLCAST(PHOTO AS XML(SEQUENCE)) AS VARBINARY(4))";
    assert_eq!(one(&a, &format!("WITH XMLBINARY BASE64 SELECT {back} FROM E")), "0FB7");
    let read = |text: &str| format!("XMLCAST(XMLPARSE(CONTENT '{text}' PRESERVE WHITESPACE) AS VARBINARY(4))");
    assert_eq!(one(&a, &format!("WITH XMLBINARY BASE64 SELECT {} FROM E", read(" D7 c= "))), "0FB7", "white space passed over");
    assert_eq!(one(&a, &format!("SELECT {} FROM E", read("0fb7"))), "0FB7");
    for (options, text) in [("BASE64", "0FB7?"), ("BASE64", "D7c"), ("BASE64", "D=7c"), ("HEX", BASE64)] {
        let sql = format!("WITH XMLBINARY {options} SELECT {} FROM E", read(text));
        assert_eq!(fetched(&a, &sql).unwrap_err().sqlstate().to_string(), "10000", "{sql}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §7.2 SR2, §10.12 SR2, §6.14 SR1]
/// XMLNAMESPACES of a query: a prefix and the default namespace it
/// declares name XMLELEMENT's and XMLFOREST's elements and attributes; an
/// XMLELEMENT's own declaration is more inner, NO DEFAULT too; XMLQUERY's
/// statically known namespaces and default element namespace are those
/// in scope [§10.20 SR2d]; a prefix no declaration in scope binds is
/// refused.
#[test]
fn query_level_namespaces_7_2() {
    let db = photos();
    let a = db.session("A");
    let x = |sql: &str| {
        one(&a, &format!("WITH XMLNAMESPACES('u' AS \"p\", DEFAULT 'd') SELECT XMLSERIALIZE(CONTENT {sql} AS VARCHAR(200)) FROM E"))
    };
    assert_eq!(x("XMLELEMENT(NAME \"p:a\", XMLELEMENT(NAME \"b\"))"), "<p:a xmlns:p=\"u\"><b xmlns=\"d\"/></p:a>");
    assert_eq!(x("XMLFOREST(ID AS \"p:i\")"), "<p:i xmlns:p=\"u\">1</p:i>");
    assert_eq!(x("XMLELEMENT(NAME \"a\", XMLATTRIBUTES(ID AS \"p:i\"))"), "<a xmlns=\"d\" xmlns:p=\"u\" p:i=\"1\"/>");
    let inner = "XMLELEMENT(NAME \"p:a\", XMLNAMESPACES('w' AS \"p\", NO DEFAULT), XMLELEMENT(NAME \"b\"))";
    assert_eq!(x(inner), "<p:a xmlns:p=\"w\"><b/></p:a>");
    let query = "XMLQUERY('<p:q><r/></p:q>' RETURNING CONTENT NULL ON EMPTY)";
    assert_eq!(x(query), "<p:q xmlns:p=\"u\"><r xmlns=\"d\"/></p:q>", "[§10.20 SR2d] XQuery's statically known namespaces");
    let unbound = "SELECT XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"p:a\") AS VARCHAR(20)) FROM E";
    assert_eq!(code(&a, unbound), Code::SyntaxRule, "[§10.12 SR2a ii 2] outside the query's scope");
    let predeclared = "WITH XMLNAMESPACES('u' AS \"xs\") SELECT XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"xs:a\") AS VARCHAR(40)) FROM E";
    assert_eq!(one(&a, predeclared), "<xs:a xmlns:xs=\"u\"/>", "[§10.12 SR2a ii 2 A] a declaration before a predeclared prefix");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §7.2 SR2, SR3, CR3] (X135) the
/// scope of a query's options is its whole `<query expression>`: its
/// subqueries, derived tables, `<with list element>`s and the operands
/// of its set operations; a `<with clause>` within is more inner, so its
/// options are searched first, and a nested XMLELEMENT's declaration
/// more inner still.
#[test]
fn scope_is_the_query_expression_7_2() {
    let db = photos();
    let a = db.session("A");
    let base64 = p(BASE64);
    let within = [
        format!("WITH XMLBINARY BASE64 SELECT (SELECT {ELEMENT} FROM E) FROM E"),
        format!("WITH XMLBINARY BASE64 SELECT S FROM (SELECT {ELEMENT} FROM E) AS D (S)"),
        format!("WITH XMLBINARY BASE64, T (S) AS (SELECT {ELEMENT} FROM E) SELECT S FROM T"),
        format!("WITH XMLBINARY BASE64 SELECT {ELEMENT} FROM E UNION SELECT {ELEMENT} FROM E"),
        format!("WITH XMLBINARY BASE64 SELECT S FROM (SELECT {ELEMENT} FROM E WHERE ID = (SELECT MAX(ID) FROM E)) AS D (S)"),
        format!("SELECT (WITH XMLBINARY BASE64 SELECT {ELEMENT} FROM E) FROM E"),
        format!("WITH XMLBINARY HEX SELECT (WITH XMLBINARY BASE64 SELECT {ELEMENT} FROM E) FROM E"),
        format!("WITH XMLBINARY HEX, T (S) AS (WITH XMLBINARY BASE64 SELECT {ELEMENT} FROM E) SELECT S FROM T"),
    ];
    for sql in within {
        assert_eq!(one(&a, &sql), base64, "{sql}");
    }
    let hex = [
        format!("WITH XMLBINARY BASE64 SELECT (WITH XMLBINARY HEX SELECT {ELEMENT} FROM E) FROM E"),
        format!("SELECT S FROM (SELECT {ELEMENT} FROM E) AS D (S), (WITH XMLBINARY BASE64 SELECT 1 FROM E) AS F (O)"),
    ];
    for sql in hex {
        assert_eq!(one(&a, &sql), p("0FB7"), "{sql}");
    }
    let nested = "WITH XMLNAMESPACES('u' AS \"p\") SELECT XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"a\", XMLNAMESPACES('v' AS \"p\"), \
         (WITH XMLNAMESPACES('w' AS \"p\") SELECT XMLELEMENT(NAME \"p:c\") FROM E), \
         (SELECT XMLELEMENT(NAME \"p:b\") FROM E)) AS VARCHAR(200)) FROM E";
    assert_eq!(one(&a, nested), "<a><p:c xmlns:p=\"w\"/><p:b xmlns:p=\"v\"/></a>");
    let query_name = "WITH XMLBINARY AS (SELECT ID FROM E) SELECT ID FROM XMLBINARY";
    assert_eq!(one(&a, query_name), "1", "[§5.1] a query name XMLBINARY stays one");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §7.2 SR1, SR4, §11.3 SR1 to SR7]
/// the rules of a `<with clause>`'s options, each broken one SQLCODE
/// -103: options, a list or both; a comma between both and none else; at
/// most one declaration and one encoding; the rules of a declaration.
/// Text that is no option, SQLCODE -104.
#[test]
fn option_rules_7_2_11_3() {
    let db = photos();
    let a = db.session("A");
    let queries = [
        "WITH SELECT ID FROM E",
        "WITH XMLBINARY HEX, SELECT ID FROM E",
        "WITH XMLBINARY HEX T AS (SELECT ID FROM E) SELECT ID FROM T",
        "WITH , T AS (SELECT ID FROM E) SELECT ID FROM T",
        "WITH XMLBINARY BASE64, XMLBINARY HEX SELECT ID FROM E",
        "WITH XMLNAMESPACES('u' AS \"p\"), XMLBINARY HEX, XMLNAMESPACES('v' AS \"q\") SELECT ID FROM E",
        "WITH XMLNAMESPACES(DEFAULT 'u', NO DEFAULT) SELECT ID FROM E",
        "WITH XMLNAMESPACES('u' AS \"1p\") SELECT ID FROM E",
        "WITH XMLNAMESPACES('u' AS \"p\", 'v' AS \"p\") SELECT ID FROM E",
        "WITH XMLNAMESPACES('u' AS \"xml\") SELECT ID FROM E",
        "WITH XMLNAMESPACES('u' AS \"xmlns\") SELECT ID FROM E",
        "WITH XMLNAMESPACES('http://www.w3.org/2000/xmlns/' AS \"p\") SELECT ID FROM E",
        "WITH XMLNAMESPACES(DEFAULT 'http://www.w3.org/XML/1998/namespace') SELECT ID FROM E",
        "WITH XMLNAMESPACES('' AS \"p\") SELECT ID FROM E",
    ];
    for sql in queries {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    let statements = [
        "DELETE WITH XMLBINARY HEX, XMLBINARY HEX FROM X",
        "CREATE TABLE C (B VARBINARY(4), CHECK (WITH XMLNAMESPACES('' AS \"p\") B IS NULL))",
    ];
    for sql in statements {
        assert_eq!(refused(&a, sql), Code::SyntaxRule, "{sql}");
    }
    for sql in ["WITH XMLBINARY USING OCTAL SELECT ID FROM E", "WITH XMLNAMESPACES('u' AS) SELECT ID FROM E"] {
        assert_eq!(code(&a, sql), Code::Syntax, "{sql}");
    }
    let syntax = [
        "DELETE WITH FROM X",
        "DELETE WITH XMLBINARY HEX FROM X WHERE CURRENT OF C",
        "MERGE WITH XMLBINARY HEX X USING E ON TRUE WHEN MATCHED THEN DELETE",
        "BEGIN DECLARE XMLBINARY HEX END",
    ];
    for sql in syntax {
        assert_eq!(refused(&a, sql), Code::Syntax, "{sql}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §14.3 to §14.7 SR1, SR2] WITH's
/// options of DELETE, INSERT, MERGE and UPDATE, searched and positioned,
/// in scope of the whole statement: its query, its SET values and its
/// conditions; prepared too; none beyond it.
#[test]
fn statement_options_14_3_to_14_7() {
    let db = photos();
    let a = db.session("A");
    a.run(&format!("INSERT WITH XMLBINARY BASE64 INTO X SELECT ID, {ELEMENT} FROM E")).unwrap();
    a.run("INSERT WITH XMLNAMESPACES(DEFAULT 'd') INTO X VALUES (2, XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"a\") AS VARCHAR(30)))").unwrap();
    a.run(&format!("INSERT INTO X SELECT 3, {ELEMENT} FROM E")).unwrap();
    assert_eq!(sorted(&a, "SELECT ID, V FROM X"), ["1,<p>D7c=</p>", "2,<a xmlns=\"d\"/>", "3,<p>0FB7</p>"]);
    a.run(&format!("UPDATE WITH XMLBINARY BASE64 X SET V = (SELECT {ELEMENT} FROM E) WHERE ID = 3")).unwrap();
    assert_eq!(one(&a, "SELECT V FROM X WHERE ID = 3"), p(BASE64));
    a.run(&format!("DELETE WITH XMLBINARY HEX FROM X WHERE V = (SELECT {ELEMENT} FROM E)")).unwrap();
    assert_eq!(rows(&a, "SELECT ID FROM X WHERE ID <> 2").unwrap().len(), 2, "no row's V is in hex");
    a.run(&format!("DELETE WITH XMLBINARY BASE64 FROM X WHERE V = (SELECT {ELEMENT} FROM E)")).unwrap();
    assert_eq!(sorted(&a, "SELECT ID FROM X"), ["2"]);
    let merge = format!("MERGE WITH XMLBINARY BASE64 INTO X USING E ON X.ID = E.ID WHEN NOT MATCHED THEN INSERT VALUES (E.ID, {ELEMENT})");
    a.run(&merge).unwrap();
    a.run(&format!("MERGE WITH XMLBINARY HEX INTO X USING E ON X.ID = E.ID WHEN MATCHED THEN UPDATE SET V = {ELEMENT} || V")).unwrap();
    assert_eq!(one(&a, "SELECT V FROM X WHERE ID = 1"), "<p>0FB7</p><p>D7c=</p>");
    let positioned = format!(
        "BEGIN DECLARE I INTEGER; DECLARE C CURSOR FOR SELECT ID FROM X WHERE ID = 1 FOR UPDATE; OPEN C; FETCH C INTO I; \
         UPDATE WITH XMLBINARY BASE64 X SET V = (SELECT {ELEMENT} FROM E) WHERE CURRENT OF C; CLOSE C; END"
    );
    a.run(&positioned).unwrap();
    assert_eq!(one(&a, "SELECT V FROM X WHERE ID = 1"), p(BASE64), "[§14.6] positioned");
    let prepared = a.prepare(&format!("UPDATE WITH XMLBINARY BASE64 X SET V = (SELECT {ELEMENT} || ? FROM E) WHERE ID = 2")).unwrap();
    a.execute_prepared(&prepared, &[turborel::value::Value::Char("!".to_owned())]).unwrap();
    assert_eq!(one(&a, "SELECT V FROM X WHERE ID = 2"), "<p>D7c=</p>!");
    a.run(&format!("INSERT INTO X SELECT 4, {ELEMENT} FROM E")).unwrap();
    assert_eq!(one(&a, "SELECT V FROM X WHERE ID = 4"), p("0FB7"), "none after the statement");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §15.1 SR1, SR2] DECLARE's
/// options of a compound statement in scope of what it holds: its
/// statements and values, a compound statement within, whose own are more
/// inner, and a statement's own WITH more inner still; of a procedure's
/// body and a trigger's action too.
#[test]
fn compound_statement_options_15_1() {
    let db = photos();
    let a = db.session("A");
    a.run(&format!(
        "BEGIN DECLARE XMLBINARY BASE64; DECLARE S VARCHAR(100); SET S = (SELECT {ELEMENT} FROM E); \
         INSERT INTO X VALUES (1, S); INSERT INTO X SELECT 2, {ELEMENT} FROM E; \
         BEGIN DECLARE XMLBINARY HEX; INSERT INTO X SELECT 3, {ELEMENT} FROM E; END; \
         INSERT WITH XMLBINARY HEX INTO X SELECT 4, {ELEMENT} FROM E; \
         IF (SELECT {ELEMENT} FROM E) = '<p>D7c=</p>' THEN INSERT INTO X VALUES (5, 'if'); END IF; END"
    ))
    .unwrap();
    assert_eq!(sorted(&a, "SELECT ID, V FROM X"), ["1,<p>D7c=</p>", "2,<p>D7c=</p>", "3,<p>0FB7</p>", "4,<p>0FB7</p>", "5,if"]);
    a.run("DELETE FROM X").unwrap();
    a.run(
        "CREATE PROCEDURE PUT (IN K INTEGER) MODIFIES SQL DATA BEGIN DECLARE XMLNAMESPACES('u' AS \"q\"); \
         INSERT INTO X VALUES (K, XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"q:k\") AS VARCHAR(40))); END",
    )
    .unwrap();
    a.run("CALL PUT(6)").unwrap();
    a.run(
        "CREATE TRIGGER LOGGED AFTER INSERT ON E REFERENCING NEW ROW AS N FOR EACH ROW \
         BEGIN ATOMIC DECLARE XMLBINARY BASE64; INSERT INTO X VALUES (N.ID, XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"t\", N.PHOTO) AS VARCHAR(40))); END",
    )
    .unwrap();
    a.run("INSERT INTO E VALUES (7, X'FF')").unwrap();
    assert_eq!(sorted(&a, "SELECT ID, V FROM X"), ["6,<q:k xmlns:q=\"u\"/>", "7,<t>/w==</t>"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §12.1 SR1, SR2, §12.2 SR1, SR2,
/// §12.4 SR1, SR2, §7.2 SR3] [ISO/IEC 9075-11:2006 draft §6.14
/// Description 2a] [DESIGN] RULES.md D86: a generation expression's, a
/// CHECK's and an assertion's options, and a view's and a routine's, are
/// kept with the text the catalog keeps, so they bind the same when it
/// is read again, after the database is opened again too, as are a
/// procedure's DECLARE and a trigger's statement's WITH; CHECK_CLAUSE
/// shows the search condition alone. A routine's body and a view's query
/// see none of the options of the statement invoking or reading them.
#[test]
fn definitions_keep_their_options_12_1_12_2_12_4() {
    let scratch = Scratch::new("xml-options");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema(SCHEMA).unwrap();
    let a = db.session("A");
    let b = "XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"b\", B) AS VARCHAR(20))";
    for ddl in [
        format!("CREATE TABLE C (B VARBINARY(4), CHECK (WITH XMLBINARY BASE64 {b} = '<b>D7c=</b>'))"),
        format!("CREATE TABLE G (B VARBINARY(4), S VARCHAR(20) GENERATED ALWAYS AS (WITH XMLBINARY USING BASE64 {b}))"),
        "CREATE ASSERTION N CHECK (WITH XMLNAMESPACES('u' AS \"p\") \
         NOT EXISTS (SELECT * FROM X WHERE V = XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"p:a\") AS VARCHAR(100))))"
            .to_owned(),
        format!("CREATE VIEW V (S) AS WITH XMLBINARY BASE64 SELECT {ELEMENT} FROM E"),
        format!("CREATE VIEW W (S) AS SELECT {ELEMENT} FROM E"),
        "CREATE FUNCTION HEXED (B VARBINARY(4)) RETURNS VARCHAR(20) CONTAINS SQL RETURN XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"f\", B) AS VARCHAR(20))".to_owned(),
        "CREATE DOMAIN PIC AS VARBINARY(4) CHECK (WITH XMLBINARY BASE64 \
         XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"b\", VALUE) AS VARCHAR(20)) <> '<b>/w==</b>')"
            .to_owned(),
        "CREATE TABLE D (B PIC)".to_owned(),
        format!("CREATE PROCEDURE PUT (IN K INTEGER) MODIFIES SQL DATA BEGIN DECLARE XMLBINARY BASE64; INSERT INTO X SELECT K, {ELEMENT} FROM E WHERE ID = 1; END"),
        "CREATE TRIGGER LOGGED AFTER INSERT ON E REFERENCING NEW ROW AS N FOR EACH ROW \
         INSERT WITH XMLBINARY BASE64 INTO X VALUES (N.ID, XMLSERIALIZE(CONTENT XMLELEMENT(NAME \"t\", N.PHOTO) AS VARCHAR(40)))"
            .to_owned(),
    ] {
        a.run(&ddl).unwrap_or_else(|e| panic!("{ddl}: {e:?}"));
    }
    a.run("INSERT INTO E VALUES (1, X'0FB7')").unwrap();
    a.commit().unwrap();
    let kept = |db: &Database| {
        let a = db.session("A");
        a.run("INSERT INTO C VALUES (X'0FB7')").unwrap();
        assert_eq!(refused(&a, "INSERT INTO C VALUES (X'0FB8')"), Code::Constraint, "[§12.2] base64 of the CHECK's");
        a.run("INSERT INTO D VALUES (X'0FB7')").unwrap();
        assert_eq!(refused(&a, "INSERT INTO D VALUES (X'FF')"), Code::Constraint, "[§12.2] a domain's CHECK, on its column");
        a.run("INSERT INTO G (B) VALUES (X'0FB7')").unwrap();
        assert_eq!(one(&a, "SELECT S FROM G"), "<b>D7c=</b>");
        assert_eq!(refused(&a, "INSERT INTO X VALUES (9, '<p:a xmlns:p=\"u\"/>')"), Code::Constraint, "[§12.4] the assertion's prefix");
        assert_eq!(one(&a, "SELECT S FROM V"), p(BASE64));
        assert_eq!(one(&a, "WITH XMLBINARY BASE64 SELECT S FROM W"), p("0FB7"), "none of the query reading it");
        assert_eq!(one(&a, "WITH XMLBINARY BASE64 SELECT HEXED(PHOTO) FROM E"), "<f>0FB7</f>", "none of the query invoking it");
        a.run("CALL PUT(5)").unwrap();
        a.run("INSERT INTO E VALUES (8, X'FF')").unwrap();
        assert_eq!(sorted(&a, "SELECT ID, V FROM X WHERE ID IN (5, 8)"), ["5,<p>D7c=</p>", "8,<t>/w==</t>"], "a routine's, a trigger's");
        let clauses = sorted(&a, "SELECT CHECK_CLAUSE FROM INFORMATION_SCHEMA.CHECK_CONSTRAINTS WHERE CHECK_CLAUSE LIKE '%XMLSERIALIZE%'");
        assert!(clauses.iter().all(|c| !c.starts_with("WITH")), "{clauses:?}");
        assert_eq!(clauses.len(), 3, "the table's, the assertion's and the domain's: {clauses:?}");
        a.rollback().unwrap();
    };
    kept(&db);
    drop(a);
    db.close().unwrap();
    drop(db);
    kept(&open());
}
