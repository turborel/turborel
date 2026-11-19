# CREATE XQUERY MODULE

[Statements](README.md)

## NAME

CREATE XQUERY MODULE: register an XQuery library module *(extension)*

## SYNOPSIS

```
<XQuery module definition> ::=
    CREATE XQUERY MODULE <XQuery module name>
    [ LOCATION <XQuery module location> ]
    AS <XQuery module text>

<XQuery module name> ::= <schema qualified name>

<XQuery module location> ::= <character string literal>

<XQuery module text> ::= <character string literal>
```

## DESCRIPTION

CREATE XQUERY MODULE registers an XQuery library module under a name in
an SQL schema. A query of [XMLQUERY, XMLEXISTS or
XMLTABLE](../19-xml.md#querying-xml), or another library module, imports
it by `import module`, and calls its functions and reads its variables.

The text is a library module of XQuery 1.0: a module declaration, which
binds a prefix to the module's target namespace, then a prolog. Every
function and variable it declares is in that namespace. A character
string literal holds no line end, so a module written in one goes on in
quoted parts, one to a line, which are joined as one string. LOCATION is
its location, the empty string if not written.

The module is analysed as it is registered, in a static context of its
own: none of the namespaces a statement declares, its own setters,
namespaces and imports. Its schema imports import among the registered
XML Schemas, and its module imports among the registered XQuery modules,
the owner of its schema may use, those of its own schemas and those it
holds USAGE on ([GRANT](grant.md)). A module imports only modules
registered: [ALTER XQUERY MODULE](alter-xquery-module.md) replaces its
text once one it would import is, so modules of one namespace may import
one another. Those it imported are kept: it depends on them, and
imports them again whenever a query imports it, whoever runs the query.

CREATE XQUERY MODULE runs within the session's transaction: ROLLBACK
undoes it.

## RULES

1. Its schema exists, and holds no registered XQuery module of its name.
2. The text is a library module whose target namespace is not the empty
   string.
3. The module raises no static error of XQuery, nor does a module it
   imports.

## PRIVILEGES

Its schema's owner. A registered XQuery module or XML Schema it imports
is found if that owner owns its schema too, or holds USAGE on it (see
[GRANT](grant.md)).

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | One of its name exists, or its target namespace is the empty string |
| 3F000 | -103 | Its schema does not exist |
| 2A000 | -104 | Its user does not own the schema |
| 10000 | -301 | The text is no library module, or raises a static error of XQuery |
| 0A000 | -900 | Its modules import modules more deeply than turborel allows |

## EXAMPLES

```sql
CREATE XQUERY MODULE NOTES_LIB LOCATION 'notes.xq' AS
  'module namespace n = "urn:notes";'
  'declare function n:shout($s as xs:string) as xs:string { fn:upper-case($s) };';
VALUES (XMLCAST(XMLQUERY('import module namespace n = "urn:notes" at "NOTES_LIB"; n:shout("hello")' RETURNING CONTENT NULL ON EMPTY) AS VARCHAR(20)));
CREATE XQUERY MODULE LOUD_LIB AS
  'module namespace l = "urn:loud"; import module namespace n = "urn:notes";'
  'declare function l:twice($s as xs:string) as xs:string { fn:concat(n:shout($s), n:shout($s)) };';
CREATE XQUERY MODULE BROKEN AS 'module namespace b = "urn:b"; declare function local:f() { 1 };';
CREATE XQUERY MODULE MAIN AS '1 + 1';
CREATE XQUERY MODULE NOTES_LIB AS 'module namespace n = "urn:notes";';
```

```
OK

----------------------
 HELLO
(1 row)
OK
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error err:XQST0048: the function {http://www.w3.org/2005/xquery-local-functions}f is not in the target namespace urn:b of its library module [XQuery §4.2] [ISO/IEC 9075-14:2006 draft §6.18 GR3]
turborel: SQLCODE -301 (SQLSTATE 10000): XQuery error err:XPST0003: a registered XQuery module is a library module, which begins with a module declaration [XQuery §4] [ISO/IEC 9075-14:2006 draft §6.18 GR3]
turborel: SQLCODE -103 (SQLSTATE 2A000): registered XQuery module ANN.NOTES_LIB exists: one name identifies one (at character 21)
```

LOUD_LIB imports NOTES_LIB, which is of the namespace it imports.
BROKEN declares a function outside its target namespace, and MAIN is
no library module.

## STANDARD

An extension. ISO/IEC 9075-14:2006 draft has no statement of XQuery
modules. XQuery 1.0 §5.2.5 leaves where a module import finds its
modules to the implementation; this statement is turborel's. See
[CONFORMANCE.md](../../../CONFORMANCE.md#sqlxml).

## SEE ALSO

[ALTER XQUERY MODULE](alter-xquery-module.md), [DROP XQUERY MODULE](drop-xquery-module.md), [XML Values](../19-xml.md#xquery-modules),
[GRANT](grant.md), [DROP SCHEMA](drop-schema.md)
