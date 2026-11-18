//! XML within other types [ISO/IEC 9075-14:2006 draft §6.1, §6.2, §12.5,
//! §12.6, §12.7, Features X011 to X015, X120]: arrays and multisets of
//! XML, distinct types sourced on XML, attributes of structured types and
//! fields of rows of XML, and XML parameters and results of routines; each
//! stored, kept through a crash, assigned and cast part by part, and
//! refused wherever an XML value would be compared [§10.8 to §10.11].

use turborel::db::{Database, OpenOptions, Opened, Session};
use turborel::error::{Code, SqlState};

use crate::common::{Scratch, after_crash, code, dynamic_host, fetched, rows, run_with as run, set_text as text, sorted};

/// The schema A's types and tables: a distinct type DX of XML(DOCUMENT),
/// a structured type ST of an XML attribute, and table T of an array, a
/// multiset, a row, a DX and an ST.
const TABLES: &str = "CREATE SCHEMA AUTHORIZATION A \
     CREATE TYPE DX AS XML(DOCUMENT(ANY)) FINAL \
     CREATE TYPE ST AS (X XML(CONTENT(ANY)), N INTEGER) NOT FINAL \
     CREATE TABLE T (K INTEGER, AX XML(CONTENT(ANY)) ARRAY[3], MX XML(CONTENT(ANY)) MULTISET, \
       R ROW(F XML(CONTENT(ANY)), N INTEGER), D DX, S ST)";

/// XMLPARSE of `text` as CONTENT.
fn parsed(text: &str) -> String {
    format!("XMLPARSE(CONTENT '{text}' PRESERVE WHITESPACE)")
}

/// A database of `TABLES`, T holding row 1 of `<a/>`s and `<b/>`s, and
/// row 2 of nulls, committed.
fn database() -> Database {
    let db = Database::new();
    db.schema(TABLES).unwrap();
    let a = db.session("A");
    let (x, y) = (parsed("<a/>"), parsed("<b/>"));
    a.run(&format!(
        "INSERT INTO T VALUES (1, ARRAY[{x}, CAST(NULL AS XML(CONTENT(ANY))), {y}], MULTISET[{x}, {y}], ROW({x}, 1), CAST({x} AS DX), NEW ST().X({y}).N(2))"
    ))
    .unwrap();
    a.run("INSERT INTO T (K) VALUES (2)").unwrap();
    a.commit().unwrap();
    db
}

/// Each row of `sql`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// The SQLSTATE `sql`, a query, fails with.
fn state(s: &Session, sql: &str) -> SqlState {
    fetched(s, sql).expect_err(sql).sqlstate()
}

/// The SQLCODE `sql`, a query or a statement, fails with.
fn refused(s: &Session, sql: &str) -> Code {
    if sql.starts_with("SELECT") { code(s, sql) } else { s.run(sql).expect_err(sql).code }
}

