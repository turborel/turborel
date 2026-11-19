//! \[EXTENSION\] XMLMAP (RULES.md D106): a table, a schema or the catalog
//! mapped to XML and to an XML Schema document [ISO/IEC 9075-14:2006
//! draft §9.4 to §9.17].

use std::collections::BTreeSet;

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};

use crate::common::{code, fetched, rows};

/// The namespace of `xsi`, as each data document declares it.
const XSI: &str = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;

/// Schema A: table T of an INTEGER K NOT NULL, a VARCHAR(20) S, a
/// VARBINARY(4) B and an XML(CONTENT) X, of the rows (1, 'a<b', X'0FB7',
/// <p/>) and (2, NULL, NULL, NULL).
fn mapped() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL, S VARCHAR(20), B VARBINARY(4), X XML(CONTENT))").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 'a<b', X'0FB7', XMLPARSE(CONTENT '<p/>' PRESERVE WHITESPACE)), (2, NULL, NULL, NULL)").unwrap();
    a.commit().unwrap();
    db
}

/// The value of `xml`, an XML value, serialized as CONTENT, as `s` sees
/// it.
fn text(s: &Session, xml: &str) -> String {
    let found = rows(s, &format!("VALUES (XMLSERIALIZE(CONTENT {xml} AS CLOB))")).unwrap();
    found.into_iter().next().unwrap()
}

/// The SQLSTATE of the error of `sql`.
fn state(s: &Session, sql: &str) -> SqlState {
    fetched(s, sql).expect_err(sql).sqlstate()
}

/// The names of the elements `text` holds, as their start tags write them.
fn element_names(text: &str) -> BTreeSet<String> {
    text.split('<')
        .skip(1)
        .filter(|tag| !tag.starts_with(['/', '?', '!']))
        .map(|tag| tag.chars().take_while(|c| !c.is_whitespace() && *c != '/' && *c != '>').collect())
        .collect()
}

