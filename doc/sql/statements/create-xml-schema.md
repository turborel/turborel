# CREATE XML SCHEMA

[Statements](README.md)

## NAME

CREATE XML SCHEMA: register an XML Schema

## SYNOPSIS

```
<XML Schema definition> ::=
    CREATE XML SCHEMA <registered XML Schema name>
    [ LOCATION <XML valid schema location URI> ]
    AS <XML Schema document>

<registered XML Schema name> ::= <schema qualified name>

<XML valid schema location URI> ::= <character string literal>

<XML Schema document> ::= <value expression>
```

## DESCRIPTION

CREATE XML SCHEMA registers an XML Schema, a schema of XML Schema Part 1
read from one schema document, under a name in an SQL schema. A
registered XML Schema is what SQL/XML's [types of
XMLSCHEMA](../xml.md#the-xmlschema-types), [XMLVALIDATE](../xml.md#xmlvalidate)
and [IS VALID](../xml.md#is-valid) validate XML values by.

The document is a character string, a binary string or an XML value,
evaluated once. A character string is the document as written. A binary
string is read as XMLPARSE DOCUMENT reads one, its encoding found as
there, and an XML value must be a document. Either is kept as its
serialization. A `?` written alone takes the type CHARACTER LARGE OBJECT.
A character string literal holds no line end, so a document written in
one goes on in quoted parts, one to a line, which are joined as one
string; a document with line ends is passed as a parameter, or read from
a column.

The document's element is an `xs:schema`. Its targetNamespace is the
registered XML Schema's target namespace, the empty string if it has
none. LOCATION is its schema location, the empty string if not written.

Its `<xs:include>`, `<xs:redefine>` and `<xs:import>` find their documents
among the registered XML Schemas the owner of its schema may use, those
of its own schemas, and nowhere else: no file or URL is read. An include
or redefine finds the one whose LOCATION is its schemaLocation; an import
the one at its schemaLocation, which must be of the namespace it imports,
else the first of that namespace, in order of name. One not found adds
nothing, so a reference to what it would define is refused. The
components of what it includes, redefines and imports are its own too. A registered XML Schema depends on those it read:
DROP XML SCHEMA RESTRICT of one of them is refused while it stands.

The document is checked as XML Schema Part 1 says a schema is: each
element as its representation allows, every name it refers to defined,
each content model deterministic. Its simple types are XML Schema Part
2's built-in datatypes and those its `<xs:simpleType>`s define, by
restriction with facets, by list or by union, and those a restriction of
simple content makes with facets. A facet that does not apply to its
base, or widens or changes a fixed one of its base, a type derived as its
base's final forbids, and a type defined by way of itself, are refused.

Three XML Schemas are registered always, and are never registered again
or dropped: INFORMATION_SCHEMA.XS, XSI and SQLXML, of the namespaces of
XML Schema, of XML Schema instances and of SQL/XML.

CREATE XML SCHEMA runs within the session's transaction: ROLLBACK undoes
it.

## RULES

1. Its schema exists, and holds no registered XML Schema of its name.
2. The document is a character string, a binary string or an XML type.
3. It is one well-formed XML document, whose element is an `xs:schema`,
   of a valid schema.

## PRIVILEGES

Its schema's owner. A registered XML Schema it includes, redefines or
imports is found if that owner owns its schema too.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | One of its name exists, or the document is of another type |
| 3F000 | -103 | Its schema does not exist |
| 2A000 | -104 | Its user does not own the schema |
| 2200L | -301 | The document is the null value, or an XML value that is no document |
| 2200M | -301 | The document is no well-formed XML document |
| 22000 | -301 | The document is no schema document, or not of a valid schema |
| 0A000 | -900 | The schema is past a limit of [XML Values](../xml.md#limits) or of reading a schema |

## EXAMPLES

```sql
CREATE XML SCHEMA ORDERS LOCATION 'http://example.com/orders.xsd' AS
  '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:orders">'
  '<xs:complexType name="item"><xs:sequence><xs:element name="sku" type="xs:string"/></xs:sequence></xs:complexType>'
  '</xs:schema>';
CREATE XML SCHEMA LINES AS
  '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:o="urn:orders" targetNamespace="urn:lines">'
  '<xs:import namespace="urn:orders"/><xs:element name="line" type="o:item"/></xs:schema>';
CREATE XML SCHEMA LOST AS
  '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:p="urn:prices" targetNamespace="urn:lost">'
  '<xs:import namespace="urn:prices"/><xs:element name="price" type="p:money"/></xs:schema>';
CREATE XML SCHEMA ORDERS AS '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"/>';
CREATE XML SCHEMA BROKEN AS '<order/>';
```

```
OK
OK
turborel: SQLCODE -301 (SQLSTATE 22000): [Schema1 §3.15.3 src-resolve] the schema is not valid: {urn:prices}money names no type definition
turborel: SQLCODE -103 (SQLSTATE 2A000): registered XML Schema ANN.ORDERS exists: one name identifies one [ISO/IEC 9075-14:2006 draft §4.2.6] (at character 18)
turborel: SQLCODE -301 (SQLSTATE 22000): [Schema1 §3.15.2] the schema is not valid: a schema document's element is a <schema> of the XML Schema namespace
```

LINES imports the namespace of ORDERS and finds its type `item` there. No
registered XML Schema is of the namespace LOST imports, so its type
`money` is found nowhere.

## STANDARD

ISO/IEC 9075-14 §4.2.6 leaves how an XML Schema is registered to the
implementation; this statement is turborel's. The registered XML Schema
name is §5.2's. See [CONFORMANCE.md](../../../CONFORMANCE.md#sqlxml).

## SEE ALSO

[DROP XML SCHEMA](drop-xml-schema.md), [XML Values](../xml.md#registered-xml-schemas),
[DROP SCHEMA](drop-schema.md)
