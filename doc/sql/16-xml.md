# XML Values

[Contents](README.md)

The XML type holds XML documents and fragments of them, as SQL/XML
(ISO/IEC 9075-14) defines it. XMLPARSE makes an XML value from text, and
XMLSERIALIZE writes one back as text. XMLDOCUMENT makes a document node
of a value, XMLCAST casts SQL values to XML and back, and IS DOCUMENT and
IS CONTENT test what a value is. XMLQUERY, XMLEXISTS and XMLTABLE query
XML values with XQuery. XMLMAP maps a table, a schema or the catalog to
XML and to an XML Schema document. XMLVALIDATE validates a value by a
registered XML Schema, IS VALID tests whether it is valid by one, and
XQuery's validate expression validates nodes by them in a query.

## The type

```
<XML type> ::= XML [ ( <XML type modifier> ) ]
<XML type modifier> ::=
      DOCUMENT [ ( <secondary XML type modifier> ) ]
    | CONTENT [ ( <secondary XML type modifier> ) ]
    | SEQUENCE
<secondary XML type modifier> ::=
      ANY
    | UNTYPED
    | XMLSCHEMA <XML valid according to what> [ <XML valid element clause> ]
```

| Type | Holds |
|---|---|
| XML(DOCUMENT) | A document: one root element, with comments and processing instructions beside it. |
| XML(CONTENT) | A document node of any children: elements, text, comments and processing instructions. |
| XML(SEQUENCE) | Any XML value. XML alone is XML(SEQUENCE). |

