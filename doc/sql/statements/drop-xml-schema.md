# DROP XML SCHEMA

[Statements](README.md)

## NAME

DROP XML SCHEMA: drop a registered XML Schema

## SYNOPSIS

```
<drop XML Schema statement> ::=
    DROP XML SCHEMA <registered XML Schema name> { RESTRICT | CASCADE }
```

## DESCRIPTION

DROP XML SCHEMA drops an XML Schema CREATE XML SCHEMA registered, and
the privileges on it.

A registered XML Schema whose document includes, redefines or imports
one depends on it, and so on through each that depends on that. So
does an SQL-invoked routine with a parameter or result of a [type of
XMLSCHEMA](../19-xml.md#the-xmlschema-types) of one of them, and a view,
a CHECK, a domain's constraint, an assertion, a trigger or a routine
that names one of them by [XMLVALIDATE](../19-xml.md#xmlvalidate), [IS
VALID](../19-xml.md#is-valid) or a type of XMLSCHEMA, or has one of them in
scope of an XQuery [validate expression](../19-xml.md#the-validate-expression),
and what invokes such a routine. With RESTRICT the drop is refused while anything depends on
the one named. With CASCADE each that does is dropped too.

A column, a domain or a user-defined type of a type of XMLSCHEMA of one
of them keeps it from being dropped, even with CASCADE, as dropping it
would change no value of the column: drop or alter that first.

The permanently registered XML Schemas, INFORMATION_SCHEMA.XS, XSI and
SQLXML, are never dropped. DROP SCHEMA with CASCADE drops the registered
XML Schemas of the schema, as this statement with CASCADE does.

DROP XML SCHEMA runs within the session's transaction: ROLLBACK undoes
it.

## RULES

1. It names a registered XML Schema, not a permanently registered one.
2. RESTRICT: nothing depends on it.
3. No column, domain or user-defined type is of a type of XMLSCHEMA of it
   or of one that depends on it.

## PRIVILEGES

Its schema's owner.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the registered XML Schema's schema |

## EXAMPLES

```sql
CREATE XML SCHEMA BASE AS '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:base"><xs:complexType name="money"/></xs:schema>';
CREATE XML SCHEMA PRICES AS
  '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:b="urn:base" targetNamespace="urn:prices">'
  '<xs:import namespace="urn:base"/><xs:element name="price" type="b:money"/></xs:schema>';
DROP XML SCHEMA BASE RESTRICT;
DROP XML SCHEMA BASE CASCADE;
DROP XML SCHEMA PRICES CASCADE;
DROP XML SCHEMA INFORMATION_SCHEMA.XS CASCADE;
```

```
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): registered XML Schema ANN.PRICES depends on registered XML Schema ANN.BASE, so it is dropped only with CASCADE (at character 16)
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no registered XML Schema ANN.PRICES (at character 16)
turborel: SQLCODE -103 (SQLSTATE 2A000): INFORMATION_SCHEMA.XS is permanently registered: it is never dropped [ISO/IEC 9075-14:2006 draft §4.2.6] (at character 16)
```

PRICES went with BASE, as it imports it.

## STANDARD

ISO/IEC 9075-14 §4.2.6 leaves how an XML Schema is registered to the
implementation; this statement is turborel's. See
[CONFORMANCE.md](../../../CONFORMANCE.md#sqlxml).

## SEE ALSO

[CREATE XML SCHEMA](create-xml-schema.md),
[DROP SCHEMA](drop-schema.md)