/// The SQLSTATE `sql`, a statement, fails with.
fn refused_with(s: &Session, sql: &str) -> SqlState {
    s.run(sql).expect_err(sql).sqlstate()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.1 CR2, CR3] [ISO/IEC
/// 9075-2:2023 §6.25, §6.31, §6.36, §6.38, §7.6, §10.9] (Features X011,
/// X012) an array and a multiset of XML are stored and read: an element
/// by its place, CARDINALITY, UNNEST, ELEMENT, concatenation, TRIM_ARRAY,
/// MULTISET UNION ALL, ARRAY_AGG, COLLECT and FUSION; an element
/// assigned by UPDATE.
#[test]
fn arrays_and_multisets_of_xml_x011_x012() {
    let db = database();
    let a = db.session("A");
    let shown = |e: &str| format!("XMLSERIALIZE(CONTENT {e} AS VARCHAR(40))");
    assert_eq!(
        query(&a, &format!("SELECT {}, {}, CARDINALITY(AX), CARDINALITY(MX) FROM T WHERE K = 1", shown("AX[1]"), shown("AX[3]"))),
        ["<a/>,<b/>,3,2"]
    );
    assert_eq!(query(&a, "SELECT CARDINALITY(AX), CARDINALITY(MX) FROM T WHERE K = 2"), ["NULL,NULL"]);
    let unnested = format!("SELECT {} FROM T, UNNEST(T.AX) AS U(E) WHERE K = 1", shown("E"));
    assert_eq!(query(&a, &unnested), ["<a/>", "NULL", "<b/>"]);
    let elements = format!("SELECT {} FROM T, UNNEST(T.MX) AS U(E) WHERE K = 1", shown("E"));
    assert_eq!(sorted(&a, &elements), ["<a/>", "<b/>"]);
    assert_eq!(query(&a, &format!("SELECT {} FROM T WHERE K = 1", shown("ELEMENT(MULTISET[AX[1]])"))), ["<a/>"]);
    let built = "SELECT CARDINALITY(AX || AX), CARDINALITY(TRIM_ARRAY(AX, 1)), CARDINALITY(MX MULTISET UNION ALL MX) FROM T WHERE K = 1";
    assert_eq!(query(&a, built), ["6,2,4"]);
    let aggregates = "SELECT CARDINALITY(ARRAY_AGG(AX[1])), CARDINALITY(COLLECT(AX[1])), CARDINALITY(FUSION(MX)) FROM T";
    assert_eq!(query(&a, aggregates), ["2,2,2"]);
    a.run(&format!("UPDATE T SET AX[2] = {} WHERE K = 1", parsed("<c/>"))).unwrap();
    assert_eq!(query(&a, &format!("SELECT {} FROM T WHERE K = 1", shown("AX[2]"))), ["<c/>"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.2 SR1b, GR1a] [ISO/IEC
/// 9075-2:2023 §9.2 GR2, §6.13 GR] an array, a multiset or a row of XML is
/// assigned and cast part by part: to a DOCUMENT element or field, a
/// document alone, else SQLSTATE 2200L; to an UNTYPED one, a value of an
/// UNTYPED type alone, else SQLCODE -103; CAST to an array, a multiset or
/// a row of other XML types, its parts as CAST takes them: to XML types
/// alone, BY REF, so to an UNTYPED one of an UNTYPED one [§6.5 SR3, SR4a],
/// else SQLCODE -103.
#[test]
fn assigned_and_cast_part_by_part_10_2() {
    let db = database();
    let a = db.session("A");
    a.run("CREATE TABLE V (A XML(DOCUMENT(ANY)) ARRAY[2], U XML(CONTENT(UNTYPED)) ARRAY[2], R ROW(F XML(DOCUMENT(ANY))))").unwrap();
    assert_eq!(refused_with(&a, &format!("INSERT INTO V (A) VALUES (ARRAY[{}])", parsed("text"))), SqlState::NOT_AN_XML_DOCUMENT);
    assert_eq!(refused_with(&a, &format!("INSERT INTO V (R) VALUES (ROW({}))", parsed("text"))), SqlState::NOT_AN_XML_DOCUMENT);
    assert_eq!(refused(&a, "INSERT INTO V (U) SELECT AX FROM T"), Code::SyntaxRule, "ANY to UNTYPED");
    a.run(&format!("INSERT INTO V (A, U) VALUES (ARRAY[{}], ARRAY[{}])", parsed("<d/>"), parsed("text"))).unwrap();
    let casts = "SELECT CARDINALITY(CAST(AX AS XML(SEQUENCE) MULTISET)), \
                 XMLSERIALIZE(CONTENT CAST(R AS ROW(G XML(DOCUMENT(ANY)), M BIGINT)).G AS VARCHAR(9)) FROM T WHERE K = 1";
    assert_eq!(query(&a, casts), ["3,<a/>"]);
    assert_eq!(
        state(&a, &format!("SELECT CAST(ARRAY[{}] AS XML(DOCUMENT) ARRAY[1]) FROM T", parsed("text"))),
        SqlState::NOT_AN_XML_DOCUMENT
    );
    assert_eq!(code(&a, "SELECT CAST(AX AS INTEGER ARRAY[3]) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT CAST(AX AS XML(CONTENT(UNTYPED)) ARRAY[3]) FROM T"), Code::SyntaxRule, "BY REF, ANY to UNTYPED");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.4, §10.8 SR1, §10.9 SR1,
/// §10.10 SR1, §10.11 SR1] [ISO/IEC 9075-2:2023 §9.11 to §9.14] no
/// equality, grouping, multiset element grouping or ordering operation
/// takes an array, a multiset, a row or a distinct type of XML, nor a
/// structured type of no ordering: comparisons, IN, GROUP BY, ORDER BY,
/// DISTINCT, UNION, MEMBER, SUBMULTISET, IS A SET, SET, MULTISET UNION
/// DISTINCT, INTERSECT and EXCEPT, INTERSECTION, keys and indexes, each
/// SQLCODE -103.
#[test]
fn refused_where_compared_10_8() {
    let db = database();
    let a = db.session("A");
    for sql in [
        "SELECT K FROM T WHERE AX = AX",
        "SELECT K FROM T WHERE R <> R",
        "SELECT K FROM T WHERE D = D",
        "SELECT K FROM T WHERE S = S",
        "SELECT K FROM T WHERE D IN (SELECT D FROM T)",
        "SELECT K FROM T WHERE AX IS DISTINCT FROM AX",
        "SELECT AX FROM T GROUP BY AX",
        "SELECT K FROM T ORDER BY R",
        "SELECT DISTINCT D FROM T",
        "SELECT MX FROM T UNION SELECT MX FROM T",
        "SELECT K FROM T WHERE AX[1] MEMBER OF MX",
        "SELECT K FROM T WHERE MX SUBMULTISET OF MX",
        "SELECT K FROM T WHERE MX IS A SET",
        "SELECT SET(MX) FROM T",
        "SELECT MX MULTISET UNION DISTINCT MX FROM T",
        "SELECT MX MULTISET INTERSECT MX FROM T",
        "SELECT MX MULTISET EXCEPT MX FROM T",
        "SELECT INTERSECTION(MX) FROM T",
        "CREATE TABLE W (A XML ARRAY[2] PRIMARY KEY)",
        "CREATE TABLE W (D DX UNIQUE)",
        "CREATE TABLE W (R ROW(F XML), CONSTRAINT U UNIQUE (R))",
    ] {
        assert_eq!(refused(&a, sql), Code::SyntaxRule, "{sql}");
    }
    a.commit().unwrap();
    assert_eq!(refused(&a, "CREATE INDEX I ON T (AX)"), Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §12.5 CR1, §10.4 SR1, §10.5 SR1,
/// §12.7 SR8b] [ISO/IEC 9075-2:2023 §11.51 GR2, §6.13 SR12] (Feature
/// X013) a distinct type of an XML type: its cast function to its source
/// named XML; CAST to it and from it, its value checked as its source's,
/// a non-document 2200L; an XML value assigned to it, of a type whose
/// precedence list holds XML; a parameter and a result of it, passed BY
/// REF; an array of it; of no ordering, ORDERING_FORM NONE, so not
/// compared.
#[test]
fn distinct_types_of_xml_x013() {
    let db = database();
    let a = db.session("A");
    let cast = "SELECT XMLSERIALIZE(CONTENT CAST(D AS XML(DOCUMENT(ANY))) AS VARCHAR(9)) FROM T WHERE K = 1";
    assert_eq!(query(&a, cast), ["<a/>"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE XML(D) IS DOCUMENT"), ["1"]);
    assert_eq!(state(&a, &format!("SELECT CAST({} AS DX) FROM T", parsed("text"))), SqlState::NOT_AN_XML_DOCUMENT);
    a.run(&format!("UPDATE T SET D = {} WHERE K = 2", parsed("<e/>"))).unwrap();
    a.run("CREATE FUNCTION F (P DX) RETURNS DX CONTAINS SQL RETURN P").unwrap();
    a.run("CREATE FUNCTION G (P XML(CONTENT(ANY))) RETURNS INTEGER CONTAINS SQL RETURN 7").unwrap();
    let invoked =
        "SELECT XMLSERIALIZE(CONTENT XML(F(D)) AS VARCHAR(9)), G(XMLPARSE(DOCUMENT '<z/>' PRESERVE WHITESPACE)) FROM T ORDER BY K";
    assert_eq!(query(&a, invoked), ["<a/>,7", "<e/>,7"]);
    let passing = "SELECT P.PARAMETER_NAME, P.XML_PASSING_MECHANISM, R.XML_RETURN_MECHANISM FROM INFORMATION_SCHEMA.PARAMETERS AS P \
                   JOIN INFORMATION_SCHEMA.ROUTINES AS R USING (SPECIFIC_CATALOG, SPECIFIC_SCHEMA, SPECIFIC_NAME) WHERE R.ROUTINE_NAME = 'F'";
    assert_eq!(query(&a, passing), ["P,BY REF,BY REF"]);
    a.run("CREATE TABLE W (A DX ARRAY[2])").unwrap();
    a.run("INSERT INTO W SELECT ARRAY[D] FROM T").unwrap();
    assert_eq!(query(&a, "SELECT CARDINALITY(A) FROM W"), ["1", "1"]);
    let ordering = "SELECT ORDERING_FORM, ORDERING_CATEGORY FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES WHERE USER_DEFINED_TYPE_NAME = 'DX'";
    assert_eq!(query(&a, ordering), ["NONE,NULL"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §17.1 GR1, Table 12] [ISO/IEC
/// 9075-2:2023 §20.1 SR4, §20.9 GR7] an array, a multiset and a row of
/// XML are described WITH NESTING: each its code, 50, 55 and 19, at LEVEL
/// 0, and its XML element or field after it, 137 at LEVEL 1.
#[test]
fn described_with_nesting_17_1() {
    let db = database();
    let a = db.session("A");
    let mut p = dynamic_host();
    text(&mut p, "SELECT AX, MX, R FROM T");
    run(&a, "PREPARE Q FROM TEXT", &mut p).unwrap();
    run(&a, "ALLOCATE DESCRIPTOR 'D' WITH MAX 10", &mut p).unwrap();
    run(&a, "DESCRIBE OUTPUT Q USING SQL DESCRIPTOR 'D' WITH NESTING", &mut p).unwrap();
    let mut items = Vec::new();
    for n in 1..=6 {
        a.execute(&format!("GET DESCRIPTOR 'D' VALUE {n} N = LEVEL, VAL = TYPE"), &mut p).unwrap();
        items.push(format!("{}:{}", p.value("N"), p.value("VAL")));
    }
    assert_eq!(items, ["0:50", "1:137", "0:55", "1:137", "0:19", "1:137"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §12.6 CR1, §6.2 CR1] [ISO/IEC
/// 9075-2:2023 §6.21, §6.6, §11.42] (Features X014, X015) an attribute of
/// a structured type and a field of a row of XML: observed, set by a
/// mutator and by a field's assignment, and a typed table's column of it.
#[test]
fn attributes_and_fields_of_xml_x014_x015() {
    let db = database();
    let a = db.session("A");
    let shown = |e: &str| format!("XMLSERIALIZE(CONTENT {e} AS VARCHAR(20))");
    assert_eq!(query(&a, &format!("SELECT {}, T.S.N, {}, T.R.N FROM T WHERE K = 1", shown("T.S.X"), shown("T.R.F"))), ["<b/>,2,<a/>,1"]);
    a.run(&format!("UPDATE T SET S = S.X({}), R = ROW({}, 3) WHERE K = 1", parsed("<s/>"), parsed("<r/>"))).unwrap();
    assert_eq!(query(&a, &format!("SELECT {}, {} FROM T WHERE K = 1", shown("T.S.X"), shown("T.R.F"))), ["<s/>,<r/>"]);
    a.run("CREATE TABLE TT OF ST (REF IS ID SYSTEM GENERATED)").unwrap();
    a.run(&format!("INSERT INTO TT (X, N) VALUES ({}, 5)", parsed("<t/>"))).unwrap();
    assert_eq!(query(&a, &format!("SELECT {}, N FROM TT", shown("X"))), ["<t/>,5"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §12.7 CR1] [ISO/IEC 9075-2:2023
/// §11.60, §10.4] (Feature X120) an SQL-invoked function of an array of
/// XML parameter and a row of XML result, and a procedure of an output
/// parameter of an array of XML, invoked and called.
#[test]
fn routines_of_xml_composites_x120() {
    let db = database();
    let a = db.session("A");
    a.run("CREATE FUNCTION FIRST_OF (P XML(CONTENT(ANY)) ARRAY[3]) RETURNS ROW(E XML(CONTENT(ANY)), C INTEGER) CONTAINS SQL RETURN ROW(P[1], CARDINALITY(P))")
        .unwrap();
    let invoked = "SELECT XMLSERIALIZE(CONTENT FIRST_OF(AX).E AS VARCHAR(9)), FIRST_OF(AX).C FROM T WHERE K = 1";
    assert_eq!(query(&a, invoked), ["<a/>,3"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.3] [DESIGN] RULES.md D79,
/// D106: an array, a multiset, a row, a distinct type and a structured
/// type of XML values of many chunks, copied from another table's column,
/// are whole after their source row is deleted and after a crash.
#[test]
fn kept_through_a_crash() {
    let scratch = Scratch::new("xml-composite");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        let opened = db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        (db, opened)
    };
    let (crashed, _) = open();
    crashed.schema(TABLES).unwrap();
    let a = crashed.session("A");
    let long = format!("<list>{}</list>", "<item>value</item>".repeat(40_000));
    a.run("CREATE TABLE SOURCE (X XML(CONTENT(ANY)))").unwrap();
    a.run(&format!("INSERT INTO SOURCE VALUES (XMLPARSE(DOCUMENT '{long}' PRESERVE WHITESPACE))")).unwrap();
    a.commit().unwrap();
    a.run("INSERT INTO T SELECT 1, ARRAY[X], MULTISET[X], ROW(X, 1), CAST(X AS DX), NEW ST().X(X) FROM SOURCE").unwrap();
    a.run("DELETE FROM SOURCE").unwrap();
    a.commit().unwrap();
    drop((a, crashed));
    let (db, opened) = open();
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    let s = db.session("A");
    let length = |e: &str| format!("CHAR_LENGTH(XMLSERIALIZE(CONTENT {e} AS CLOB))");
    let lengths = format!("SELECT {}, {}, {}, {} FROM T", length("AX[1]"), length("T.R.F"), length("XML(D)"), length("T.S.X"));
    let n = long.chars().count().to_string();
    assert_eq!(query(&s, &lengths), [[n.as_str(); 4].join(",")]);
    let elements = format!("SELECT {} FROM T, UNNEST(T.MX) AS U(E)", length("E"));
    assert_eq!(query(&s, &elements), [n]);
}

/// Verifies: [DESIGN] RULES.md D31, D69, D106: the column store keeps no
/// value of a distinct type of XML, as it keeps no XML value, nor an array
/// of one.
#[test]
fn the_column_store_keeps_no_distinct_xml() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TYPE DX AS XML FINAL").unwrap();
    let a = db.session("A");
    for t in ["DX", "DX ARRAY[2]"] {
        let error = a.run(&format!("CREATE TABLE C (K INTEGER, V {t}) USING columnstore")).unwrap_err();
        assert_eq!(error.code, Code::Unsupported, "{t}: {error}");
    }
}