UNTYPED, ANY and XMLSCHEMA say whether a value may carry types from an
XML Schema. Every value XMLPARSE makes is UNTYPED. DOCUMENT and CONTENT
alone are ANY. An UNTYPED column takes values of an UNTYPED type only;
an ANY or SEQUENCE column takes any XML value. A type of XMLSCHEMA holds
values valid by a [registered XML Schema](#registered-xml-schemas),
which [The XMLSCHEMA types](#the-xmlschema-types) describes.

A value stored in a DOCUMENT column that is not a document is refused
with SQLSTATE 2200L. A short value is kept in its row, and a long one
apart from it as large objects are, logged and recovered with them. A
value inside an array, a multiset, a row or a structured value is kept
whole in that value's row, up to the limit of a value's stored form. The
column store keeps no XML value, nor any inside another value.

XML values are not comparable. `=`, IN, DISTINCT, GROUP BY, ORDER BY,
MAX, MIN, UNION without ALL, a NATURAL or USING join on an XML column,
and a key or an index of one are refused (SQLCODE -103), but an index of
an XML method ([XML indexes](#xml-indexes)).

[The Information Schema](07-information-schema.md) shows a type's modifiers
in XML_PRIMARY_MODIFIER and XML_SECONDARY_MODIFIER, the second null of
SEQUENCE. A parameter or result of a routine of an XML type is passed by
reference: XML_PASSING_MECHANISM in PARAMETERS and XML_RETURN_MECHANISM
in ROUTINES are BY REF.

```sql
CREATE TABLE PAGES (ID INTEGER, BODY XML(DOCUMENT(UNTYPED)), NOTES XML);
SELECT COLUMN_NAME, DATA_TYPE, XML_PRIMARY_MODIFIER, XML_SECONDARY_MODIFIER FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'PAGES' ORDER BY ORDINAL_POSITION;
```

```
OK
 COLUMN_NAME                                                  | DATA_TYPE                                                    | XML_PRIMARY_MODIFIER                                         | XML_SECONDARY_MODIFIER
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 ID                                                           | INTEGER                                                      | NULL                                                         | NULL
 BODY                                                         | XML                                                          | DOCUMENT                                                     | UNTYPED
 NOTES                                                        | XML                                                          | SEQUENCE                                                     | NULL
(3 rows)
```

## XMLPARSE

```
XMLPARSE ( { DOCUMENT | CONTENT } <string value expression>
           { PRESERVE | STRIP } WHITESPACE )
```

XMLPARSE reads a character string, a binary string or a large object as
XML 1.0 or XML 1.1, with namespaces. DOCUMENT requires a document, else
SQLSTATE 2200M. CONTENT takes a document or any well-formed fragment,
else 2200N. Its result is XML(DOCUMENT(UNTYPED)) or
XML(CONTENT(UNTYPED)).

STRIP WHITESPACE removes text that is white space alone, except within
an element whose `xml:space` is `preserve`. PRESERVE WHITESPACE keeps
every character.

The internal DTD subset is read: its entities are expanded and its
attribute defaults added. No external DTD or external entity is read,
so a parse never opens a file or a URL. A reference to an entity that
would need one fails as the parse does.

A binary string is read by its byte order mark and the encoding its XML
declaration names: UTF-8, UTF-16, UTF-16BE, UTF-16LE, ISO-8859-1 or
US-ASCII.

## XMLSERIALIZE

```
XMLSERIALIZE ( [ DOCUMENT | CONTENT ] <XML value expression> AS <data type>
               [ ENCODING { UTF8 | UTF16 } ]
               [ VERSION '1.0' | VERSION '1.1' ]
               [ INCLUDING XMLDECLARATION | EXCLUDING XMLDECLARATION ] )
```

XMLSERIALIZE writes a value as text of the character or binary string
type AS names. CONTENT is the default. DOCUMENT requires a document,
else SQLSTATE 2200L. ENCODING, of a binary string alone, is UTF8 by
default; UTF16 writes big-endian after a byte order mark. VERSION is
'1.0' by default.

An XML declaration is written with INCLUDING XMLDECLARATION, and for a
DOCUMENT of version '1.1'. A result longer than its type fails with
SQLSTATE 22001. Characters a parse would change, such as a carriage
return, are written as character references.

A value selected without XMLSERIALIZE is shown as its serialization.

```sql
CREATE TABLE ORDERS (ID INTEGER, BODY XML(DOCUMENT));
INSERT INTO ORDERS VALUES (1, XMLPARSE(DOCUMENT '<order no="17"><item>tea</item> <item>cups</item></order>' STRIP WHITESPACE));
SELECT ID, XMLSERIALIZE(DOCUMENT BODY AS VARCHAR(100)) FROM ORDERS;
SELECT XMLSERIALIZE(BODY AS VARCHAR(100) INCLUDING XMLDECLARATION) FROM ORDERS;
INSERT INTO ORDERS VALUES (2, XMLPARSE(CONTENT '<a/><b/>' PRESERVE WHITESPACE));
SELECT ID FROM ORDERS ORDER BY BODY;
```

```
OK
OK
           ID |
--------------+--------------------------------------------------------------
            1 | <order no="17"><item>tea</item><item>cups</item></order>
(1 row)

--------------------------------------------------------------
 <?xml version="1.0" encoding="UTF-8"?><order no="17"><item>tea</item><item>cups</item></order>
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2200L): the value of XML(DOCUMENT(ANY)) is not an XML document [ISO/IEC 9075:1992 §13.8 GR4b iii; ISO/IEC 9075-14:2006 draft §10.2 GR1a]
turborel: SQLCODE -103 (SQLSTATE 2A000): ORDER BY orders values of XML(DOCUMENT(ANY)), and XML values are not comparable [ISO/IEC 9075-14:2006 draft §10.11 SR1]
```

## CAST

CAST takes an XML value to another XML type, and nothing else to or from
one. A value cast to a DOCUMENT or CONTENT type is checked as one stored
in such a column. BY REF, the default, keeps the value's nodes, and casts
to an UNTYPED type only from an UNTYPED one, and to a type of XMLSCHEMA
only from one its column would take. BY VALUE copies them: to an UNTYPED
type without their types, to a type of XMLSCHEMA validating them by its
registered XML Schema, as [XMLVALIDATE](#xmlvalidate) does.

```
CAST ( <value expression> AS <XML type> [ BY REF | BY VALUE ] )
```

```sql
INSERT INTO ORDERS VALUES (3, XMLPARSE(DOCUMENT '<a/>' PRESERVE WHITESPACE));
SELECT XMLSERIALIZE(BODY AS VARBINARY(40) ENCODING UTF16) FROM ORDERS WHERE ID = 3;
SELECT XMLSERIALIZE(CAST(BODY AS XML(CONTENT(UNTYPED)) BY VALUE) AS VARCHAR(80)) FROM ORDERS;
SELECT CAST(BODY AS VARCHAR(80)) FROM ORDERS;
```

```
OK

--------------------------------------------------------------
 FEFF003C0061002F003E
(1 row)

--------------------------------------------------------------
 <order no="17"><item>tea</item><item>cups</item></order>
 <a/>
(2 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): CAST of XML(DOCUMENT(ANY)) to CHARACTER VARYING(80) is not valid [ISO/IEC 9075-14:2006 draft §6.5 SR3] (at character 7)
```

## XMLCAST

```
XMLCAST ( { <value expression> | NULL } AS { <data type> | <domain name> }
          [ BY REF | BY VALUE ] )
```

XMLCAST casts a value of an SQL type to an XML type, or an XML value to
an SQL type. Between two XML types it is CAST. No SQL value is cast to a
type of XMLSCHEMA.

An SQL value cast to XML(CONTENT) becomes a document node holding its
text: a string's characters, a number as CAST writes it, a truth value
as `true` or `false`, a date and time with a T between them, an interval
as an XML duration such as `P01Y02M`, and a binary string in hex, or in
base64 where XMLBINARY USING BASE64 is in scope
([XML options](#xml-options-of-a-scope)). Cast
to XML(SEQUENCE), it becomes one atomic value of the XML Schema type its
type maps to, in that type's canonical form: `xs:decimal` 1.5 for
NUMERIC 1.50, `xdt:yearMonthDuration` P1Y2M, `xs:hexBinary` or, of
BASE64, `xs:base64Binary` for a binary string. A character no XML 1.1
character fails with SQLSTATE 0N002. No SQL value is cast to
XML(DOCUMENT). No array, multiset or row is cast either, but a value of
a distinct type whose source is an array or a multiset is cast to
XML(CONTENT), written as XMLELEMENT writes it as content; cast to
XML(SEQUENCE), it is refused (SQLCODE -900).

An XML value cast to an SQL type gives the text of its one item: a
document node's children, an element's text, or, of an element a
validation typed, its typed value ([XMLVALIDATE](#xmlvalidate)). That text is read as a
value of the XML Schema type the SQL type maps to, restricted as the
table below shows, its white space collapsed first unless the type is a
string type. The value is then cast to the SQL type. A value the
restriction does not take fails with SQLSTATE 10000 where CAST would
round, pad or cut it: CHAR(5) takes text of five characters alone, and
NUMERIC(6,1) no second digit of fraction.

| SQL type | XML Schema type, and what it takes |
|---|---|
| CHARACTER(n) | `xs:string` of n characters; at most n of a length in OCTETS |
| CHARACTER VARYING(n), CLOB(n) | `xs:string` of at most n characters |
| BINARY(n), VARBINARY(n), BLOB(n) | `xs:hexBinary` of at most n octets; `xs:base64Binary` where XMLBINARY USING BASE64 is in scope |
| NUMERIC(p,s), DECIMAL(p,s) | `xs:decimal` of at most p digits, s after the point |
| INTEGER, SMALLINT, BIGINT | `xs:integer` within the type's range |
| REAL, DOUBLE PRECISION, FLOAT | `xs:float` or `xs:double` |
| BOOLEAN | `xs:boolean` |
| DATE | `xs:date` of four digits of year and two of month and day |
| TIME(p), TIMESTAMP(p) | `xs:time`, `xs:dateTime` of at most p digits of fraction, zeros ending it not counted; WITH TIME ZONE, with a time zone |
| INTERVAL | `xdt:yearMonthDuration` or `xdt:dayTimeDuration`, each field written as XMLCAST writes it: the first of the leading precision's digits, the others of two, the seconds' fraction of the fractional precision's, none if it is 0: `PT1234.500S` for INTERVAL SECOND(4, 3), `PT01H02M03S` for INTERVAL HOUR TO SECOND(0) |

A date or time with a time zone is moved to UTC for a type without one.
A time or timestamp is checked as XMLCAST writes a value of its type, its
fraction padded with zeros, so TIMESTAMP takes 14:30:00 and 14:30:00.5,
and TIMESTAMP(0) refuses 14:30:00.5. Text that is no such value, or more
than one item, fails with SQLSTATE 10000; a value with no item is null.

```sql
SELECT ID, XMLCAST(BODY AS VARCHAR(20)) FROM ORDERS ORDER BY ID;
VALUES (XMLSERIALIZE(XMLCAST(CAST(1.50 AS NUMERIC(5,2)) AS XML(CONTENT)) AS VARCHAR(20)), XMLSERIALIZE(XMLCAST(CAST(1.50 AS NUMERIC(5,2)) AS XML) AS VARCHAR(20)));
VALUES (XMLCAST(XMLPARSE(CONTENT '<total currency="EUR"> 17.25 </total>' PRESERVE WHITESPACE) AS NUMERIC(6,2)));
VALUES (XMLCAST(XMLPARSE(CONTENT '<due>2026-11-17T09:30:00-05:00</due>' PRESERVE WHITESPACE) AS TIMESTAMP));
VALUES (XMLCAST(XMLPARSE(CONTENT '<total currency="EUR"> 17.25 </total>' PRESERVE WHITESPACE) AS NUMERIC(6,1)));
VALUES (XMLCAST(XMLPARSE(CONTENT '<qty>many</qty>' PRESERVE WHITESPACE) AS INTEGER));
```

```
           ID |
--------------+----------------------
            1 | teacups
            3 |
(2 rows)
                      |
----------------------+----------------------
 1.50                 | 1.5
(1 row)

----------
    17.25
(1 row)

----------------------------
 2026-11-17 14:30:00.000000
(1 row)
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error: '17.25' is not valid for NUMERIC(6,1): its 2 digits break its fractionDigits [Schema2 §4.3.12.3] [Schema2 §4.1.4] [XQueryFO §17.1.1] err:FORG0001 [ISO/IEC 9075-14:2006 draft §6.6 GR3d vi, GR4j]
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error: 'many' is not valid for INTEGER: it is no literal of xs:integer [Schema2 §4.1.4] [XQueryFO §17.1.1] err:FORG0001 [ISO/IEC 9075-14:2006 draft §6.6 GR3d vi, GR4j]
```

## XMLDOCUMENT

```
XMLDOCUMENT ( <XML value expression> [ RETURNING { CONTENT | SEQUENCE } ] )
```

XMLDOCUMENT makes a new document node whose children are copies of the
value's nodes; a document node of the value gives its children instead.
Adjacent atomic values become one text node, a space apart, and adjacent
text is joined. An attribute standing alone among the value's items
fails with SQLSTATE 10000. XMLDOCUMENT of the null value is null.

RETURNING CONTENT, the default, makes a value of XML(CONTENT(UNTYPED));
RETURNING SEQUENCE one of XML(SEQUENCE). A document node is not always
a document, so a DOCUMENT column refuses one that is not, as the
example below shows.

## IS DOCUMENT and IS CONTENT

```
<XML value expression> IS [ NOT ] DOCUMENT
<XML value expression> IS [ NOT ] CONTENT
```

IS DOCUMENT is true of a document: one document node whose children are
one element, with comments and processing instructions beside it. IS
CONTENT is true of one document node. Each is unknown of the null value.
Either may follow WHEN in a simple CASE, testing its operand.

```sql
CREATE TABLE NOTES (ID INTEGER, BODY XML);
INSERT INTO NOTES VALUES (1, XMLPARSE(DOCUMENT '<a/>' PRESERVE WHITESPACE)), (2, XMLPARSE(CONTENT 'note <b/>' PRESERVE WHITESPACE)), (3, NULL);
SELECT ID, BODY IS DOCUMENT, BODY IS CONTENT FROM NOTES ORDER BY ID;
SELECT ID, CASE BODY WHEN IS DOCUMENT THEN 'document' WHEN IS CONTENT THEN 'content' END FROM NOTES ORDER BY ID;
SELECT XMLSERIALIZE(XMLDOCUMENT(BODY) AS VARCHAR(40)) FROM NOTES WHERE ID = 2;
INSERT INTO ORDERS SELECT 4, XMLDOCUMENT(BODY) FROM NOTES WHERE ID = 2;
```

```
OK
OK
           ID |       |
--------------+-------+-------
            1 | TRUE  | TRUE
            2 | FALSE | TRUE
            3 | NULL  | NULL
(3 rows)
           ID |
--------------+----------
            1 | document
            2 | content
            3 | NULL
(3 rows)

------------------------------------------
 note <b/>
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2200L): the value of XML(DOCUMENT(ANY)) is not an XML document [ISO/IEC 9075:1992 §13.8 GR4b iii; ISO/IEC 9075-14:2006 draft §10.2 GR1a]
```

## Publishing XML

The publishing operators make XML values of SQL values.

```
XMLELEMENT ( NAME <XML element name>
             [ , XMLNAMESPACES ( <XML namespace declaration item> [ , ... ] ) ]
             [ , XMLATTRIBUTES ( <value expression> [ AS <XML attribute name> ] [ , ... ] ) ]
             [ { , <value expression> }... [ OPTION <XML content option> ] ]
             [ RETURNING { CONTENT | SEQUENCE } ] )
XMLFOREST ( [ XMLNAMESPACES ( ... ) , ] <value expression> [ AS <forest element name> ] [ , ... ]
            [ OPTION <XML content option> ] [ RETURNING { CONTENT | SEQUENCE } ] )
XMLCONCAT ( <XML value expression> , <XML value expression> [ , ... ] [ RETURNING { CONTENT | SEQUENCE } ] )
XMLCOMMENT ( <character value expression> [ RETURNING { CONTENT | SEQUENCE } ] )
XMLPI ( NAME <XML PI target> [ , <character value expression> ] [ RETURNING { CONTENT | SEQUENCE } ] )
XMLTEXT ( <character value expression> [ RETURNING { CONTENT | SEQUENCE } ] )

<XML namespace declaration item> ::= <XML namespace URI> AS <XML namespace prefix>
                                   | DEFAULT <XML namespace URI> | NO DEFAULT
<XML content option> ::= NULL ON NULL | EMPTY ON NULL | ABSENT ON NULL
                       | NIL ON NULL | NIL ON NO CONTENT
```

XMLELEMENT makes an element. Its name, and each attribute's, is an
identifier: a regular identifier's in upper case, a delimited one's as
written, with a prefix if it has a colon. An attribute or a forest
element named by no AS must be a column, and takes the column's name.
An attribute whose value is null is left out. Each value of the content
becomes the element's content: an XML value's nodes, copied, or another
value's text. A character string is escaped as text; a number, a
datetime, an interval or a truth value is written as XML Schema writes
it; a binary string is written in hex, or in base64 where XMLBINARY
USING BASE64 is in scope. An attribute's value is of a
predefined type other than XML, or of a distinct type of one.

Content may be an array or a multiset too, or a value of a distinct type
whose source is one. Each element becomes an element named `element`,
holding its own value written as content is; a null element becomes
`<element xsi:nil="true"/>`, which declares the prefix `xsi`. An element
that is a row writes an element of each field that is not null, named by
the field's name as an XML name, its characters an XML name may not hold
escaped as `_xHHHH_`. An element that is an XML value is written as
XMLSERIALIZE writes it. A collection of XML(SEQUENCE), XML alone among
them, of a structured or a reference type, or of DECFLOAT, and content
of a row, are refused (SQLCODE -103).

The content option says what an element of content all null is: EMPTY
ON NULL, the default, an empty element; NULL ON NULL the null value;
ABSENT ON NULL nothing; NIL ON NULL an empty element with the attribute
`xsi:nil="true"`. NIL ON NO CONTENT gives that attribute to an element
with no element or text in its content.

XMLFOREST makes an element of each value, named as attributes are, and
gives them in order. Its content option applies to each element; NULL
ON NULL is its default, so a forest of null values alone is null.

XMLNAMESPACES binds prefixes to namespaces for the names of the element
or forest it is in and of the operators within it, inside those a query,
a statement or a definition declares
([XML options](#xml-options-of-a-scope)). DEFAULT names the
namespace of a name with no prefix, and NO DEFAULT none. The prefixes
`xml`, `xs`, `xsi` and `sqlxml` are bound unless declared otherwise.
Each element declares the namespaces its names use.

XMLCONCAT gives the items of each value that is not null, in order, and
is null if every value is. XMLCOMMENT makes a comment, which may not
hold two hyphens together or end with one (SQLSTATE 2200S). XMLPI makes
a processing instruction of a target and text, its leading white space
removed, which may not hold `?>` (SQLSTATE 2200T); its target may not be
`xml`. XMLTEXT makes a text node. A character no XML 1.1 text holds is
SQLSTATE 0N002.

Each operator is RETURNING CONTENT by default: its value is a document
node of what it makes, of XML(CONTENT(UNTYPED)), which a DOCUMENT or
CONTENT column takes. RETURNING SEQUENCE gives what it makes itself, of
XML(SEQUENCE).

```sql
CREATE TABLE STAFF (ID INTEGER, NAME VARCHAR(20), PAY DECIMAL(7,2), PHOTO VARBINARY(4));
INSERT INTO STAFF VALUES (1, 'Ann', 1200.50, X'0FB7'), (2, 'Bo & Co', NULL, NULL);
SELECT XMLSERIALIZE(XMLELEMENT(NAME "emp", XMLATTRIBUTES(ID AS "id"), XMLELEMENT(NAME "name", NAME), XMLFOREST(PAY, PHOTO)) AS VARCHAR(100)) FROM STAFF ORDER BY ID;
SELECT XMLSERIALIZE(XMLELEMENT(NAME emp, PAY OPTION NIL ON NULL) AS VARCHAR(100)) FROM STAFF ORDER BY ID;
SELECT XMLSERIALIZE(XMLELEMENT(NAME "h:p", XMLNAMESPACES('http://www.w3.org/1999/xhtml' AS "h"), XMLCOMMENT('pay'), XMLTEXT('see '), XMLPI(NAME "page", 'next')) AS VARCHAR(200)) FROM STAFF WHERE ID = 1;
SELECT XMLSERIALIZE(XMLCONCAT(XMLELEMENT(NAME "a"), XMLELEMENT(NAME "b")) AS VARCHAR(20)) FROM STAFF WHERE ID = 1;
SELECT XMLCOMMENT('a--b') FROM STAFF;
```

```
OK
OK

--------------------------------------------------------------
 <emp id="1"><name>Ann</name><PAY>1200.50</PAY><PHOTO>0FB7</PHOTO></emp>
 <emp id="2"><name>Bo &amp; Co</name></emp>
(2 rows)

--------------------------------------------------------------
 <EMP>1200.50</EMP>
 <EMP xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:nil="true"/>
(2 rows)

--------------------------------------------------------------
 <h:p xmlns:h="http://www.w3.org/1999/xhtml"><!--pay-->see <?page next?></h:p>
(1 row)

----------------------
 <a/><b/>
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2200S): the comment holds two hyphens together, ends with one, or holds a character XML does not [XML 1.1 §2.5 [15]] [ISO/IEC 9075-14:2006 draft §6.11 GR3]
```

Arrays, multisets and rows inside them are published as their elements:

```sql
SELECT XMLSERIALIZE(XMLELEMENT(NAME "pays", ARRAY[PAY, PAY * 2]) AS VARCHAR(200)) FROM STAFF ORDER BY ID;
SELECT XMLSERIALIZE(XMLFOREST(MULTISET[NAME] AS "names", ARRAY[CAST(ROW(ID, NAME) AS ROW("id" INTEGER, "full name" VARCHAR(20)))] AS "rows") AS VARCHAR(200)) FROM STAFF WHERE ID = 1;
SELECT XMLELEMENT(NAME "r", ROW(ID, NAME)) FROM STAFF;
```
```

--------------------------------------------------------------
 <pays><element>1200.50</element><element>2401.00</element></pays>
 <pays><element xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:nil="true"/><element xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:nil="true"/></pays>
(2 rows)

--------------------------------------------------------------
 <names><element>Ann</element></names><rows><element><id>1</id><full_x0020_name>Ann</full_x0020_name></element></rows>
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): a value of ROW(F1 INTEGER, F2 CHARACTER VARYING(20)) is not mapped to XML here [ISO/IEC 9075-14:2006 draft §6.14 SR7] (at character 7)
```

## XMLAGG

```
XMLAGG ( <XML value expression> [ ORDER BY <sort specification list> ]
         [ RETURNING { CONTENT | SEQUENCE } ] )
```

XMLAGG is a set function. It gives the XML values of a group, in the
order of its sort keys, one after another; values that are null are left
out, and a group of no value not null gives the null value. Without
ORDER BY the order is that in which the rows are read. RETURNING CONTENT,
the default, makes a document node of them; RETURNING SEQUENCE gives them
as a sequence. XMLAGG may be the aggregate of a window function. Values
that together hold more than 64 MiB fail with SQLSTATE 2200R.

```sql
SELECT XMLSERIALIZE(XMLELEMENT(NAME "staff", XMLAGG(XMLELEMENT(NAME "emp", XMLATTRIBUTES(ID AS "id"), NAME) ORDER BY NAME DESC)) AS VARCHAR(200)) FROM STAFF;
```

```

--------------------------------------------------------------
 <staff><emp id="2">Bo &amp; Co</emp><emp id="1">Ann</emp></staff>
(1 row)
```

## XML in other types

An array or a multiset may hold XML values, of any XML type or of a
distinct type of one, and so may a field of a row and an attribute of a
structured type. A distinct type may have an XML type as its source:
`CREATE TYPE LEAF AS XML(DOCUMENT(ANY)) FINAL`. Its function to its
source is named XML, as `XML(COVER)` below, and CAST takes a value to the
type and back. A routine's parameter or result may be of any of them.

Each XML value inside is assigned as one stored in a column of its
type is, so an array of XML(DOCUMENT) refuses an element that is not a
document (SQLSTATE 2200L). None of these is ever compared: an array,
multiset, row or distinct type holding XML is refused wherever an XML
value is (SQLCODE -103), and so are DISTINCT, SET, IS A SET, MEMBER,
SUBMULTISET and the multiset operators but MULTISET UNION ALL over a
multiset of XML.

```sql
CREATE TYPE LEAF AS XML(DOCUMENT(ANY)) FINAL;
CREATE TABLE BOOKS (ID INTEGER, PAGES XML(CONTENT(ANY)) ARRAY[10], COVER LEAF, META ROW(TITLE VARCHAR(20), BLURB XML(CONTENT(ANY))));
INSERT INTO BOOKS VALUES (1, ARRAY[XMLPARSE(CONTENT '<p>One</p>' PRESERVE WHITESPACE), XMLPARSE(CONTENT '<p>Two</p>' PRESERVE WHITESPACE)], CAST(XMLPARSE(DOCUMENT '<cover/>' PRESERVE WHITESPACE) AS LEAF), ROW('Tales', XMLPARSE(CONTENT 'A <b>bold</b> start' PRESERVE WHITESPACE)));
SELECT ID, CARDINALITY(PAGES), XMLSERIALIZE(PAGES[2] AS VARCHAR(20)), XMLSERIALIZE(BOOKS.META.BLURB AS VARCHAR(30)) FROM BOOKS;
SELECT XMLSERIALIZE(XML(COVER) AS VARCHAR(20)) FROM BOOKS;
INSERT INTO BOOKS (ID, COVER) VALUES (2, CAST(XMLPARSE(CONTENT 'no root' PRESERVE WHITESPACE) AS LEAF));
SELECT ID FROM BOOKS WHERE PAGES = PAGES;
```
```
OK
OK
OK
           ID |              |                      |
--------------+--------------+----------------------+--------------------------------
            1 |            2 | <p>Two</p>           | A <b>bold</b> start
(1 row)

----------------------
 <cover/>
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2200L): the value of XML(DOCUMENT(ANY)) is not an XML document [ISO/IEC 9075-14:2006 draft §6.5 GR1a]
turborel: SQLCODE -103 (SQLSTATE 2A000): the comparison compares values of XML(CONTENT(ANY)) ARRAY[10], and XML values are not comparable [ISO/IEC 9075-14:2006 draft §10.8 SR1] (at character 27)
```

## Querying XML

XMLQUERY, XMLEXISTS and XMLTABLE evaluate a query of XQuery 1.0, the
language of the W3C Candidate Recommendation of 3 November 2005, over
XML values and SQL values passed to it.

```
XMLQUERY ( <XQuery expression>
           [ PASSING { BY REF | BY VALUE } <argument> [ , <argument> ]... ]
           [ RETURNING { CONTENT | SEQUENCE } [ BY REF | BY VALUE ] ]
           { NULL | EMPTY } ON EMPTY )
XMLEXISTS ( <XQuery expression> [ PASSING { BY REF | BY VALUE } <argument> [ , ... ] ] )
XMLTABLE ( [ XMLNAMESPACES ( ... ) , ] <row pattern>
           [ PASSING { BY REF | BY VALUE } <argument> [ , ... ] ]
           COLUMNS <column> [ , <column> ]... ) [ AS ] <correlation name> [ ( <column name list> ) ]

<argument> ::= <value expression> [ AS <identifier> ] [ BY REF | BY VALUE ]
<column>   ::= <column name> FOR ORDINALITY
             | <column name> <data type> [ BY REF | BY VALUE ] [ DEFAULT <default option> ] [ PATH <column pattern> ]
```

The XQuery expression, the row pattern and each column pattern are
character string literals. The expression is parsed and analysed as the
statement is prepared, so a syntax error or a name it does not know
fails then. Every error the query raises, as it is analysed or
evaluated, is SQLSTATE 10000, XQuery error, its message naming the
error's code, such as `err:XPST0008`.

### Arguments

An argument with no name is the context item, `.`, of which there is at
most one. One with a name is a variable, `$name`; a regular identifier is
upper case, so `AS n` is `$N`, and a delimited one, `AS "n"`, keeps its
case. An XML value is passed BY REF, its nodes themselves, or BY VALUE,
a copy; the mechanism after PASSING applies to every argument that
writes none. Another value is cast to XML(SEQUENCE) as XMLCAST casts it,
and writes no mechanism. A null context item makes the result null; a
null variable is the empty sequence. A context item of more than one
item fails with SQLSTATE 2200V.

The XMLNAMESPACES declarations in scope, of an XMLELEMENT, XMLFOREST or
XMLTABLE around the query, or of the query, statement or definition it
is in ([XML options](#xml-options-of-a-scope)), are the query's
namespaces too, beside `xml`, `xs`, `xsi`, `fn`, `xdt`, `local` and
`sqlxml`.

### XMLQUERY

XMLQUERY gives the query's value. RETURNING SEQUENCE gives it as it is,
of XML(SEQUENCE), BY REF or a copy BY VALUE; it needs a mechanism, its
own or the one after PASSING. RETURNING CONTENT, the default, gives a
document node of it, as XMLDOCUMENT does, of XML(CONTENT(UNTYPED)). NULL
ON EMPTY makes the empty sequence null, EMPTY ON EMPTY keeps it.

```sql
CREATE TABLE ORDERS2 (ID INTEGER, DOC XML);
INSERT INTO ORDERS2 VALUES (1, XMLPARSE(DOCUMENT '<order><item price="10">pen</item><item price="25">ink</item></order>' STRIP WHITESPACE)), (2, XMLPARSE(DOCUMENT '<order><item price="5">cap</item></order>' STRIP WHITESPACE));
SELECT ID, XMLSERIALIZE(XMLQUERY('<total items="{count(//item)}">{//item[last()]/text()}</total>' PASSING BY REF DOC NULL ON EMPTY) AS VARCHAR(60)) FROM ORDERS2 ORDER BY ID;
SELECT ID, XMLCAST(XMLQUERY('(for $i in //item where $i/@price > $min return string($i))[last()]' PASSING BY VALUE DOC, 8 AS "min" RETURNING SEQUENCE NULL ON EMPTY) AS VARCHAR(20)) FROM ORDERS2 ORDER BY ID;
SELECT XMLQUERY('$x div 0' PASSING BY REF 1 AS "x" EMPTY ON EMPTY) FROM ORDERS2;
```
```
OK
OK
           ID |
--------------+--------------------------------------------------------------
            1 | <total items="2">ink</total>
            2 | <total items="1">cap</total>
(2 rows)
           ID |
--------------+----------------------
            1 | ink
            2 | NULL
(2 rows)
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error err:FOAR0001: a divisor is zero [XQueryFO §6.2.4] [ISO/IEC 9075-14:2006 draft §6.18 GR3]
```

### XMLEXISTS

XMLEXISTS is true if the query's value is not the empty sequence, false
if it is, and unknown if the context item passed is null. It is a
predicate, and a boolean value.

```sql
SELECT ID FROM ORDERS2 WHERE XMLEXISTS('//item[. = $name]' PASSING BY REF DOC, 'ink' AS "name");
SELECT ID, XMLEXISTS('/order[count(item) > 1]' PASSING BY REF DOC) FROM ORDERS2 ORDER BY ID;
```
```
           ID
--------------
            1
(1 row)
           ID |
--------------+-------
            1 | TRUE
            2 | FALSE
(2 rows)
```

### XMLTABLE

XMLTABLE is a table of FROM. It may refer to the columns of the tables
before it, as a lateral derived table does. Each item of its row
pattern's value is a row. FOR ORDINALITY numbers the rows from 1. A
regular column's pattern, PATH or the column's name, is evaluated with
the row's item as the context item. Its value is the empty sequence's
DEFAULT, else null; or it is cast to the column's type: of XML(SEQUENCE),
the value as it is, passed by the column's mechanism or the one after
PASSING; of another XML type, a document node of it, copied; of another
type, XMLCAST of a document node of it. An attribute cannot be made a
document node, so a column of an attribute's value writes `data(@name)`,
not `@name`.

```sql
SELECT O.ID, I.N, I.NAME, I.PRICE FROM ORDERS2 O, XMLTABLE('/order/item' PASSING BY REF O.DOC COLUMNS N FOR ORDINALITY, NAME VARCHAR(10) PATH '.', PRICE DECIMAL(5,2) DEFAULT 0 PATH 'data(@price)') AS I ORDER BY O.ID, I.N;
SELECT * FROM XMLTABLE(XMLNAMESPACES('urn:t' AS "t"), '<t:r xmlns:t="urn:t"><t:v>1</t:v><t:v>2</t:v></t:r>/t:v' COLUMNS V INTEGER PATH '.') AS X;
```
```
           ID |            N | NAME       |   PRICE
--------------+--------------+------------+---------
            1 |            1 | pen        |   10.00
            1 |            2 | ink        |   25.00
            2 |            1 | cap        |    5.00
(3 rows)
            V
--------------
            1
            2
(2 rows)
```

### What XQuery evaluates

Every expression of XQuery 1.0 is evaluated: paths on every axis, in
document order, predicates, sequences and ranges, arithmetic and
comparisons, FLWOR expressions with order by, quantified, conditional
and typeswitch expressions, instance of, treat, cast and castable, direct
and computed constructors, ordered and unordered expressions, validate
expressions ([below](#the-validate-expression)), and a prolog's
namespaces, setters, variables and functions, recursive ones too. Static
typing is not done, so a type error is found as the query is evaluated.

A prolog may import a registered XML Schema
([below](#schema-imports)), whose types and declarations a query then
names. A prolog may import a library module registered by CREATE XQUERY
MODULE ([below](#xquery-modules)) and call its functions. A library
module given as a query is refused with `err:XPST0003`, as a query is a
main module.

An element a query constructs has the type annotation `xs:anyType`, and
an element it copies keeps its own, a validated element its type; after
`declare construction strip;` each is `xdt:untyped`. An element test
such as `element(*, xdt:untyped)` tells them apart. XMLELEMENT and
XMLFOREST RETURNING SEQUENCE make an element of `xs:anyType` too. A
value of XML(CONTENT(UNTYPED)), as RETURNING CONTENT makes, has every
element `xdt:untyped`.

```sql
SELECT XMLCAST(XMLQUERY('<a/> instance of element(*, xdt:untyped)' NULL ON EMPTY) AS BOOLEAN), XMLCAST(XMLQUERY('declare construction strip; <a/> instance of element(*, xdt:untyped)' NULL ON EMPTY) AS BOOLEAN) FROM ORDERS2 WHERE ID = 1;
SELECT XMLCAST(XMLQUERY('./* instance of element(*, xdt:untyped)' PASSING BY REF XMLELEMENT(NAME "e") NULL ON EMPTY) AS BOOLEAN) FROM ORDERS2 WHERE ID = 1;
```
```
       |
-------+-------
 FALSE | TRUE
(1 row)

-------
 TRUE
(1 row)
```

A cast to `xs:QName`, and its constructor function, takes a string
literal alone, as `xs:QName("h:td")` or `"td" cast as xs:QName`; any
other operand fails as the query is prepared, with `err:XPST0083`. The
prefix is resolved by the query's namespaces, those of its prolog and
of the XMLNAMESPACES in scope among them, a name with none by the
default element namespace; a prefix bound to none fails with
`err:FONS0004`.

```sql
WITH XMLNAMESPACES ('urn:shop' AS "s") SELECT XMLCAST(XMLQUERY('namespace-uri-from-QName(xs:QName("s:item"))' NULL ON EMPTY) AS VARCHAR(20)) FROM ORDERS2 WHERE ID = 1;
SELECT XMLQUERY('xs:QName(concat("s", ":item"))' NULL ON EMPTY) FROM ORDERS2 WHERE ID = 1;
```
```

----------------------
 urn:shop
(1 row)
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error err:XPST0083: a value cast to xs:QName, or a type derived from it or xs:NOTATION, is a string literal [XQuery §3.12.3, §3.12.5] [ISO/IEC 9075-14:2006 draft §6.18 GR3] (at character 7)
```

### Functions

A query calls every function of XQuery 1.0 Functions and Operators, the
W3C Candidate Recommendation of 3 November 2005, by its name in the
namespace `fn`, the default, so `count(//item)` is `fn:count(//item)`.
Each converts its arguments as its signature says: a node's typed value
where a value is wanted, an untyped value cast to the type wanted, an
integer promoted to a decimal or double, and the empty sequence where
`?` allows it.

| Kind | Functions |
|---|---|
| Accessors | `node-name`, `nilled`, `string`, `data`, `base-uri`, `document-uri` |
| Errors and tracing | `error`, `trace` |
| Constructing | `dateTime`, and `xs:TYPE($arg)` of each atomic type, which casts |
| Numbers | `abs`, `ceiling`, `floor`, `round`, `round-half-to-even` |
| Strings | `codepoints-to-string`, `string-to-codepoints`, `compare`, `codepoint-equal`, `concat`, `string-join`, `substring`, `string-length`, `normalize-space`, `normalize-unicode`, `upper-case`, `lower-case`, `translate`, `encode-for-uri`, `iri-to-uri`, `escape-html-uri` |
| Substrings | `contains`, `starts-with`, `ends-with`, `substring-before`, `substring-after` |
| Regular expressions | `matches`, `replace`, `tokenize` |
| URIs | `resolve-uri` |
| Booleans | `true`, `false`, `not`, `boolean` |
| Durations | `years-from-duration`, `months-from-duration`, `days-from-duration`, `hours-from-duration`, `minutes-from-duration`, `seconds-from-duration` |
| Dates and times | `year-from-dateTime`, `month-from-dateTime`, `day-from-dateTime`, `hours-from-dateTime`, `minutes-from-dateTime`, `seconds-from-dateTime`, `timezone-from-dateTime`, `year-from-date`, `month-from-date`, `day-from-date`, `timezone-from-date`, `hours-from-time`, `minutes-from-time`, `seconds-from-time`, `timezone-from-time`, `adjust-dateTime-to-timezone`, `adjust-date-to-timezone`, `adjust-time-to-timezone` |
| QNames | `resolve-QName`, `QName`, `prefix-from-QName`, `local-name-from-QName`, `namespace-uri-from-QName`, `namespace-uri-for-prefix`, `in-scope-prefixes` |
| Nodes | `name`, `local-name`, `namespace-uri`, `number`, `lang`, `root` |
| Sequences | `index-of`, `empty`, `exists`, `distinct-values`, `insert-before`, `remove`, `reverse`, `subsequence`, `unordered`, `zero-or-one`, `one-or-more`, `exactly-one`, `deep-equal`, `count`, `avg`, `max`, `min`, `sum` |
| Documents and IDs | `id`, `idref`, `doc`, `doc-available`, `collection` |
| The context | `position`, `last`, `current-dateTime`, `current-date`, `current-time`, `implicit-timezone`, `default-collation`, `static-base-uri` |

No document or collection is available to a query, so `fn:doc` of a URI
fails with `err:FODC0005`, `fn:doc-available` is false, and
`fn:collection` fails with `err:FODC0004`, or `err:FODC0002` of none. The
current dateTime is the statement's, the same through it, in the
session's time zone, which is the implicit time zone. `fn:error` fails
with SQLSTATE 10000, its message the error's code and description;
`fn:trace` gives its value and writes nothing.

`xs:integer` and `xs:decimal` values have up to 1,024 digits, and an
operation past them fails with `err:FOAR0002`. A quotient of decimals is
exact when it ends within 18 digits after the point, or within the
scale of an operand, and is cut there otherwise. Floats and doubles
compute as IEEE 754 does. `fn:normalize-unicode` normalizes by Unicode
13.0, to NFC, NFD, NFKC, NFKD or FULLY-NORMALIZED. A fully normalized
string is its NFC, as XML 1.1 defines full normalization, the string
being its one construct; a string whose NFC begins with a composing
character, a combining mark or a character second in a canonical
composition, has none and fails with `err:FOCH0003`. `fn:upper-case` and `fn:lower-case` map each character
in full, as Unicode's tables do apart from language and context.
`fn:matches`, `fn:replace` and `fn:tokenize` read XQuery 1.0's regular
expressions, with the flags `s`, `m`, `i` and `x`.

```sql
SELECT XMLCAST(XMLQUERY('string-join(for $i in //item order by xs:decimal($i/@price) descending return upper-case($i), ", ")' PASSING BY REF DOC NULL ON EMPTY) AS VARCHAR(40)) FROM ORDERS2 WHERE ID = 1;
SELECT ID, XMLCAST(XMLQUERY('sum(//item/@price) div count(//item)' PASSING BY REF DOC NULL ON EMPTY) AS DECIMAL(9,2)) FROM ORDERS2 ORDER BY ID;
SELECT XMLCAST(XMLQUERY('replace("2026-11-17", "(\d+)-(\d+)-(\d+)", "$3.$2.$1")' NULL ON EMPTY) AS VARCHAR(20)), XMLCAST(XMLQUERY('tokenize("a, b,c", ",\s*")[last()]' NULL ON EMPTY) AS VARCHAR(5)) FROM ORDERS2 WHERE ID = 1;
SELECT XMLCAST(XMLQUERY('year-from-date(xs:date("2026-11-17") + xdt:yearMonthDuration("P1Y2M"))' NULL ON EMPTY) AS INTEGER), XMLCAST(XMLQUERY('days-from-duration(xs:date("2026-12-25") - xs:date("2026-11-17"))' NULL ON EMPTY) AS INTEGER) FROM ORDERS2 WHERE ID = 1;
SELECT XMLCAST(XMLQUERY('count(distinct-values(("b", "a", "B", "a")))' NULL ON EMPTY) AS INTEGER), XMLCAST(XMLQUERY('string-join(for $n in (round-half-to-even(2.5), round(2.5), abs(-3)) return string($n), " ")' NULL ON EMPTY) AS VARCHAR(20)) FROM ORDERS2 WHERE ID = 1;
SELECT XMLQUERY('exactly-one(())' EMPTY ON EMPTY) FROM ORDERS2 WHERE ID = 1;
SELECT XMLCAST(XMLQUERY('string-join(for $c in string-to-codepoints(normalize-unicode("e&#x301;t", "FULLY-NORMALIZED")) return string($c), " ")' NULL ON EMPTY) AS VARCHAR(20)) FROM ORDERS2 WHERE ID = 1;
SELECT XMLQUERY('normalize-unicode("&#x301;e", "FULLY-NORMALIZED")' NULL ON EMPTY) FROM ORDERS2 WHERE ID = 1;
```
```

------------------------------------------
 INK, PEN
(1 row)
           ID |
--------------+-------------
            1 |       17.50
            2 |        5.00
(2 rows)
                      |
----------------------+-------
 17.11.2026           | c
(1 row)
              |
--------------+--------------
         2028 |           38
(1 row)
              |
--------------+----------------------
            3 | 2 3 3
(1 row)
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error err:FORG0005: fn:exactly-one is given 0 items [XQueryFO §15.2.3] [ISO/IEC 9075-14:2006 draft §6.18 GR3]

----------------------
 233 116
(1 row)
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error err:FOCH0003: a string beginning with a composing character has no fully normalized form [XQueryFO §7.4.6] [XML 1.1 Appendix B] [ISO/IEC 9075-14:2006 draft §6.18 GR3]
```

Strings compare by Unicode code points unless the query declares
another collation or a function is given one. A collation of the
database is the URI `urn:turborel:collation:` then its name, as
`urn:turborel:collation:UNICODE`; another fails with `err:FOCH0002`.
`fn:contains`, `fn:starts-with`, `fn:ends-with`, `fn:substring-before`
and `fn:substring-after` match by the collation's collation units. Of
the code point collation, and of one that compares one character at a
time as `CASE_INSENSITIVE` does, each character is a unit. Of `UNICODE`
and its tailorings a unit is a character and the marks after it, or a
contraction, as `ch` is in `UNICODE_CS`, so a match never splits one: a
precomposed `é` matches `e` and a combining acute accent, and `c` is
not found in `chata` by `UNICODE_CS`. A unit matches another of the same
weights at every level the collation compares, so `UNICODE` tells case
and accents apart. A match is minimal: it begins and ends at a unit of
some weight, so a character ignored at every level, as a control
character is, is left outside it.

```sql
SELECT XMLCAST(XMLQUERY('compare("pen", "PEN", "urn:turborel:collation:CASE_INSENSITIVE")' NULL ON EMPTY) AS INTEGER), XMLCAST(XMLQUERY('contains("Pen and Ink", "INK", "urn:turborel:collation:CASE_INSENSITIVE")' NULL ON EMPTY) AS BOOLEAN) FROM ORDERS2 WHERE ID = 1;
SELECT XMLCAST(XMLQUERY('contains("re&#x301;sume&#x301;", "&#xE9;s", "urn:turborel:collation:UNICODE")' NULL ON EMPTY) AS BOOLEAN), XMLCAST(XMLQUERY('declare default collation "urn:turborel:collation:UNICODE_CS"; concat(contains("chata", "c"), " ", substring-after("ochota", "ch"))' NULL ON EMPTY) AS VARCHAR(20)) FROM ORDERS2 WHERE ID = 1;
```
```
              |
--------------+-------
            0 | TRUE
(1 row)
       |
-------+----------------------
 TRUE  | false ota
(1 row)
```

## XML indexes

```
CREATE INDEX <index name> ON <table name> ( <XML column name> )
    USING { XML_PATH | XML_VALUE | XML_PROPERTY | XML_PRIMARY }
```

An XML index keys the nodes of an XML column's values, so a query of a
path reads the rows whose values hold it, not every row
([CREATE INDEX](statements/create-index.md)). It keys each element and
attribute below each item of a value: its path from the item, as names in
namespaces; its value, the text of an element's descendants or an
attribute's, as a string, by its first 64 octets, and as a number if the
whole of it is one; and its place in document order. The methods order
their keys for different questions, after Pal et al., "Indexing XML Data
Stored in a Relational Database" (VLDB 2004):

| Method | Keyed by | Answers best |
|---|---|---|
| XML_PATH | path, value | a path, and a value at it, of any row |
| XML_VALUE | value, path | a value, at a path given in part |
| XML_PROPERTY | row, path, value | the values of paths of one row |
| XML_PRIMARY | row, place in document order; and row, path, value | a query of one large value, answered from the index alone |

XML_PRIMARY keeps each node whole: its kind, its path and its value up to
512 octets. It answers whether a row's value holds a path, a value at it
and a predicate of an ancestor, such as `/order[@no = 17]/item`, without
reading the value; a row whose answer needs a longer value is read and
checked. It suits large values queried for a few of their nodes; of
small values, reading the value costs less. It takes about twice the
space of the others.

Each indexes one XML column alone; another column, or more than one, is
refused (SQLCODE -103). Like every index, it is built of the rows there,
kept with each change, and changes no result. A value with more than
262,144 elements and attributes, or one of a name or path too long for
the database's page size, cannot be indexed, and its INSERT or UPDATE
fails (SQLCODE -900). Its lookups are also reached from Rust
(`Database::xml_candidates`).

```sql
COMMIT;
CREATE INDEX ORDER_PATHS ON ORDERS (BODY) USING XML_PATH;
CREATE INDEX ORDER_VALUES ON ORDERS (BODY) USING XML_VALUE;
CREATE INDEX ORDER_ROWS ON ORDERS (BODY) USING XML_PRIMARY;
CREATE INDEX ORDER_IDS ON ORDERS (ID) USING XML_PROPERTY;
DROP INDEX ORDER_VALUES;
SELECT INDEX_NAME, METHOD_NAME, COLUMN_NAMES FROM INFORMATION_SCHEMA.INDEXES WHERE TABLE_NAME = 'ORDERS' ORDER BY INDEX_NAME;
```

```
OK
OK
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): index ORDER_IDS of method xml_property keys one column, of an XML type (RULES.md D106)
OK
 INDEX_NAME                                                   | METHOD_NAME                                                  | COLUMN_NAMES
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 ORDER_PATHS                                                  | xml_path                                                     | BODY
 ORDER_ROWS                                                   | xml_primary                                                  | BODY
(2 rows)
```

### Queries an XML index reads for

The planner reads an XML index for a condition on a table's rows that
keeps a row only if a query's value of the row's column is not empty:

- XMLEXISTS, in WHERE or in a join's ON;
- XMLQUERY ... NULL ON EMPTY IS NOT NULL;
- XMLTABLE joined to the rows it is of, not by a LEFT join: a row its
  row pattern gives nothing of gives no row.

The query's context item is the column, of XML(DOCUMENT) or
XML(CONTENT), passed BY REF or BY VALUE, and nothing else is passed. Its
body is one path from `.` or `/` of child, descendant (`//`) and
attribute steps over names or `*`. Its last step may have one predicate:
a relative path of such steps, or a comparison of one, or of `.`, with a
literal. A comparison by `=`, `!=`, `<`, `<=`, `>` or `>=` with a string
or a number narrows the rows the index gives; any other leaves the path
alone. One step before the last, of child steps after it, may have a
predicate too, of an ancestor of the nodes selected: a relative path, or
its comparison with a string, as in `/order[@no = "17"]/item`. A query
of another shape, a column of XML(SEQUENCE), whose value may be no one
node, or a table too small to gain is read whole.

The index gives the rows whose values may hold such a node. Each is
fetched and the condition checked on it, so the result is a full read's.
A row whose value a numeric comparison cannot cast is among those it
gives, so its error is raised as by a full read. If another condition
of the table may raise an error, the table is read whole, so the error
is raised whatever rows the index would skip. In ON, a query that may
raise an error, as a numeric comparison may, is read whole too.

XML_PRIMARY answers of each row from the index alone a query of a path,
its comparisons with strings and its predicate of an ancestor: a row it
holds of is not checked, only one whose answer needs a value longer
than it keeps whole. Of the other methods, and of a query compared with
a number or by a value comparison, each row given is checked. The
planner weighs reading an index against reading every row and checking
its value, by the values' average length. EXPLAIN shows the index read
by its method, the path, predicate and comparison it looks up, and
whether it answers the query.

```sql
CREATE TABLE PARCELS (ID INTEGER, LABEL XML(DOCUMENT));
INSERT INTO PARCELS WITH RECURSIVE N (I) AS (SELECT CAST(1 AS INTEGER) FROM ORDERS WHERE ID = 1 UNION ALL SELECT I + 1 FROM N WHERE I < 400) SELECT I, XMLPARSE(DOCUMENT '<parcel><to>' || CAST(I AS VARCHAR(3)) || '</to>' || CASE WHEN I > 395 THEN '<fragile/>' ELSE '' END || '</parcel>' STRIP WHITESPACE) FROM N;
COMMIT;
CREATE INDEX PARCEL_PATHS ON PARCELS (LABEL) USING XML_PATH;
SELECT ID FROM PARCELS WHERE XMLEXISTS('/parcel[to = "7"]' PASSING BY REF LABEL) ORDER BY ID;
EXPLAIN SELECT ID FROM PARCELS WHERE XMLEXISTS('/parcel[to = "7"]' PASSING BY REF LABEL);
EXPLAIN SELECT P.ID FROM PARCELS P, XMLTABLE('//fragile' PASSING BY REF P.LABEL COLUMNS F FOR ORDINALITY) AS X;
```

```
OK
OK
OK
OK
           ID
--------------
            7
(1 row)
 PLAN
--------------------------------------------------------------
 -- bound:
 Project PARCELS.ID
   Filter XMLEXISTS(PARCELS.LABEL)
     Scan ANN.PARCELS
 -- rules: none
 -- optimized:
 Project PARCELS.ID
   Filter XMLEXISTS(PARCELS.LABEL)
     Scan ANN.PARCELS
 -- physical:
 Project PARCELS.ID
   Filter XMLEXISTS(PARCELS.LABEL)  (est. rows=0)
     IndexScan ANN.PARCELS using PARCEL_PATHS [xml_path]: PARCELS.LABEL has /parcel/to = '7'  (est. rows=1, cost=60.5)
(13 rows)
 PLAN
--------------------------------------------------------------
 -- bound:
 Project P.ID
   Unnest XMLTABLE(P.LABEL)
     Scan ANN.PARCELS
 -- rules: none
 -- optimized:
 Project P.ID
   Unnest XMLTABLE(P.LABEL)
     Scan ANN.PARCELS
 -- physical:
 Project P.ID
   Unnest of 1 collections
     IndexScan ANN.PARCELS using PARCEL_PATHS [xml_path]: P.LABEL has //fragile  (est. rows=10, cost=137.9)
(13 rows)
```

```sql
COMMIT;
DROP INDEX PARCEL_PATHS;
CREATE INDEX PARCEL_NODES ON PARCELS (LABEL) USING XML_PRIMARY;
SELECT ID FROM PARCELS WHERE XMLEXISTS('/parcel[to = "398"]/fragile' PASSING BY REF LABEL);
EXPLAIN SELECT ID FROM PARCELS WHERE XMLEXISTS('/parcel[to = "398"]/fragile' PASSING BY REF LABEL);
```

```
OK
OK
OK
           ID
--------------
          398
(1 row)
 PLAN
--------------------------------------------------------------
 -- bound:
 Project PARCELS.ID
   Filter XMLEXISTS(PARCELS.LABEL)
     Scan ANN.PARCELS
 -- rules: none
 -- optimized:
 Project PARCELS.ID
   Filter XMLEXISTS(PARCELS.LABEL)
     Scan ANN.PARCELS
 -- physical:
 Project PARCELS.ID
   IndexScan ANN.PARCELS using PARCEL_NODES [xml_primary]: PARCELS.LABEL has /parcel[to = '398']/fragile, answered by the index  (est. rows=6, cost=103.5)
(12 rows)
```

## XML options of a scope

```
<XML lexically scoped options> ::= <XML lexically scoped option> [ , <XML lexically scoped option> ]
<XML lexically scoped option> ::=
      XMLNAMESPACES ( <XML namespace declaration item> [ , ... ] )
    | XMLBINARY [ USING ] { BASE64 | HEX }

<with clause> ::= WITH [ <XML lexically scoped options> ] [ , ] [ [ RECURSIVE ] <with list> ]
DELETE [ WITH <XML lexically scoped options> ] FROM ...
INSERT [ WITH <XML lexically scoped options> ] INTO ...
MERGE [ WITH <XML lexically scoped options> ] INTO ...
UPDATE [ WITH <XML lexically scoped options> ] <table name> SET ...
CHECK ( [ WITH <XML lexically scoped options> ] <search condition> )
GENERATED ALWAYS AS ( [ WITH <XML lexically scoped options> ] <value expression> )
BEGIN [ [ NOT ] ATOMIC ] [ DECLARE <XML lexically scoped options> ; ] ... END
```

XMLNAMESPACES and XMLBINARY may be declared for a whole query, a whole
statement or a whole definition, as well as in one XMLELEMENT or
XMLFOREST. Each holds at most one XMLNAMESPACES and one XMLBINARY.

XMLNAMESPACES names the elements and attributes of XMLELEMENT and
XMLFOREST, and is among the namespaces of XMLQUERY's, XMLEXISTS's and
XMLTABLE's queries.

XMLBINARY says how XMLELEMENT, XMLFOREST, XMLCAST and XMLMAP of no
ENCODING write a binary string, and how XMLCAST reads one: BASE64 in
base64, HEX in hex. Where no XMLBINARY is in scope it is HEX. XMLPI always writes hex.

The options after WITH, before a query's WITH list or alone, are in
scope of the whole query: its subqueries, derived tables, the queries
of its WITH list and the operands of its UNION, EXCEPT and INTERSECT.
A comma comes between the options and the WITH list, and only there.
After the first word of DELETE, INSERT, MERGE or UPDATE, positioned or
searched, they are in scope of the whole statement. A CHECK, of a table,
a domain or an assertion, and a generation expression are in their
scope, and keep them with their text, so they mean the same whenever
the definition is read again. DECLARE as a compound statement begins
puts them in scope of all it holds. The innermost declaration of a name
wins: an XMLELEMENT's own, then each query's from the inside out, then
the statement's or the definition's, then the compound statements'.

A view keeps the options of its query, and a routine those of its body;
neither sees those of the statement that reads or invokes it.

```sql
WITH XMLBINARY USING BASE64 SELECT XMLSERIALIZE(XMLFOREST(ID, PHOTO) AS VARCHAR(60)) FROM STAFF WHERE ID = 1;
WITH XMLNAMESPACES('http://www.w3.org/1999/xhtml' AS "h"), XMLBINARY BASE64 SELECT XMLSERIALIZE(XMLELEMENT(NAME "h:img", XMLATTRIBUTES(PHOTO AS "h:src"), (SELECT XMLELEMENT(NAME "h:b", NAME) FROM STAFF WHERE ID = 2)) AS VARCHAR(200)) FROM STAFF WHERE ID = 1;
CREATE TABLE PICTURES (ID INTEGER, PHOTO VARBINARY(4), TAG VARCHAR(40) GENERATED ALWAYS AS (WITH XMLBINARY BASE64 XMLSERIALIZE(XMLELEMENT(NAME "img", PHOTO) AS VARCHAR(40))));
INSERT WITH XMLBINARY BASE64 INTO PICTURES (ID, PHOTO) VALUES (1, XMLCAST(XMLPARSE(CONTENT 'D7c=' PRESERVE WHITESPACE) AS VARBINARY(4)));
SELECT ID, PHOTO, TAG FROM PICTURES;
WITH XMLBINARY BASE64, XMLBINARY HEX SELECT ID FROM STAFF;
```

```

--------------------------------------------------------------
 <ID>1</ID><PHOTO>D7c=</PHOTO>
(1 row)

--------------------------------------------------------------
 <h:img xmlns:h="http://www.w3.org/1999/xhtml" h:src="D7c="><h:b>Bo &amp; Co</h:b></h:img>
(1 row)
OK
OK
           ID | PHOTO    | TAG
--------------+----------+------------------------------------------
            1 | 0FB7     | <img>D7c=</img>
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): the options declare namespaces once and a binary encoding once [ISO/IEC 9075-14:2006 draft §11.3 SR1] (at character 23)
```

## Mapping names

```
XMLNAME_TO_IDENTIFIER ( <character value expression> )
IDENTIFIER_TO_XMLNAME ( <character value expression> [ , { PARTIALLY | FULLY } ESCAPED ] )
```

*(extension)* An SQL identifier may hold characters an XML Name may
not, so SQL/XML maps identifiers to XML Names, as XMLELEMENT, XMLFOREST
and XMLMAP map column names, and XML Names back to identifiers. These
two functions apply the mappings to a string.

IDENTIFIER_TO_XMLNAME maps the characters of an identifier to an XML
Name. A character no XML 1.1 name holds, or a first character no name
begins with, is written `_xHHHH_` in hexadecimal, or `_xHHHHHH_` beyond
the Basic Multilingual Plane, and an underscore before a lowercase `x`
is written `_x005F_`. PARTIALLY ESCAPED, the default, keeps a colon
after the first character; FULLY ESCAPED escapes every colon and the
first letter of a leading `xml` in any case.

XMLNAME_TO_IDENTIFIER maps an XML Name back: each `_xHHHH_` or
`_xHHHHHH_` of an assigned Unicode character becomes that character,
any other stays as written. It gives the delimited identifier, in
double quotes, each double quote doubled. Either mapping read back
gives the identifier again.

Each takes a character string, not a large object, and gives a
CHARACTER VARYING string: of XMLNAME_TO_IDENTIFIER twice the argument's
length and 2, of IDENTIFIER_TO_XMLNAME 9 times it, at most 32,767
characters. A longer result fails with SQLSTATE 22001. A null argument
gives null. The words are no key words.

```sql
VALUES (IDENTIFIER_TO_XMLNAME('order line'), IDENTIFIER_TO_XMLNAME('ns:a', FULLY ESCAPED), IDENTIFIER_TO_XMLNAME('xml1', FULLY ESCAPED));
VALUES (XMLNAME_TO_IDENTIFIER('order_x0020_line'), XMLNAME_TO_IDENTIFIER('say_x0020__x0022_hi_x0022_'));
```
```
                                                              |                                      |
--------------------------------------------------------------+--------------------------------------+--------------------------------------
 order_x0020_line                                             | ns_x003A_a                           | _x0078_ml1
(1 row)
                                    |
------------------------------------+--------------------------------------------------------
 "order line"                       | "say ""hi"""
(1 row)
```

## Mapping tables to XML

XMLMAP maps a table, a schema or the catalog to XML, and to an XML
Schema document that describes that XML, as SQL/XML defines the
mappings. SQL/XML gives the mappings no syntax, so XMLMAP is
turborel's own. Its clauses are written in this order, each at most
once.

```
XMLMAP ( { TABLE <table name> | SCHEMA <schema name> | CATALOG }
         [ NULLS { ABSENT | NIL } ]
         [ AS { FOREST | ELEMENT } ]
         [ TARGETNAMESPACE <character string literal> ]
         [ DATA | METADATA | DATA AND METADATA ]
         [ ENCODING { BASE64 | HEX } ] )
```

DATA, the default, gives the data. A table is an element named after
it, holding an element `row` for each row, which holds an element for
each column named after the column. A value is written as XMLFOREST
writes it: a string's characters escaped, a number, a datetime, an
interval or a truth value as XML Schema writes it, an XML value as its
serialization. A schema is an element named after it, holding the
element of each of its tables, and the catalog an element named after
it (the setting CATALOG.NAME), holding the element of each schema.
Names are escaped as XMLELEMENT's are, a character a name cannot hold
written `_xHHHH_`. The rows are in the order the table is read, the
tables and schemas in order of name.

NULLS ABSENT, the default, leaves a null value's element out. NULLS NIL
writes it empty, with the attribute `xsi:nil="true"`. AS FOREST maps a
table to an element named after the table for each row, with no element
around them; AS ELEMENT, the default, maps it as above. TARGETNAMESPACE
puts the elements in a namespace; with none, or with `''`, they are in
none. ENCODING writes binary strings in HEX or BASE64; with none
written, they are in the XMLBINARY in scope, HEX outside every one.

METADATA gives the XML Schema document instead. It defines a type of
each column's data type, distinct type or domain, named after it, a type
of each table's rows, and, AS ELEMENT, of each table; then of the
schema and of the catalog, and declares the element at the root of the
data. DATA AND METADATA gives both: the schema document, then the data.

Only what the user may read is mapped. TABLE needs SELECT on the table
(SQLCODE -104). A schema or the catalog holds only the tables the user
may SELECT that have a column to map, and the catalog only the schemas
of such tables, the Information Schema among them. A column is left out
if it is of XML(SEQUENCE) (XML alone), a structured or reference type,
DECFLOAT or a plug-in's type, or of a row, array, multiset or distinct
type made of one, or of a distinct type whose cast to its source type
the user may not execute. A row is an element of each field, a null one
left out or nilled as NULLS says, and an array or a multiset an element
`element` of each element, a null one nilled. A mapping
that leaves out a column the user may read, as its type does not map,
completes with the warning 01010 (column cannot be mapped).

| Mapping | Value |
|---|---|
| DATA, AS ELEMENT, or of a schema or the catalog | XML(DOCUMENT(UNTYPED)): a document |
| DATA of a table AS FOREST | XML(CONTENT(UNTYPED)): the rows' elements |
| METADATA | XML(DOCUMENT(UNTYPED)): the XML Schema document |
| DATA AND METADATA | XML(SEQUENCE): the schema document, then the data |

A view whose query maps a table depends on the table, so DROP TABLE
RESTRICT is refused while it does. A view, a routine or a trigger whose
text maps a schema depends on the schema: DROP SCHEMA RESTRICT is
refused while it is there, and CASCADE drops it. A mapping of a schema
or of the catalog reads the tables there are as it runs, so a table
added later is mapped, and one dropped is not; a mapping of the catalog
depends on nothing that can be dropped. As a view that maps a schema
could be in the schema it maps, a mapping of a schema or the catalog
leaves out a view whose query maps a schema or the catalog, or reads a
view that does; one that meets such a mapping through a routine is
refused (SQLCODE -103). A CHECK constraint may not hold
XMLMAP (SQLCODE -103), as it reads rows in an order not determined. A
target namespace of the namespace of `xml` or `xmlns` is refused
(SQLCODE -103), and a schema that is not there is SQLSTATE 3F000.

```sql
VALUES (XMLSERIALIZE(CONTENT XMLMAP(TABLE STAFF) AS VARCHAR(400)));
VALUES (XMLSERIALIZE(CONTENT XMLMAP(TABLE STAFF NULLS NIL AS FOREST ENCODING BASE64) AS VARCHAR(400)));
WITH XMLBINARY USING BASE64 SELECT XMLSERIALIZE(CONTENT XMLMAP(TABLE STAFF AS FOREST) AS VARCHAR(400)) FROM STAFF WHERE ID = 1;
CREATE SCHEMA SHOP CREATE TABLE ITEMS (SKU CHAR(4) NOT NULL, PRICE NUMERIC(6,2));
INSERT INTO SHOP.ITEMS VALUES ('A100', 9.99), ('B200', NULL);
VALUES (XMLSERIALIZE(CONTENT XMLMAP(SCHEMA SHOP TARGETNAMESPACE 'urn:shop') AS VARCHAR(400)));
VALUES (XMLSERIALIZE(CONTENT XMLMAP(SCHEMA SHOP METADATA) AS VARCHAR(2000)));
VALUES (XMLMAP(TABLE NOWHERE));
```

```

--------------------------------------------------------------
 <STAFF xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><row><ID>1</ID><NAME>Ann</NAME><PAY>1200.50</PAY><PHOTO>0FB7</PHOTO></row><row><ID>2</ID><NAME>Bo &amp; Co</NAME></row></STAFF>
(1 row)

--------------------------------------------------------------
 <STAFF xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><ID>1</ID><NAME>Ann</NAME><PAY>1200.50</PAY><PHOTO>D7c=</PHOTO></STAFF><STAFF xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><ID>2</ID><NAME>Bo &amp; Co</NAME><PAY xsi:nil="true"/><PHOTO xsi:nil="true"/></STAFF>
(1 row)

--------------------------------------------------------------
 <STAFF><ID>1</ID><NAME>Ann</NAME><PAY>1200.50</PAY><PHOTO>D7c=</PHOTO></STAFF><STAFF><ID>2</ID><NAME>Bo &amp; Co</NAME></STAFF>
(1 row)
OK
OK

--------------------------------------------------------------
 <SHOP xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns="urn:shop"><ITEMS><row><SKU>A100</SKU><PRICE>9.99</PRICE></row><row><SKU>B200</SKU></row></ITEMS></SHOP>
(1 row)

--------------------------------------------------------------
 <xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:simpleType name="CHAR_4"><xs:restriction base="xs:string"><xs:length value="4"/></xs:restriction></xs:simpleType><xs:simpleType name="NUMERIC_6_2"><xs:restriction base="xs:decimal"><xs:totalDigits value="6"/><xs:fractionDigits value="2"/></xs:restriction></xs:simpleType><xs:complexType name="RowType.TURBOREL.SHOP.ITEMS"><xs:sequence><xs:element name="SKU" type="CHAR_4"/><xs:element name="PRICE" type="NUMERIC_6_2" minOccurs="0"/></xs:sequence></xs:complexType><xs:complexType name="TableType.TURBOREL.SHOP.ITEMS"><xs:sequence><xs:element name="row" type="RowType.TURBOREL.SHOP.ITEMS" minOccurs="0" maxOccurs="unbounded"/></xs:sequence></xs:complexType><xs:complexType name="SchemaType.TURBOREL.SHOP"><xs:all><xs:element name="ITEMS" type="TableType.TURBOREL.SHOP.ITEMS"/></xs:all></xs:complexType><xs:element name="SHOP" type="SchemaType.TURBOREL.SHOP"/></xs:schema>
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): table ANN.NOWHERE is not defined [X3.135-1986 §5.4 SR5] (at character 21)
```

## Registered XML Schemas

An XML Schema is registered by [CREATE XML
SCHEMA](statements/create-xml-schema.md) from one schema document, under
a name in an SQL schema, and dropped by [DROP XML
SCHEMA](statements/drop-xml-schema.md). Its document may include,
redefine or import the documents of other registered XML Schemas its
schema's owner may use, and nothing outside the database. The XML Schemas of the namespaces of XML Schema, of XML
Schema instances and of SQL/XML are registered always, as
INFORMATION_SCHEMA.XS, XSI and SQLXML. [XMLVALIDATE](#xmlvalidate) and
[IS VALID](#is-valid) validate values by one, and [the XMLSCHEMA
types](#the-xmlschema-types) hold the values it validates.

```sql
CREATE XML SCHEMA NOTES AS '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:notes"><xs:complexType name="text"/></xs:schema>';
CREATE XML SCHEMA PAGES AS '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:n="urn:notes" targetNamespace="urn:pages"><xs:import namespace="urn:notes"/><xs:element name="page" type="n:text"/></xs:schema>';
DROP XML SCHEMA NOTES RESTRICT;
```

```
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): registered XML Schema ANN.PAGES depends on registered XML Schema ANN.NOTES, so it is dropped only with CASCADE (at character 16)
```

Every user may use the XML Schemas registered always. The owner of a
registered XML Schema's schema may use it, and grants USAGE on it to
others:

```
GRANT USAGE ON XML SCHEMA <registered XML Schema name> TO <grantee> [ { , <grantee> }... ] [ WITH GRANT OPTION ]
REVOKE [ GRANT OPTION FOR ] USAGE ON XML SCHEMA <registered XML Schema name> FROM <grantee> [ { , <grantee> }... ] { CASCADE | RESTRICT }
```

See [GRANT](statements/grant.md) and [REVOKE](statements/revoke.md).
INFORMATION_SCHEMA.XML_SCHEMAS, XML_SCHEMA_NAMESPACES and
XML_SCHEMA_ELEMENTS describe those a user may use
([The Information Schema](07-information-schema.md)):

```sql
SELECT XML_SCHEMA_NAME, XML_SCHEMA_TARGET_NAMESPACE, XML_SCHEMA_IS_DETERMINISTIC FROM INFORMATION_SCHEMA.XML_SCHEMAS ORDER BY 1;
SELECT XML_SCHEMA_ELEMENT, XML_SCHEMA_ELEMENT_IS_DETERMINISTIC FROM INFORMATION_SCHEMA.XML_SCHEMA_ELEMENTS WHERE XML_SCHEMA_NAME = 'SQLXML' ORDER BY 1;
GRANT USAGE ON XML SCHEMA INFORMATION_SCHEMA.XS TO BOB;
GRANT USAGE ON XML SCHEMA NOTES TO BOB;
```

```
 XML_SCHEMA_NAME                                              | XML_SCHEMA_TARGET_NAMESPACE                                  | XML_SCHEMA_IS_DETERMINISTIC
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 NOTES                                                        | urn:notes                                                    | YES
 PAGES                                                        | urn:pages                                                    | YES
 SQLXML                                                       | http://standards.iso.org/iso/9075/2003/sqlxml                | YES
 XS                                                           | http://www.w3.org/2001/XMLSchema                             | NO
 XSI                                                          | http://www.w3.org/2001/XMLSchema-instance                    | YES
(5 rows)
 XML_SCHEMA_ELEMENT                                           | XML_SCHEMA_ELEMENT_IS_DETERMINISTIC
--------------------------------------------------------------+--------------------------------------------------------------
 sqlname                                                      | YES
 sqltype                                                      | YES
(2 rows)
OK, with a warning: SQLSTATE 01007
OK
```

XS is not deterministic, as each of its elements may hold an annotation
whose content is any element at all. Every user holds USAGE on XS
already, without grant option, so the first grant gives BOB nothing
more; the second lets BOB's own registered XML Schemas import NOTES.

XML_SCHEMAS_S, XML_SCH_NSPACES_S and XML_SCH_ELEMENTS_S show the same
rows as the three views, under SQL/XML's short names
([The short name views](07-information-schema.md#the-short-name-views)):

```sql
SELECT XML_SCHEMA_NAME, XML_SCH_TGT_NSPACE, XML_SCHEMA_IS_PERM FROM INFORMATION_SCHEMA.XML_SCHEMAS_S WHERE XML_SCHEMA_SCHEMA = 'ANN' ORDER BY 1;
SELECT XML_SCHEMA_NAME, XML_SCH_NSPACE, XML_SCH_NSP_IS_DET FROM INFORMATION_SCHEMA.XML_SCH_NSPACES_S WHERE XML_SCHEMA_NAME = 'PAGES' ORDER BY 2;
```

```
 XML_SCHEMA_NAME                                              | XML_SCH_TGT_NSPACE                                           | XML_SCHEMA_IS_PERM
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 NOTES                                                        | urn:notes                                                    | NO
 PAGES                                                        | urn:pages                                                    | NO
(2 rows)
 XML_SCHEMA_NAME                                              | XML_SCH_NSPACE                                               | XML_SCH_NSP_IS_DET
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 PAGES                                                        | urn:notes                                                    | YES
 PAGES                                                        | urn:pages                                                    | YES
(2 rows)
```

PAGES's namespaces are its target namespace and urn:notes, which it
imports.

## The XMLSCHEMA types

```
<secondary XML type modifier> ::=
    XMLSCHEMA <XML valid according to what> [ <XML valid element clause> ]

<XML valid according to what> ::=
      URI <XML URI> [ LOCATION <XML URI> ]
    | NO NAMESPACE [ LOCATION <XML URI> ]
    | ID <registered XML Schema name>

<XML valid element clause> ::=
      ELEMENT <identifier>
    | { NO NAMESPACE | NAMESPACE <XML URI> } [ ELEMENT <identifier> ]

<XML URI> ::= <character string literal>
```

XML(DOCUMENT(XMLSCHEMA ...)) and XML(CONTENT(XMLSCHEMA ...)) hold
values valid by a registered XML Schema. ID names it. URI names it by its
target namespace, NO NAMESPACE by having none, and LOCATION by its
schema location too: of those its user may use, the one of the least
name is taken. The element clause narrows the type to elements of a
namespace of the schema, or to one global element declaration, whose
name is the identifier's text as written, so `ELEMENT "book"` names the
element `book`. ELEMENT alone takes the schema's target namespace. A
namespace the schema has not is refused with SQLSTATE 2201M, an element
it does not declare with 2201N, as the type is written.

A column of such a type takes a value of a type of XMLSCHEMA of the
same registered XML Schema, of the namespace and the element the column
names, if it names them; anything else is refused with SQLCODE -103. Such
a value is made by XMLVALIDATE, or by CAST BY VALUE, which validates as
XMLVALIDATE does. A value sent by a client as a parameter of such a type
is refused with SQLSTATE 2201R, as no validation made it. A column, a
domain and a distinct type of such a type keep its registered XML Schema
from being dropped, even with CASCADE; a routine with a parameter or
result of one is dropped with it by DROP XML SCHEMA CASCADE.

The elements and attributes of a validated value are typed: XQuery reads
an `xs:integer` element as an integer, and XMLCAST to an SQL type casts
that value. Values of types of XMLSCHEMA of one registered XML Schema
brought together by UNION or CASE are of a type of it that names what
all name; with any other type, XML(CONTENT(ANY)). A type of XMLSCHEMA is
written with ID and the registered XML Schema's full name.

```sql
CREATE XML SCHEMA BOOKS AS
  '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:books" elementFormDefault="qualified">'
  '<xs:element name="book"><xs:complexType><xs:sequence>'
  '<xs:element name="title" type="xs:string"/><xs:element name="pages" type="xs:integer"/>'
  '</xs:sequence></xs:complexType></xs:element></xs:schema>';
CREATE TABLE SHELF (ID INTEGER, BOOK XML(DOCUMENT(XMLSCHEMA URI 'urn:books' ELEMENT "book")));
INSERT INTO SHELF VALUES (1, XMLVALIDATE(DOCUMENT XMLPARSE(DOCUMENT '<book xmlns="urn:books"><title>Dune</title><pages>412</pages></book>' PRESERVE WHITESPACE) ACCORDING TO XMLSCHEMA ID BOOKS ELEMENT "book"));
INSERT INTO SHELF VALUES (2, CAST(XMLPARSE(DOCUMENT '<book xmlns="urn:books"><title>Emma</title><pages>474</pages></book>' PRESERVE WHITESPACE) AS XML(DOCUMENT(XMLSCHEMA ID BOOKS ELEMENT "book")) BY VALUE));
INSERT INTO SHELF VALUES (3, XMLPARSE(DOCUMENT '<book xmlns="urn:books"><title>Kim</title><pages>368</pages></book>' PRESERVE WHITESPACE));
SELECT ID, XMLCAST(XMLQUERY('declare default element namespace "urn:books"; data(/book/pages) instance of xs:integer' PASSING BY REF BOOK RETURNING SEQUENCE EMPTY ON EMPTY) AS BOOLEAN) AS TYPED, XMLCAST(XMLQUERY('declare default element namespace "urn:books"; /book/pages' PASSING BY REF BOOK RETURNING SEQUENCE EMPTY ON EMPTY) AS INTEGER) + 1 AS MORE FROM SHELF ORDER BY ID;
DROP XML SCHEMA BOOKS CASCADE;
```

```
OK
OK
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): cannot assign a XML(DOCUMENT(UNTYPED)) value to column BOOK (XML(DOCUMENT(XMLSCHEMA ID ANN.BOOKS NAMESPACE 'urn:books' ELEMENT "book"))) [ISO/IEC 9075:1992 §13.8 SR6; ISO/IEC 9075-14:2006 draft §10.2 SR1b] (at character 29)
           ID | TYPED |                 MORE
--------------+-------+----------------------
            1 | TRUE  |                  413
            2 | TRUE  |                  475
(2 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.SHELF is of an XML type of registered XML Schema ANN.BOOKS, so it is not dropped, even with CASCADE (at character 16)
```

## XMLVALIDATE

```
XMLVALIDATE ( { DOCUMENT | CONTENT | SEQUENCE } <XML value expression>
              [ ACCORDING TO XMLSCHEMA <XML valid according to what>
                [ <XML valid element clause> ] ] )
```

XMLVALIDATE validates an XML value by a registered XML Schema, as
XQuery's `validate strict` does, and gives a copy of it whose elements
and attributes carry their types, with the default values the schema
gives added. DOCUMENT takes a document and validates it whole, its IDs
and identity constraints checked. CONTENT takes a document node and
validates each element child of it alone; SEQUENCE takes document,
element, comment and processing instruction nodes, and validates each
element, and each element child of a document node, alone. Comments and
processing instructions are copied. The null value gives the null value.

ACCORDING TO names the registered XML Schema, as a type of XMLSCHEMA
does, and its user must be able to use it. Its element clause makes each
element validated be of that namespace or that name. Without it, each
element is validated by the registered XML Schema of its namespace that
the user may use, the one of the least name if there are several; an
element of no namespace by one of no target namespace. The registered
XML Schemas INFORMATION_SCHEMA.XS, XSI and SQLXML validate as the texts
define them: XS by the Schema for Schemas, so a schema document is
valid by it; SQLXML by SQL/XML's own XML Schema; XSI declares no
element.

Of a DOCUMENT or CONTENT value with ACCORDING TO, the result is of
XML(DOCUMENT(XMLSCHEMA ...)) or XML(CONTENT(XMLSCHEMA ...)) of what the
clause names; without, of XML(DOCUMENT(ANY)) or XML(CONTENT(ANY)). Of
XML(SEQUENCE), the result is of XML(SEQUENCE).

A view, a CHECK, a domain's constraint, an assertion, a trigger or a
routine that names a registered XML Schema, by XMLVALIDATE, IS VALID or a
type of XMLSCHEMA, takes the one it named when it was created, even if
another of the same target namespace and a lesser name is registered
later, and depends on it: DROP XML SCHEMA RESTRICT is refused, and
CASCADE drops it too.

| SQLSTATE | When |
|---|---|
| 2200M | DOCUMENT of a value that is no document |
| 2200N | CONTENT of a value that is not one document node |
| 2201J | SEQUENCE of an atomic value, an attribute or a text node |
| 2201K | A document node validated has a text child |
| 2201L | No registered XML Schema the user may use is of an element's namespace |
| 2201M | The element clause names a namespace the schema has not |
| 2201N | The element clause names an element the schema does not declare |
| 2201P | An element is not of the name the element clause gives |
| 2201Q | An element is not of the namespace the element clause gives |
| 2201R | An element is not valid |

```sql
CREATE XML SCHEMA SIZES AS '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="size" type="xs:positiveInteger"/></xs:schema>';
VALUES (XMLSERIALIZE(CONTENT XMLVALIDATE(CONTENT XMLPARSE(CONTENT '<size>12</size><!--and--><size>4</size>' PRESERVE WHITESPACE)) AS VARCHAR(60)));
VALUES (XMLVALIDATE(CONTENT XMLPARSE(CONTENT '<size>0</size>' PRESERVE WHITESPACE) ACCORDING TO XMLSCHEMA ID SIZES));
VALUES (XMLVALIDATE(CONTENT XMLPARSE(CONTENT '<size>3</size>' PRESERVE WHITESPACE) ACCORDING TO XMLSCHEMA NO NAMESPACE ELEMENT "width"));
VALUES (XMLVALIDATE(DOCUMENT XMLPARSE(DOCUMENT '<weight xmlns="urn:weights">3</weight>' PRESERVE WHITESPACE)));
```

```
OK

--------------------------------------------------------------
 <size>12</size><!--and--><size>4</size>
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2201R): validation failure: [Schema1 §3.3.4 cvc-type.3.1.3] '0' is no value of the element's type [ISO/IEC 9075-14:2006 draft §6.20 GR6b iv]
turborel: SQLCODE -301 (SQLSTATE 2201N): registered XML Schema ANN.SIZES declares no global element width of namespace '' [ISO/IEC 9075-14:2006 draft §11.6 GR1b] (at character 106)
turborel: SQLCODE -301 (SQLSTATE 2201L): no registered XML Schema of namespace 'urn:weights' may be used: no XML schema found [ISO/IEC 9075-14:2006 draft §6.20 GR6b i 2]
```

## IS VALID

```
<XML valid predicate> ::= <XML value expression> IS [ NOT ] VALID
    { DOCUMENT | CONTENT | SEQUENCE }
    [ ACCORDING TO XMLSCHEMA <XML valid according to what>
      [ <XML valid element clause> ] ]
```

IS VALID is True if XMLVALIDATE of the value would succeed, and raises
nothing: it is False of a value not of the shape DOCUMENT, CONTENT or
SEQUENCE asks, or of an element not valid, or not of the element
clause's name or namespace. Without ACCORDING TO, an element of a
namespace of no registered XML Schema the user may use makes it Unknown,
unless another makes it False. It is Unknown of the null value. A CHECK
of IS VALID keeps a column of an UNTYPED type valid by a schema.

```sql
CREATE TABLE SIZED (S XML(CONTENT(UNTYPED)) CHECK (S IS VALID CONTENT ACCORDING TO XMLSCHEMA ID SIZES));
INSERT INTO SIZED VALUES (XMLPARSE(CONTENT '<size>8</size>' PRESERVE WHITESPACE));
INSERT INTO SIZED VALUES (XMLPARSE(CONTENT '<size>-8</size>' PRESERVE WHITESPACE));
SELECT CASE WHEN X IS VALID CONTENT THEN 'valid' WHEN X IS NOT VALID CONTENT THEN 'not valid' ELSE 'unknown' END AS IT FROM (VALUES (XMLPARSE(CONTENT '<size>8</size>' PRESERVE WHITESPACE)), (XMLPARSE(CONTENT '<size>eight</size>' PRESERVE WHITESPACE)), (XMLPARSE(CONTENT '<w xmlns="urn:none"/>' PRESERVE WHITESPACE))) AS V (X);
```

```
OK
OK
turborel: SQLCODE -201 (SQLSTATE 23000): CHECK ANN.SIZED_CHECK1 of ANN.SIZED is false for a row [ISO/IEC 9075:1992 §11.6 GR3]
 IT
-----------
 valid
 not valid
 unknown
(3 rows)
```

## The validate expression

```
validate [ lax | strict ] { <expression> }
```

XQuery's validate expression, in a query of XMLQUERY, XMLEXISTS or
XMLTABLE, validates one document or element node and gives a new one,
of no parent, whose elements and attributes carry their types, as
XMLVALIDATE's do. It validates by the registered XML Schemas the user may
use, of each target namespace the one of the least name, chosen as the
statement is prepared: the element validated, the node or a document
node's element, is validated by the one of its namespace, with whatever
that one imports; of a namespace none of them has,
INFORMATION_SCHEMA.XS, XSI or SQLXML. A view, a constraint, a trigger or a routine whose query has a
validate expression keeps the ones chosen when it was created, and
depends on each of them, as on a registered XML Schema it names.

strict, the default, needs a global declaration of the element. lax
takes one if there is one, else gives the element the type `xs:anyType`,
its attributes `xdt:untypedAtomic`, and validates each child element the
same way. A document node's IDs and identity constraints are checked;
an element node's are not. Each error is SQLSTATE 10000, the message
naming XQuery's code:

| Code | When |
|---|---|
| `err:XQTY0030` | The expression is not one document or element node |
| `err:XQDY0061` | A document node's children are not one element among comments and processing instructions |
| `err:XQDY0084` | strict, and no registered XML Schema in scope declares the element |
| `err:XQDY0027` | The node is not valid |

```sql
VALUES (XMLCAST(XMLQUERY('data((validate { . })/size) + 1' PASSING BY REF XMLPARSE(DOCUMENT '<size>12</size>' PRESERVE WHITESPACE) RETURNING SEQUENCE EMPTY ON EMPTY) AS INTEGER));
VALUES (XMLQUERY('validate { . }' PASSING BY REF XMLPARSE(DOCUMENT '<size>0</size>' PRESERVE WHITESPACE) RETURNING SEQUENCE EMPTY ON EMPTY));
VALUES (XMLQUERY('validate { <note/> }' RETURNING SEQUENCE BY REF EMPTY ON EMPTY));
VALUES (XMLSERIALIZE(CONTENT XMLQUERY('let $v := validate lax { <note>see <size>4</size></note> } return ($v instance of element(note, xs:anyType), data($v/size) instance of xs:positiveInteger)' RETURNING SEQUENCE BY REF EMPTY ON EMPTY) AS VARCHAR(20)));
```

```

--------------
           13
(1 row)
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error err:XQDY0027: [Schema1 §3.3.4 cvc-type.3.1.3] '0' is no value of the element's type (against registered XML Schema ANN.SIZES) [ISO/IEC 9075-14:2006 draft §6.18 GR3]
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error err:XQDY0084: [XQuery §3.13] note has no top-level element declaration (against registered XML Schema ANN.SIZES) [ISO/IEC 9075-14:2006 draft §6.18 GR3]

----------------------
 true true
(1 row)
```

## Schema imports

```
import schema [ namespace <prefix> = | default element namespace ]
    "<target namespace>" [ at "<location>" [ , "<location>" ]... ]
```

A query's prolog imports a registered XML Schema the user may use by
its target namespace: of several, the one whose schema location is one
of the locations after `at`, else the one of the least name,
INFORMATION_SCHEMA.XS, XSI or SQLXML after the others. The prefix is
bound to the namespace, or the namespace made the default element
namespace. The schema's type definitions, element declarations and
attribute declarations then name sequence types: `element(*, p:t)`
matches an element whose type is `p:t` or derived from it,
`schema-element(p:e)` one of the declaration `p:e` or of the
substitution group it heads, nilled only if `p:e` is nillable, and
`schema-attribute(p:a)` an attribute of the declaration `p:a`. A view,
a constraint, a trigger or a routine depends on what its queries
import.

A simple type the schema defines, restricting a built-in atomic type,
is an atomic type of the query as the built-in ones are. `$v instance
of p:t` holds of a value of `p:t` or of a type derived from it,
`treat as`, typeswitch and a function's parameters and result take it,
and `cast as p:t` and the constructor function `p:t(...)` make a value
of it, checking the type's facets: a value they refuse fails with
`err:FORG0001`. A string is read as the type's literal, its whitespace
normalized and its patterns matched first; another value is cast to the
built-in type the type restricts, then its patterns are matched against
its canonical form. A cast to a type derived from `xs:QName` is of a
string literal alone. The typed value of an element or attribute a
validation typed is of its own type, a list's items of the item type
and a union's of the member that validated each, and a value keeps its
type when stored. Otherwise a value of `p:t` is a value of the built-in
type it restricts: it compares, computes and serializes as one, and a
cast to that type keeps the value and drops `p:t`. A list or union type
is no atomic type (`err:XPST0051`).

| Code | When |
|---|---|
| `err:XQST0058` | The prolog imports one target namespace twice |
| `err:XQST0059` | No registered XML Schema the user may use has the target namespace |
| `err:XQST0035` | Two schemas imported define one name from different schema documents |
| `err:XQST0057` | A prefix is bound to no target namespace |

```sql
VALUES (XMLSERIALIZE(CONTENT XMLQUERY('import schema default element namespace ""; (validate { . })/size instance of schema-element(size)' PASSING BY REF XMLPARSE(DOCUMENT '<size>12</size>' PRESERVE WHITESPACE) RETURNING SEQUENCE EMPTY ON EMPTY) AS VARCHAR(10)));
VALUES (XMLQUERY('import schema "urn:nowhere"; 1' RETURNING SEQUENCE BY REF EMPTY ON EMPTY));
CREATE XML SCHEMA SHOES AS '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:s="urn:shoes" targetNamespace="urn:shoes"><xs:simpleType name="shoesize"><xs:restriction base="xs:integer"><xs:minInclusive value="1"/><xs:maxInclusive value="50"/></xs:restriction></xs:simpleType><xs:element name="shoe" type="s:shoesize"/></xs:schema>';
VALUES (XMLSERIALIZE(CONTENT XMLQUERY('import schema namespace s = "urn:shoes"; let $v := data((validate { . })/s:shoe) return ($v instance of s:shoesize, $v + 1, s:shoesize("42") instance of xs:integer, 42 instance of s:shoesize)' PASSING BY REF XMLPARSE(DOCUMENT '<shoe xmlns="urn:shoes">44</shoe>' PRESERVE WHITESPACE) RETURNING SEQUENCE EMPTY ON EMPTY) AS VARCHAR(40)));
VALUES (XMLQUERY('import schema namespace s = "urn:shoes"; 51 cast as s:shoesize' RETURNING SEQUENCE BY REF EMPTY ON EMPTY));
```

```

------------
 true
(1 row)
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error err:XQST0059: no registered XML Schema the current user may use has the target namespace 'urn:nowhere' [XQuery §4.10] [ISO/IEC 9075-14:2006 draft §6.18 GR3] (at character 8)
OK

------------------------------------------
 true 45 true false
(1 row)
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error: '51' is not valid for {urn:shoes}shoesize: it is not within its maxInclusive [Schema2 §4.3.7.3] 50 [Schema2 §4.1.4] [XQueryFO §17.1.1] err:FORG0001 [ISO/IEC 9075-14:2006 draft §6.18 GR3]
```

## XQuery modules

```
import module [ namespace <prefix> = ] "<target namespace>"
    [ at "<location>" [ , "<location>" ]... ]
```

A query's prolog imports an XQuery library module registered by [CREATE
XQUERY MODULE](statements/create-xquery-module.md) by its target
namespace, and calls its functions and reads its variables. Of the
modules of that namespace the user may use, it imports the one whose
location, or name, alone or after its schema's, is one of the locations
after `at`, else the one of the least name. The prefix is bound to the
namespace.

A query imports a module's own functions and variables, not those of the
modules it imports. Each module is analysed in its own static context: a
module sees none of the namespaces the statement declares, and its
functions keep its own setters, such as its construction mode. A library
module imports the modules it imported as it was registered, whoever
runs the query, so a user needs USAGE alone on the modules its query
imports. The modules a user may use are those of its own schemas and
those it holds USAGE on ([GRANT](statements/grant.md)).

| Code | When |
|---|---|
| `err:XQST0047` | The prolog imports one target namespace twice |
| `err:XQST0059` | No registered XQuery module the user may use has the target namespace |
| `err:XQST0034` | A function imported has the name and arity of one declared or imported |
| `err:XQST0049` | A variable imported has the name of one declared or imported |
| `err:XQST0036` | A function or variable imported is of a schema type the prolog does not import |
| `err:XQST0088` | The target namespace is the empty string |

```sql
CREATE XQUERY MODULE ORDERS_LIB AS
  'module namespace o = "urn:orders";'
  'declare function o:total($order as element(order)) as xs:double { fn:sum($order/item/@price) };';
SELECT ID, XMLCAST(XMLQUERY('import module namespace o = "urn:orders" at "ORDERS_LIB"; o:total(/order)' PASSING BY REF DOC RETURNING CONTENT NULL ON EMPTY) AS INTEGER) FROM ORDERS2 ORDER BY ID;
VALUES (XMLQUERY('import module "urn:nowhere"; 1' RETURNING SEQUENCE BY REF EMPTY ON EMPTY));
```

```
OK
           ID |
--------------+--------------
            1 |           35
            2 |            5
(2 rows)
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error err:XQST0059: no registered XQuery module the module may import has the target namespace 'urn:nowhere' [XQuery §4.11] [ISO/IEC 9075-14:2006 draft §6.18 GR3] (at character 8)
```

## Clients

A column of an XML type is described with its modifiers. A value is
sent as its serialization, and the client library parses it when its
nodes are needed. A value with no serialization, such as an attribute
standing alone, is refused with SQLSTATE 2200W when it is fetched.

## Limits

| Limit | Value |
|---|---|
| Elements nested | 512 |
| Text in one tree | 16 MiB |
| Nodes and namespace bindings in one tree | 1,048,576 |
| Text entities add to one parse | 16 MiB |
| Text XMLPARSE reads | 64 MiB |
| A value's stored form | 64 MiB |
| XMLMAP's data, of all its tables, and its schema document | 64 MiB |
| Elements and attributes an XML index keys of one value | 262,144 |
| Octets of a value an XML index keys | 64; 512 of XML_PRIMARY |
| Expressions nested in an XQuery query | 64 |
| Operations and function calls nested as a query is evaluated | 256 |
| Items of one sequence a query makes | 1,048,576 |
| Text of one string a query makes | 16 MiB |
| What the nodes one evaluation of a query constructs hold | 64 MiB |

A value or query past a limit fails with SQLCODE -900.
