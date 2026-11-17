# XML Values

[Contents](README.md)

The XML type holds XML documents and fragments of them, as SQL/XML
(ISO/IEC 9075-14) defines it. XMLPARSE makes an XML value from text, and
XMLSERIALIZE writes one back as text. XMLDOCUMENT makes a document node
of a value, XMLCAST casts SQL values to XML and back, and IS DOCUMENT and
IS CONTENT test what a value is.

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
apart from it as large objects are, logged and recovered with them.

XML values are not comparable. `=`, IN, DISTINCT, GROUP BY, ORDER BY,
MAX, MIN, UNION without ALL, a NATURAL or USING join on an XML column,
and a key or an index of one are refused (SQLCODE -103).

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
as an XML duration such as `P01Y02M`, and a binary string in hex. Cast
to XML(SEQUENCE), it becomes one atomic value of the XML Schema type its
type maps to, in that type's canonical form: `xs:decimal` 1.5 for
NUMERIC 1.50, `xdt:yearMonthDuration` P1Y2M. A character no XML 1.1
character fails with SQLSTATE 0N002. No SQL value is cast to
XML(DOCUMENT).

An XML value cast to an SQL type gives the text of its one item: a
document node's children, an element's text. That text, its white space
collapsed, is read as a value of the XML Schema type the SQL type maps
to, then cast to the SQL type, whose length, precision and range apply
as CAST applies them. A date or time with a time zone is moved to UTC
for a type without one; a type WITH TIME ZONE needs one (SQLSTATE
22007). Text that is no such value, or more than one item, fails with
SQLSTATE 10000; a value with no item is null.

```sql
SELECT ID, XMLCAST(BODY AS VARCHAR(20)) FROM ORDERS ORDER BY ID;
VALUES (XMLSERIALIZE(XMLCAST(CAST(1.50 AS NUMERIC(5,2)) AS XML(CONTENT)) AS VARCHAR(20)), XMLSERIALIZE(XMLCAST(CAST(1.50 AS NUMERIC(5,2)) AS XML) AS VARCHAR(20)));
VALUES (XMLCAST(XMLPARSE(CONTENT '<total currency="EUR"> 17.25 </total>' PRESERVE WHITESPACE) AS NUMERIC(6,1)));
VALUES (XMLCAST(XMLPARSE(CONTENT '<due>2026-11-17T09:30:00-05:00</due>' PRESERVE WHITESPACE) AS TIMESTAMP));
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
     17.2
(1 row)

----------------------------
 2026-11-17 14:30:00.000000
(1 row)
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error: 'many' is no lexical form of xs:integer [XQueryFO §17.1.1] err:FORG0001 [ISO/IEC 9075-14:2006 draft §6.6 GR3d vi, GR4j]
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
it; a binary string is written in hex. An attribute or content of an
array or a multiset is not yet supported (SQLCODE -900).

The content option says what an element of content all null is: EMPTY
ON NULL, the default, an empty element; NULL ON NULL the null value;
ABSENT ON NULL nothing; NIL ON NULL an empty element with the attribute
`xsi:nil="true"`. NIL ON NO CONTENT gives that attribute to an element
with no element or text in its content.

XMLFOREST makes an element of each value, named as attributes are, and
gives them in order. Its content option applies to each element; NULL
ON NULL is its default, so a forest of null values alone is null.

XMLNAMESPACES binds prefixes to namespaces for the names of the element
or forest it is in and of the operators within it. DEFAULT names the
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

A value past a limit fails with SQLCODE -900.
