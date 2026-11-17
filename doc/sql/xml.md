# XML Values

[Contents](README.md)

The XML type holds XML documents and fragments of them, as SQL/XML
(ISO/IEC 9075-14) defines it. XMLPARSE makes an XML value from text, and
XMLSERIALIZE writes one back as text.

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
