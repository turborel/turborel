# XML Values

[Contents](README.md)

The XML type holds XML documents and fragments of them, as SQL/XML
(ISO/IEC 9075-14) defines it. XMLPARSE makes an XML value from text, and
XMLSERIALIZE writes one back as text. XMLDOCUMENT makes a document node
of a value, XMLCAST casts SQL values to XML and back, and IS DOCUMENT and
IS CONTENT test what a value is. XMLQUERY, XMLEXISTS and XMLTABLE query
XML values with XQuery. XMLMAP maps a table, a schema or the catalog to
XML and to an XML Schema document.

## The type

```
<XML type> ::= XML [ ( <XML type modifier> ) ]
<XML type modifier> ::=
      DOCUMENT [ ( { ANY | UNTYPED } ) ]
    | CONTENT [ ( { ANY | UNTYPED } ) ]
    | SEQUENCE
```

| Type | Holds |
|---|---|
| XML(DOCUMENT) | A document: one root element, with comments and processing instructions beside it. |
| XML(CONTENT) | A document node of any children: elements, text, comments and processing instructions. |
| XML(SEQUENCE) | Any XML value. XML alone is XML(SEQUENCE). |

UNTYPED and ANY say whether a value may carry types from an XML Schema.
Every value XMLPARSE makes is UNTYPED. DOCUMENT and CONTENT alone are
ANY. An UNTYPED column takes values of an UNTYPED type only; the others
take any XML value. XMLSCHEMA, the type of values valid by a registered
XML Schema, is not yet implemented (SQLCODE -900).

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

[The Information Schema](information-schema.md) shows a type's modifiers
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
to an UNTYPED type only from an UNTYPED one. BY VALUE copies them.

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
an SQL type. Between two XML types it is CAST.

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
document node's children, an element's text. That text is read as a
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
| INTERVAL | `xdt:yearMonthDuration` or `xdt:dayTimeDuration`, each field written as XMLCAST writes it: the first of the leading precision's digits, the others of two, the seconds' fraction of the fractional precision's |

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
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error err:FOAR0001: a decimal or integer is divided by zero [XQueryFO §6.2.4] [ISO/IEC 9075-14:2006 draft §6.18 GR3]
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
and computed constructors, ordered and unordered expressions, and a
prolog's namespaces, setters, variables and functions, recursive ones
too. Static typing is not done, so a type error is found as the query is
evaluated.

These are not supported: schema imports and library modules (error
`err:XQST0009` or `err:XQST0016`), the validate expression's lax mode
(SQLCODE -900; strict mode finds no element declared, `err:XQDY0084`),
casts to `xs:QName`, and the documents of `fn:doc`, which finds none
(`err:FODC0005`). The built-in functions are `fn:true`, `fn:false`,
`fn:not`, `fn:boolean`, `fn:data`, `fn:string`, `fn:count`, `fn:empty`,
`fn:exists`, `fn:position`, `fn:last`, `fn:concat`, `fn:root` and
`fn:doc`, and a constructor function for each atomic type, such as
`xs:date('2026-11-17')`.

Strings compare by Unicode code points unless the query declares
another collation. A collation of the database is the URI
`urn:turborel:collation:` then its name, as
`urn:turborel:collation:UNICODE`.

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
turborel: SQLCODE -103 (SQLSTATE 2A000): index ORDER_IDS of method xml_property keys one column, of an XML type (RULES.md D86)
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

XMLBINARY says how XMLELEMENT, XMLFOREST and XMLCAST write a binary
string, and how XMLCAST reads one: BASE64 in base64, HEX in hex. Where
no XMLBINARY is in scope it is HEX. XMLPI always writes hex.

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
none. ENCODING writes binary strings in HEX, the default, or BASE64.

METADATA gives the XML Schema document instead. It defines a type of
each column's data type, distinct type or domain, named after it, a type
of each table's rows, and, AS ELEMENT, of each table; then of the
schema and of the catalog, and declares the element at the root of the
data. DATA AND METADATA gives both: the schema document, then the data.

Only what the user may read is mapped. TABLE needs SELECT on the table
(SQLCODE -104). A schema or the catalog holds only the tables the user
may SELECT that have a column to map, and the catalog only the schemas
of such tables, the Information Schema among them. A column is left out
if it is of XML(SEQUENCE) (XML alone), a structured, reference, row,
array or multiset type, DECFLOAT or a plug-in's type, or of a distinct
type whose cast to its source type the user may not execute. A mapping
that leaves out a column the user may read, as its type does not map,
completes with the warning 01010 (column cannot be mapped).

| Mapping | Value |
|---|---|
| DATA, AS ELEMENT, or of a schema or the catalog | XML(DOCUMENT(UNTYPED)): a document |
| DATA of a table AS FOREST | XML(CONTENT(UNTYPED)): the rows' elements |
| METADATA | XML(DOCUMENT(UNTYPED)): the XML Schema document |
| DATA AND METADATA | XML(SEQUENCE): the schema document, then the data |

A view whose query maps a table depends on the table, so DROP TABLE
RESTRICT is refused while it does. A CHECK constraint may not hold
XMLMAP (SQLCODE -103), as it reads rows in an order not determined. A
target namespace of the namespace of `xml` or `xmlns` is refused
(SQLCODE -103), and a schema that is not there is SQLSTATE 3F000.

```sql
VALUES (XMLSERIALIZE(CONTENT XMLMAP(TABLE STAFF) AS VARCHAR(400)));
VALUES (XMLSERIALIZE(CONTENT XMLMAP(TABLE STAFF NULLS NIL AS FOREST ENCODING BASE64) AS VARCHAR(400)));
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
| XMLMAP's text of one table's rows, and of its data or schema document | 64 MiB |
| Elements and attributes an XML index keys of one value | 262,144 |
| Octets of a value an XML index keys | 64; 512 of XML_PRIMARY |
| Expressions nested in an XQuery query | 64 |
| Operations and function calls nested as a query is evaluated | 256 |
| Items of one sequence a query makes | 1,048,576 |
| Text of one string a query makes | 16 MiB |
| What the nodes one evaluation of a query constructs hold | 64 MiB |

A value or query past a limit fails with SQLCODE -900.