/// The names of the elements the XML Schema document `schema` declares.
fn declared(schema: &str) -> BTreeSet<String> {
    schema.split(r#"<xs:element name=""#).skip(1).map(|rest| rest.chars().take_while(|c| *c != '"').collect()).collect()
}

/// `text` as a `<character string literal>`.
fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The names of the elements of the XML values `mapped` holds, which the
/// type of an XML column takes as any element unvalidated [§9.5 GR8r].
const WITHIN_XML: [&str; 1] = ["p"];

/// That XMLMAP of `source` with `options` and `encoding`, its METADATA
/// and its DATA, are an XML Schema document and data that parse back, as
/// DOCUMENT or as CONTENT, and the data's elements each one the schema
/// document declares, or one of an XML value: the schema document
/// describes the data, structurally.
fn validates(s: &Session, source: &str, (options, encoding): (&str, &str)) {
    let schema = text(s, &format!("XMLMAP({source} {options} METADATA {encoding})"));
    let data = text(s, &format!("XMLMAP({source} {options} DATA {encoding})"));
    let parses = |syntax: &str, t: &str| {
        let sql = format!("VALUES (XMLSERIALIZE(CONTENT XMLPARSE({syntax} {} PRESERVE WHITESPACE) AS CLOB))", literal(t));
        rows(s, &sql).unwrap() == [t]
    };
    assert!(parses("DOCUMENT", &schema), "{schema}");
    assert!(parses("CONTENT", &data), "{data}");
    let mut known = declared(&schema);
    known.extend(WITHIN_XML.map(str::to_owned));
    let undeclared: Vec<String> = element_names(&data).difference(&known).cloned().collect();
    assert!(undeclared.is_empty(), "{options}: {undeclared:?} of {data} not in {schema}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.10 GR6, GR7a,
/// §9.11 GR12c i, §9.8 GR9] TABLE, of the implicit options, NULLS ABSENT,
/// AS ELEMENT, DATA and ENCODING HEX: the table's element, declaring
/// `xsi`, of a `row` of each row, of an element of each value, a string's
/// characters mapped, a binary string in hex, an XML value serialized; a
/// null value's element absent; NULLS NIL, the element nilled.
#[test]
fn table_as_element_9_10() {
    let db = mapped();
    let a = db.session("A");
    let rows_text = "<row><K>1</K><S>a&lt;b</S><B>0FB7</B><X><p/></X></row>";
    assert_eq!(text(&a, "XMLMAP(TABLE T)"), format!("<T {XSI}>{rows_text}<row><K>2</K></row></T>"));
    assert_eq!(text(&a, "XMLMAP(TABLE A.T NULLS ABSENT AS ELEMENT DATA ENCODING HEX)"), text(&a, "XMLMAP(TABLE T)"));
    let nil = r#"<row><K>2</K><S xsi:nil="true"/><B xsi:nil="true"/><X xsi:nil="true"/></row>"#;
    assert_eq!(text(&a, "XMLMAP(TABLE T NULLS NIL)"), format!("<T {XSI}>{rows_text}{nil}</T>"));
    a.run("DELETE FROM T").unwrap();
    assert_eq!(text(&a, "XMLMAP(TABLE T)"), format!("<T {XSI}/>"));
    assert_eq!(rows(&a, "SELECT 1 FROM (VALUES 1) AS V(Z) WHERE XMLMAP(TABLE T) IS DOCUMENT").unwrap(), ["1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.10 GR6c ii,
/// GR6c iii, GR7c, §9.11 GR12c iii, GR12d i] AS FOREST: an element of
/// the table's name of each row, of no root, as content; NULLS NIL, each
/// declaring `xsi`; TARGETNAMESPACE, each of the default namespace; of
/// XML(CONTENT(UNTYPED)).
#[test]
fn table_as_forest_9_10() {
    let db = mapped();
    let a = db.session("A");
    assert_eq!(text(&a, "XMLMAP(TABLE T AS FOREST)"), "<T><K>1</K><S>a&lt;b</S><B>0FB7</B><X><p/></X></T><T><K>2</K></T>");
    let second = format!(r#"<T {XSI} xmlns="urn:t"><K>2</K><S xsi:nil="true"/><B xsi:nil="true"/><X xsi:nil="true"/></T>"#);
    let forest = text(&a, "XMLMAP(TABLE T NULLS NIL AS FOREST TARGETNAMESPACE 'urn:t')");
    assert!(forest.ends_with(&second), "{forest}");
    assert_eq!(rows(&a, "SELECT 1 FROM (VALUES 1) AS V(Z) WHERE XMLMAP(TABLE T AS FOREST) IS DOCUMENT").unwrap(), Vec::<String>::new());
    a.run("CREATE TABLE U (C XML(CONTENT(UNTYPED)), D XML(DOCUMENT(UNTYPED)))").unwrap();
    a.run("INSERT INTO U (C) VALUES (XMLMAP(TABLE T AS FOREST))").unwrap();
    a.run("INSERT INTO U (D) VALUES (XMLMAP(TABLE T))").unwrap();
    assert_eq!(code(&a, "SELECT 1 FROM U WHERE XMLMAP(TABLE T AS FOREST) = C"), Code::SyntaxRule, "XML is not comparable");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.9 GR5, GR10a,
/// §9.11 GR10e ii, §9.7] METADATA: the XML Schema document of the
/// columns' types, each once, the row type of an element of each column,
/// left out of a null value unless the column is NOT NULL, the table type
/// of rows, and the element of the table; NULLS NIL, nillable.
#[test]
fn table_metadata_9_9() {
    let db = mapped();
    let a = db.session("A");
    let schema = text(&a, "XMLMAP(TABLE T METADATA)");
    let expected = [
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">"#,
        r#"<xs:simpleType name="INTEGER"><xs:restriction base="xs:integer"><xs:maxInclusive value="9999999999"/><xs:minInclusive value="-9999999999"/></xs:restriction></xs:simpleType>"#,
        r#"<xs:simpleType name="VARCHAR_20"><xs:restriction base="xs:string"><xs:maxLength value="20"/></xs:restriction></xs:simpleType>"#,
        r#"<xs:simpleType name="VARBINARY_4"><xs:restriction base="xs:hexBinary"><xs:maxLength value="4"/></xs:restriction></xs:simpleType>"#,
        r#"<xs:complexType name="XML" mixed="true"><xs:sequence><xs:any minOccurs="0" maxOccurs="unbounded" processContents="skip"/></xs:sequence></xs:complexType>"#,
        r#"<xs:complexType name="RowType.TURBOREL.A.T"><xs:sequence><xs:element name="K" type="INTEGER"/><xs:element name="S" type="VARCHAR_20" minOccurs="0"/><xs:element name="B" type="VARBINARY_4" minOccurs="0"/><xs:element name="X" type="XML" minOccurs="0"/></xs:sequence></xs:complexType>"#,
        r#"<xs:complexType name="TableType.TURBOREL.A.T"><xs:sequence><xs:element name="row" type="RowType.TURBOREL.A.T" minOccurs="0" maxOccurs="unbounded"/></xs:sequence></xs:complexType>"#,
        r#"<xs:element name="T" type="TableType.TURBOREL.A.T"/></xs:schema>"#,
    ];
    assert_eq!(schema, expected.concat());
    let nil = text(&a, "XMLMAP(TABLE T NULLS NIL METADATA)");
    assert!(nil.contains(r#"<xs:element name="K" type="INTEGER"/><xs:element name="S" type="VARCHAR_20" nillable="true"/>"#), "{nil}");
    assert_eq!(rows(&a, "SELECT 1 FROM (VALUES 1) AS V(Z) WHERE XMLMAP(TABLE T METADATA) IS DOCUMENT").unwrap(), ["1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.9 GR10b, §9.11
/// GR10e iii] METADATA AS FOREST and TARGETNAMESPACE: the row type, the
/// element of the table of that type, the schema document of the target
/// namespace, qualified, which it declares its default namespace so the
/// names of its types resolve [Schema1 §3.15.3]; [ISO/IEC 9075-14:2006
/// draft §9.11 GR11, GR12c ii] the data's root of the default namespace,
/// with no schema location.
#[test]
fn table_target_namespace_9_11() {
    let db = mapped();
    let a = db.session("A");
    let schema = text(&a, "XMLMAP(TABLE T AS FOREST TARGETNAMESPACE 'urn:t&u' METADATA)");
    let root = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns="urn:t&amp;u" targetNamespace="urn:t&amp;u" elementFormDefault="qualified">"#;
    assert!(schema.starts_with(root), "{schema}");
    assert!(schema.ends_with(r#"</xs:complexType><xs:element name="T" type="RowType.TURBOREL.A.T"/></xs:schema>"#), "{schema}");
    assert!(!schema.contains("TableType"), "{schema}");
    let data = text(&a, "XMLMAP(TABLE T TARGETNAMESPACE 'urn:t')");
    assert!(data.starts_with(&format!(r#"<T {XSI} xmlns="urn:t"><row>"#)), "{data}");
    assert_eq!(text(&a, "XMLMAP(TABLE T TARGETNAMESPACE '')"), text(&a, "XMLMAP(TABLE T)"), "'' is no namespace");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.11 GR13] DATA AND
/// METADATA: a sequence of the XML Schema document then the data, of
/// XML(SEQUENCE), which an XML(CONTENT) column does not take, as it is
/// not one document node [§10.2 GR1].
#[test]
fn data_and_metadata_9_11() {
    let db = mapped();
    let a = db.session("A");
    let both = text(&a, "XMLMAP(TABLE T DATA AND METADATA)");
    assert_eq!(both, format!("{}{}", text(&a, "XMLMAP(TABLE T METADATA)"), text(&a, "XMLMAP(TABLE T DATA)")));
    assert_eq!(
        rows(&a, "SELECT 1 FROM (VALUES 1) AS V(Z) WHERE XMLMAP(TABLE T DATA AND METADATA) IS CONTENT").unwrap(),
        Vec::<String>::new()
    );
    a.run("CREATE TABLE U (C XML(CONTENT), S XML)").unwrap();
    let content = a.run("INSERT INTO U (C) VALUES (XMLMAP(TABLE T DATA AND METADATA))").unwrap_err();
    assert_eq!(content.sqlstate(), SqlState::NOT_AN_XQUERY_DOCUMENT_NODE, "two document nodes are no content");
    a.run("INSERT INTO U (S) VALUES (XMLMAP(TABLE T DATA AND METADATA))").unwrap();
    assert_eq!(rows(&a, "SELECT XMLSERIALIZE(CONTENT S AS CLOB) FROM U").unwrap(), [both]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.5 GR8b, §9.8
/// GR9b] ENCODING BASE64 and HEX: a binary string's value and type in the
/// encoding; of no ENCODING, in the XMLBINARY in scope [§11.3], which an
/// ENCODING written overrides.
#[test]
fn binary_encodings_9_5() {
    let db = mapped();
    let a = db.session("A");
    assert!(text(&a, "XMLMAP(TABLE T ENCODING BASE64)").contains("<B>D7c=</B>"));
    assert!(text(&a, "XMLMAP(TABLE T ENCODING HEX)").contains("<B>0FB7</B>"));
    assert!(
        text(&a, "XMLMAP(TABLE T METADATA ENCODING BASE64)")
            .contains(r#"<xs:restriction base="xs:base64Binary"><xs:maxLength value="4"/>"#)
    );
    validates(&a, "TABLE T", ("", "ENCODING BASE64"));
    let scoped = |map: &str| {
        let sql = format!("WITH XMLBINARY USING BASE64 SELECT XMLSERIALIZE(CONTENT {map} AS CLOB) FROM T WHERE K = 1");
        rows(&a, &sql).unwrap().concat()
    };
    assert!(scoped("XMLMAP(TABLE T)").contains("<B>D7c=</B>"));
    assert!(scoped("XMLMAP(TABLE T METADATA)").contains(r#"<xs:restriction base="xs:base64Binary">"#));
    assert!(scoped("XMLMAP(TABLE T ENCODING HEX)").contains("<B>0FB7</B>"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.1 GR4, §9.2,
/// §9.9 GR3, GR5c, GR6, GR7] the names of tables and columns, each fully
/// escaped: a space, a colon, a leading `xml` and an underscore before
/// `x`; and the multi-part names of the types, their periods escaped.
#[test]
fn names_escaped_9_1() {
    let db = Database::new();
    db.schema(r#"CREATE SCHEMA AUTHORIZATION A CREATE TABLE "my t.1" ("a:b" INTEGER, "xmlx" INTEGER, "_xA" INTEGER)"#).unwrap();
    let a = db.session("A");
    a.run(r#"INSERT INTO "my t.1" VALUES (1, 2, 3)"#).unwrap();
    let data = text(&a, r#"XMLMAP(TABLE "my t.1")"#);
    assert_eq!(
        data,
        format!("<my_x0020_t.1 {XSI}><row><a_x003A_b>1</a_x003A_b><_x0078_mlx>2</_x0078_mlx><_x005F_xA>3</_x005F_xA></row></my_x0020_t.1>")
    );
    let schema = text(&a, r#"XMLMAP(TABLE "my t.1" METADATA)"#);
    assert!(schema.contains(r#"<xs:complexType name="RowType.TURBOREL.A.my_x0020_t_x002E_1">"#), "{schema}");
    validates(&a, r#"TABLE "my t.1""#, ("NULLS NIL", ""));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.11 GR7a, GR9]
/// [XML 1.1 §2.3] a name of XML 1.1 that is no name of XML 1.0: the
/// documents are of XML 1.1, which a serialization of XML 1.1 writes.
#[test]
fn names_of_xml_11_9_11() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE \"\u{237}\" (K INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO \"\u{237}\" VALUES (1), (2)").unwrap();
    let document = rows(&a, "VALUES (XMLSERIALIZE(DOCUMENT XMLMAP(TABLE \"\u{237}\") AS CLOB VERSION '1.1'))").unwrap();
    assert_eq!(
        document,
        [format!("<?xml version=\"1.1\" encoding=\"UTF-8\"?><\u{237} {XSI}><row><K>1</K></row><row><K>2</K></row></\u{237}>")]
    );
    let forest = rows(&a, "VALUES (XMLSERIALIZE(CONTENT XMLMAP(TABLE \"\u{237}\" AS FOREST) AS CLOB VERSION '1.1'))").unwrap();
    assert_eq!(forest, ["<\u{237}><K>1</K></\u{237}><\u{237}><K>2</K></\u{237}>"]);
    let schema = rows(&a, "VALUES (XMLSERIALIZE(DOCUMENT XMLMAP(TABLE \"\u{237}\" METADATA) AS CLOB VERSION '1.1'))").unwrap();
    assert!(schema[0].starts_with("<?xml version=\"1.1\""), "{schema:?}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.4, §9.5 GR8a to
/// GR8m, GR8o, §9.7 GR3, §9.8] the types of each kind of column: a
/// domain's restricting its data type; a distinct type's its source type;
/// datetimes' and an interval's patterns, `xdt` declared and imported; the
/// schema document parses and declares every element of the data.
#[test]
fn types_of_columns_9_5() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE DOMAIN FLAG AS BOOLEAN CREATE TYPE MONEY AS DECIMAL(9,2) FINAL \
         CREATE TABLE W (F FLAG, M MONEY, D DATE, TM TIME(3), TS TIMESTAMP WITH TIME ZONE, I INTERVAL DAY(3) TO SECOND(2), \
         R REAL, N NUMERIC(7,2), G BIGINT, C CHAR(3), L CLOB(100), BL BLOB(10))",
    )
    .unwrap();
    let a = db.session("A");
    a.run(
        "INSERT INTO W VALUES (TRUE, CAST(1.5 AS MONEY), DATE '2024-01-02', TIME '10:11:12.345', \
         TIMESTAMP '2024-01-02 10:11:12+01:00', INTERVAL '1 02:03:04.5' DAY TO SECOND, 1.5, 12.34, 7, 'ab', 'cd', X'01')",
    )
    .unwrap();
    let schema = text(&a, "XMLMAP(TABLE W METADATA)");
    for expected in [
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xdt="http://www.w3.org/2005/xpath-datatypes"><xs:import namespace="http://www.w3.org/2005/xpath-datatypes"/>"#,
        r#"<xs:simpleType name="Domain.TURBOREL.A.FLAG"><xs:restriction base="BOOLEAN"/></xs:simpleType><xs:simpleType name="BOOLEAN">"#,
        r#"<xs:simpleType name="UDT.TURBOREL.A.MONEY"><xs:restriction base="NUMERIC_9_2"/></xs:simpleType>"#,
        r#"<xs:simpleType name="INTERVAL_DAY_TO_SECOND_3_2"><xs:restriction base="xdt:dayTimeDuration"><xs:pattern value="-?P\p{Nd}{3}DT\p{Nd}{2}H\p{Nd}{2}M\p{Nd}{2}.\p{Nd}{2}S"/>"#,
        r#"<xs:element name="F" type="Domain.TURBOREL.A.FLAG" minOccurs="0"/><xs:element name="M" type="UDT.TURBOREL.A.MONEY" minOccurs="0"/>"#,
        r#"<xs:simpleType name="CHAR_3"><xs:restriction base="xs:string"><xs:length value="3"/>"#,
        r#"<xs:simpleType name="BLOB_10"><xs:restriction base="xs:hexBinary"><xs:maxLength value="10"/>"#,
    ] {
        assert!(schema.contains(expected), "{expected} in {schema}");
    }
    let data = text(&a, "XMLMAP(TABLE W)");
    for expected in ["<F>true</F><M>1.50</M><D>2024-01-02</D>", "<I>P001DT02H03M04.50S</I>", "<C>ab </C><L>cd</L><BL>01</BL>"] {
        assert!(data.contains(expected), "{expected} in {data}");
    }
    validates(&a, "TABLE W", ("NULLS NIL TARGETNAMESPACE 'urn:w'", "ENCODING BASE64"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §4.10.8, §9.9 GR4,
/// GR5a, §9.10 GR4] an XML unmappable column, of XML(SEQUENCE) or an
/// array of it, and [EXTENSION] one of DECFLOAT, which §9.5 maps to no
/// type, is in neither document; a table of no other column maps to rows
/// of no element.
#[test]
fn unmappable_columns_left_out_4_10_8() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML, F DECFLOAT(16), R XML ARRAY[3]) CREATE TABLE E (X XML)")
        .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, XMLCOMMENT('c' RETURNING SEQUENCE), 1.5, NULL)").unwrap();
    a.run("INSERT INTO E VALUES (NULL)").unwrap();
    assert_eq!(text(&a, "XMLMAP(TABLE T)"), format!("<T {XSI}><row><K>1</K></row></T>"));
    let schema = text(&a, "XMLMAP(TABLE T METADATA)");
    assert!(schema.contains(r#"<xs:sequence><xs:element name="K" type="INTEGER" minOccurs="0"/></xs:sequence>"#), "{schema}");
    assert!(!schema.contains(r#"name="X""#) && !schema.contains("DECFLOAT"), "{schema}");
    assert_eq!(text(&a, "XMLMAP(TABLE E)"), format!("<E {XSI}><row/></E>"));
    validates(&a, "TABLE E", ("AS FOREST", ""));
}

/// The warning the query `sql` completed with, its rows all read, if any.
fn warning(s: &Session, sql: &str) -> Option<SqlState> {
    rows(s, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    s.warning()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.11 GR3, §9.14
/// GR3, §9.17 GR3, §23.1] [ISO/IEC 9075:1992 §3.3.4.1] a mapping of a
/// visible column that is an XML unmappable column completes with the
/// warning column cannot be mapped, 01010: of DATA, and of METADATA, of no
/// row, which is not folded away; of a schema and the catalog holding
/// one; none of a column that is not visible, of a table the user may not
/// SELECT or of a distinct type it may not cast, nor of a mapping of none.
#[test]
fn column_cannot_be_mapped_9_11() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TYPE MONEY AS NUMERIC(10, 2) \
         CREATE TABLE T (K INTEGER, X XML) CREATE TABLE U (K INTEGER, P MONEY)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("GRANT SELECT ON U TO B").unwrap();
    a.commit().unwrap();
    let warned = Some(SqlState::COLUMN_CANNOT_BE_MAPPED);
    for sql in ["XMLMAP(TABLE T)", "XMLMAP(TABLE T METADATA)", "XMLMAP(SCHEMA A AS FOREST)", "XMLMAP(CATALOG DATA AND METADATA)"] {
        assert_eq!(warning(&a, &format!("VALUES ({sql})")), warned, "{sql}");
    }
    assert_eq!(warning(&a, "VALUES (XMLMAP(TABLE U))"), None);
    let b = db.session("B");
    assert_eq!(warning(&b, "VALUES (XMLMAP(SCHEMA A))"), None, "T is no visible table, nor P a visible column");
    assert_eq!(text(&b, "XMLMAP(SCHEMA A)"), format!("<A {XSI}><U/></A>"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §4.10.8] [X3.135-1986
/// §6.6 SR3] TABLE needs SELECT on its table, else SQLCODE -104; a
/// column of a distinct type is visible only with EXECUTE on its cast to
/// its source type [ISO/IEC 9075-2:2023 §6.13 AR2].
#[test]
fn privileges_of_a_table_4_10_8() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TYPE MONEY AS NUMERIC(10, 2) CREATE TABLE T (K INTEGER, P MONEY)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, CAST(2 AS MONEY))").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    assert_eq!(code(&b, "VALUES (XMLMAP(TABLE A.T))"), Code::Privilege);
    assert_eq!(code(&b, "VALUES (XMLMAP(TABLE A.T METADATA))"), Code::Privilege);
    a.run("GRANT SELECT ON T TO B").unwrap();
    a.commit().unwrap();
    assert_eq!(text(&b, "XMLMAP(TABLE A.T)"), format!("<T {XSI}><row><K>1</K></row></T>"));
    assert!(text(&a, "XMLMAP(TABLE T)").contains("<P>2.00</P>"), "the owner executes the cast");
    a.run("GRANT EXECUTE ON SPECIFIC FUNCTION MONEY_TO_SOURCE TO B").unwrap();
    a.commit().unwrap();
    assert_eq!(text(&b, "XMLMAP(TABLE A.T)"), format!("<T {XSI}><row><K>1</K><P>2.00</P></row></T>"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §4.10.9] a view is
/// mapped as a table; XMLMAP of a table in a view's query is a reading of
/// the table [ISO/IEC 9075:1992 §11.19], so DROP TABLE RESTRICT is
/// refused while the view reads it.
#[test]
fn views_and_dependencies_4_10_9() {
    let db = mapped();
    let a = db.session("A");
    a.run("CREATE VIEW V (C1, C2) AS SELECT K, S FROM T WHERE K = 1").unwrap();
    assert_eq!(text(&a, "XMLMAP(TABLE V)"), format!("<V {XSI}><row><C1>1</C1><C2>a&lt;b</C2></row></V>"));
    a.run("CREATE VIEW M (X) AS VALUES (XMLMAP(TABLE T AS FOREST))").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT XMLSERIALIZE(CONTENT X AS CLOB) FROM M").unwrap(), [text(&a, "XMLMAP(TABLE T AS FOREST)")]);
    assert_eq!(a.run("DROP TABLE T RESTRICT").unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.12, §9.13, §9.14]
/// SCHEMA: its XML visible tables in order of name, base and viewed, a
/// table of no XML visible column left out; the schema's element of each
/// table's element, as each is mapped by §9.10 of no target namespace,
/// or of its rows as a forest; the schema document of its type, of all
/// the tables or of a sequence of their rows, and their types.
#[test]
fn schemas_9_14() {
    let db = mapped();
    let a = db.session("A");
    a.run("CREATE VIEW V AS SELECT K FROM T WHERE K = 2").unwrap();
    a.run("CREATE TABLE E (X XML)").unwrap();
    let t = "<T><row><K>1</K><S>a&lt;b</S><B>0FB7</B><X><p/></X></row><row><K>2</K></row></T>";
    assert_eq!(text(&a, "XMLMAP(SCHEMA A)"), format!("<A {XSI}>{t}<V><row><K>2</K></row></V></A>"));
    let target = text(&a, "XMLMAP(SCHEMA A NULLS NIL AS FOREST TARGETNAMESPACE 'urn:a')");
    assert!(target.starts_with(&format!(r#"<A {XSI} xmlns="urn:a"><T><K>1</K>"#)), "{target}");
    assert!(target.ends_with("<V><K>2</K></V></A>"), "{target}");
    let schema = text(&a, "XMLMAP(SCHEMA A METADATA)");
    let all = r#"<xs:complexType name="SchemaType.TURBOREL.A"><xs:all><xs:element name="T" type="TableType.TURBOREL.A.T"/><xs:element name="V" type="TableType.TURBOREL.A.V"/></xs:all></xs:complexType><xs:element name="A" type="SchemaType.TURBOREL.A"/></xs:schema>"#;
    assert!(schema.ends_with(all), "{schema}");
    assert!(schema.contains(r#"<xs:complexType name="RowType.TURBOREL.A.V">"#) && !schema.contains(".A.E"), "{schema}");
    let forest = text(&a, "XMLMAP(SCHEMA A AS FOREST METADATA)");
    assert!(
        forest.contains(r#"<xs:sequence><xs:element name="T" type="RowType.TURBOREL.A.T" minOccurs="0" maxOccurs="unbounded"/>"#),
        "{forest}"
    );
    for options in [
        ("", ""),
        ("NULLS NIL AS FOREST", ""),
        ("AS ELEMENT TARGETNAMESPACE 'urn:a'", "ENCODING HEX"),
        ("AS FOREST TARGETNAMESPACE 'urn:a'", "ENCODING BASE64"),
    ] {
        validates(&a, "SCHEMA A", options);
    }
    assert_eq!(state(&a, "VALUES (XMLMAP(SCHEMA NOPE))"), SqlState::INVALID_SCHEMA_NAME);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §4.10.8, §9.12 GR4,
/// §9.14 GR4] a schema's mapping holds the tables whose SELECT the user
/// holds alone, never refused for one it does not.
#[test]
fn schemas_of_visible_tables_4_10_8() {
    let db = mapped();
    let a = db.session("A");
    a.run("CREATE TABLE U (Q INTEGER)").unwrap();
    a.run("INSERT INTO U VALUES (5)").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    assert_eq!(text(&b, "XMLMAP(SCHEMA A)"), format!("<A {XSI}/>"));
    a.run("GRANT SELECT ON U TO B").unwrap();
    a.commit().unwrap();
    assert_eq!(text(&b, "XMLMAP(SCHEMA A)"), format!("<A {XSI}><U><row><Q>5</Q></row></U></A>"));
    assert!(!text(&b, "XMLMAP(SCHEMA A METADATA)").contains(".A.T"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.15, §9.16,
/// §9.17] CATALOG: the catalog's element of each XML visible schema's,
/// in order of name, SQL/MM Full-Text's FT_INFORMATION_SCHEMA, the
/// Information Schema, Simple feature access's SF_INFORMTN_SCHEMA and
/// SQL/MM Spatial's ST_INFORMTN_SCHEMA among them; the schema document of
/// the catalog's type, of all the schemas.
#[test]
fn the_catalog_9_17() {
    let db = mapped();
    let a = db.session("A");
    let data = text(&a, "XMLMAP(CATALOG)");
    let t = "<T><row><K>1</K><S>a&lt;b</S><B>0FB7</B><X><p/></X></row><row><K>2</K></row></T>";
    assert!(data.starts_with(&format!("<TURBOREL {XSI}><A>{t}</A><FT_INFORMATION_SCHEMA>")), "{data}");
    assert!(data.contains("</FT_INFORMATION_SCHEMA><INFORMATION_SCHEMA>"), "{data}");
    assert!(data.contains("</INFORMATION_SCHEMA><SF_INFORMTN_SCHEMA>"), "{data}");
    assert!(data.contains("</SF_INFORMTN_SCHEMA><ST_INFORMTN_SCHEMA>"), "{data}");
    assert!(data.ends_with("</ST_INFORMTN_SCHEMA></TURBOREL>"));
    let schema = text(&a, "XMLMAP(CATALOG METADATA)");
    let catalog = r#"<xs:complexType name="CatalogType.TURBOREL"><xs:all><xs:element name="A" type="SchemaType.TURBOREL.A"/><xs:element name="FT_INFORMATION_SCHEMA" type="SchemaType.TURBOREL.FT_INFORMATION_SCHEMA"/><xs:element name="INFORMATION_SCHEMA" type="SchemaType.TURBOREL.INFORMATION_SCHEMA"/><xs:element name="SF_INFORMTN_SCHEMA" type="SchemaType.TURBOREL.SF_INFORMTN_SCHEMA"/><xs:element name="ST_INFORMTN_SCHEMA" type="SchemaType.TURBOREL.ST_INFORMTN_SCHEMA"/></xs:all></xs:complexType><xs:element name="TURBOREL" type="CatalogType.TURBOREL"/></xs:schema>"#;
    assert!(schema.ends_with(catalog), "{schema}");
    let information = text(&a, "XMLMAP(SCHEMA INFORMATION_SCHEMA)");
    assert!(
        information
            .starts_with(&format!("<INFORMATION_SCHEMA {XSI}><ADMINISTRABLE_ROLE_AUTHORIZATIONS/><ADMIN_ROLE_AUTHS/><APPLICABLE_ROLES/>"))
    );
    validates(&a, "CATALOG", ("NULLS NIL AS FOREST TARGETNAMESPACE 'urn:c'", ""));
}

/// Verifies: [EXTENSION] RULES.md D106 the grammar of XMLMAP, and its
/// Syntax Rules: an option not of the grammar, or out of its order, is
/// SQLCODE -101; a target namespace of `xml`'s or `xmlns`'s, a table that
/// is a query name, or XMLMAP in a constraint SQLCODE -103 [ISO/IEC
/// 9075:1992 §11.9 SR6]; a schema of another catalog SQLSTATE 3D000.
#[test]
fn refusals() {
    let db = mapped();
    let a = db.session("A");
    for bad in [
        "XMLMAP(VIEW T)",
        "XMLMAP(TABLE T NULLS SOME)",
        "XMLMAP(TABLE T AS ROWS)",
        "XMLMAP(TABLE T DATA NULLS NIL)",
        "XMLMAP(TABLE T ENCODING UTF8)",
        "XMLMAP(CATALOG A)",
    ] {
        assert_eq!(code(&a, &format!("VALUES ({bad})")), Code::Syntax, "{bad}");
    }
    for bad in [
        "VALUES (XMLMAP(TABLE T TARGETNAMESPACE 'http://www.w3.org/XML/1998/namespace'))",
        "VALUES (XMLMAP(TABLE T TARGETNAMESPACE 'http://www.w3.org/2000/xmlns/'))",
        "VALUES (XMLMAP(TABLE NOPE))",
        "WITH Q (C) AS (VALUES 1) SELECT XMLMAP(TABLE Q) FROM Q",
    ] {
        assert_eq!(code(&a, bad), Code::SyntaxRule, "{bad}");
    }
    let check = a.run("CREATE TABLE C (K INTEGER CHECK (XMLMAP(TABLE T) IS DOCUMENT))").unwrap_err();
    assert_eq!(check.code, Code::SyntaxRule);
    assert_eq!(state(&a, "VALUES (XMLMAP(SCHEMA ELSEWHERE.A))"), SqlState::INVALID_CATALOG_NAME);
    assert_eq!(text(&a, "XMLMAP(SCHEMA TURBOREL.A)"), text(&a, "XMLMAP(SCHEMA A)"));
}

/// Verifies: [EXTENSION] RULES.md D106 [ISO/IEC 9075:1992 §11.2 SR3] a
/// view, a routine or a trigger whose text maps schema S by XMLMAP
/// depends on S: DROP SCHEMA S RESTRICT is refused while one is, and
/// CASCADE drops each; one that maps the catalog, or another schema, is
/// kept. A mapping of a schema reads the tables there are as it runs.
#[test]
fn mappings_depend_on_their_schema() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE L (X XML)").unwrap();
    db.schema("CREATE SCHEMA S AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let a = db.session("A");
    a.run("CREATE VIEW V (X) AS VALUES (XMLMAP(SCHEMA S))").unwrap();
    a.run("CREATE VIEW W (X) AS VALUES (XMLMAP(CATALOG METADATA)), (XMLMAP(SCHEMA A))").unwrap();
    a.run("CREATE FUNCTION F () RETURNS XML NOT DETERMINISTIC READS SQL DATA RETURN XMLMAP(SCHEMA \"S\" METADATA)").unwrap();
    a.run("CREATE TRIGGER G AFTER INSERT ON L FOR EACH STATEMENT INSERT INTO L VALUES (XMLMAP(SCHEMA TURBOREL.S))").unwrap();
    a.commit().unwrap();
    a.run("CREATE TABLE S.U (Q INTEGER)").unwrap();
    let mapped = rows(&a, "SELECT XMLSERIALIZE(CONTENT X AS CLOB) FROM V").unwrap();
    assert!(mapped[0].contains("<U/>"), "a table made after the view is mapped: {mapped:?}");
    let refused = a.run("DROP SCHEMA S RESTRICT").unwrap_err();
    assert_eq!(refused.code, Code::SyntaxRule, "{refused:?}");
    a.run("DROP TABLE S.T CASCADE").unwrap();
    a.run("DROP TABLE S.U CASCADE").unwrap();
    assert_eq!(a.run("DROP SCHEMA S RESTRICT").unwrap_err().code, Code::SyntaxRule, "an empty schema still mapped");
    a.run("DROP SCHEMA S CASCADE").unwrap();
    a.commit().unwrap();
    let left = "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'A' ORDER BY TABLE_NAME";
    assert_eq!(rows(&a, left).unwrap(), ["L", "W"]);
    assert_eq!(rows(&a, "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A'").unwrap(), Vec::<String>::new());
    assert_eq!(rows(&a, "SELECT TRIGGER_NAME FROM INFORMATION_SCHEMA.TRIGGERS").unwrap(), Vec::<String>::new());
}

/// Verifies: [EXTENSION] RULES.md D106 a mapping of a schema or the catalog
/// leaves out a view whose query, or a view it reads, maps a schema or the
/// catalog, whose mapping could hold itself; one that reaches such a
/// mapping through a routine is refused, SQLCODE -103, where it would
/// recurse without end.
#[test]
fn mappings_never_hold_themselves() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE L (K INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO L VALUES (1)").unwrap();
    a.run("CREATE VIEW W (X) AS VALUES (XMLMAP(SCHEMA A))").unwrap();
    a.run("CREATE VIEW Y AS SELECT * FROM W").unwrap();
    a.run("CREATE VIEW C (X) AS VALUES (XMLMAP(CATALOG METADATA))").unwrap();
    a.commit().unwrap();
    let held = format!("<A {XSI}><L><row><K>1</K></row></L></A>");
    assert_eq!(rows(&a, "SELECT XMLSERIALIZE(CONTENT X AS CLOB) FROM Y").unwrap(), std::slice::from_ref(&held));
    assert_eq!(text(&a, "XMLMAP(SCHEMA A)"), held);
    assert!(!text(&a, "XMLMAP(CATALOG METADATA)").contains(".A.C\""));
    a.run("CREATE FUNCTION G () RETURNS XML NOT DETERMINISTIC READS SQL DATA RETURN XMLMAP(SCHEMA A)").unwrap();
    a.run("CREATE VIEW Z (X) AS VALUES (G())").unwrap();
    a.commit().unwrap();
    assert_eq!(code(&a, "VALUES (XMLMAP(SCHEMA A))"), Code::SyntaxRule);
    assert!(text(&a, "XMLMAP(TABLE L)").contains("<K>1</K>"), "a mapping of a table holds no other");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §4.10.8, §9.4 GR19,
/// GR21, GR22, §9.5 GR8n, GR8o, GR8p, GR8q, §9.8 GR1a, GR9i, GR9j] columns
/// of row, array and multiset types, and of a distinct type of an array,
/// are mapped: an element of each field and of each element, a null
/// element nilled, a null field left out, NULLS absent, or nilled, NULLS
/// nil; their types complex types, defined with the types they are made
/// of; the schema document describes the data.
#[test]
fn composite_columns_9_8() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TYPE TRIO AS INTEGER ARRAY[3] \
         CREATE TABLE C (K INTEGER, P ROW(X INTEGER, Y VARCHAR(5)), L INTEGER ARRAY[3], M VARCHAR(2) MULTISET, D TRIO)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO C VALUES (1, ROW(2, CAST(NULL AS VARCHAR(5))), ARRAY[4, CAST(NULL AS INTEGER)], MULTISET['a'], CAST(ARRAY[7] AS TRIO))").unwrap();
    let nil = r#"<element xsi:nil="true"/>"#;
    let row = format!("<K>1</K><P><X>2</X></P><L><element>4</element>{nil}</L><M><element>a</element></M><D><element>7</element></D>");
    assert_eq!(text(&a, "XMLMAP(TABLE C)"), format!("<C {XSI}><row>{row}</row></C>"));
    let nilled = text(&a, "XMLMAP(TABLE C NULLS NIL)");
    assert!(nilled.contains(r#"<P><X>2</X><Y xsi:nil="true"/></P>"#), "{nilled}");
    let schema = text(&a, "XMLMAP(TABLE C METADATA)");
    for expected in [
        r#"<xs:complexType name="Row.X.INTEGER.Y.VARCHAR_5"><xs:sequence><xs:element name="X" type="INTEGER" minOccurs="0"/><xs:element name="Y" type="VARCHAR_5" minOccurs="0"/></xs:sequence></xs:complexType>"#,
        r#"<xs:complexType name="Array_3.INTEGER"><xs:sequence><xs:element name="element" minOccurs="0" maxOccurs="3" nillable="true" type="INTEGER"/></xs:sequence></xs:complexType>"#,
        r#"<xs:element name="element" minOccurs="0" maxOccurs="unbounded" nillable="true" type="VARCHAR_2"/>"#,
        r#"<xs:complexType name="UDT.TURBOREL.A.TRIO"><xs:complexContent><xs:extension base="Array_3.INTEGER"/></xs:complexContent></xs:complexType>"#,
    ] {
        assert!(schema.contains(expected), "{expected} in {schema}");
    }
    assert_eq!(warning(&a, "VALUES (XMLMAP(TABLE C))"), None, "every column maps");
    validates(&a, "TABLE C", ("NULLS NIL TARGETNAMESPACE 'urn:c'", ""));
    validates(&a, "SCHEMA A", ("AS FOREST", "ENCODING BASE64"));
}
