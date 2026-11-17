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
