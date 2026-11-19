# ALTER XQUERY MODULE

[Statements](README.md)

## NAME

ALTER XQUERY MODULE: replace a registered XQuery module's text

## SYNOPSIS

```
<alter XQuery module statement> ::=
    ALTER XQUERY MODULE <XQuery module name>
    [ LOCATION <XQuery module location> ]
    AS <XQuery module text>

<XQuery module name> ::= <schema qualified name>

<XQuery module location> ::= <character string literal>

<XQuery module text> ::= <character string literal>
```

## DESCRIPTION

ALTER XQUERY MODULE replaces the text of an XQuery module CREATE XQUERY
MODULE registered, and its location if LOCATION is written. The new text
is analysed as CREATE XQUERY MODULE analyses one, and the modules and XML
Schemas its imports import are found again and kept, its USAGE grants
kept too.

A module imports only modules that are registered, so a module that
imports one registered after it is altered once that one is. So modules
of one namespace import one another, each calling the others' functions:
XQuery allows such a cycle of imports. A cycle of modules not all of one
namespace is refused.

What depends on the module must still work with the new text: each
module that imports it, at any remove, must still analyse, and each
view, constraint, assertion, generated column, trigger and routine whose
queries import it, or one of those modules, must still bind. Else the
statement is refused. So a function or variable they use is not taken
away, and the target namespace changes only if nothing imports the
module by it.

ALTER XQUERY MODULE runs within the session's transaction: ROLLBACK
undoes it.

## RULES

1. A registered XQuery module of its name exists.
2. The text is a library module whose target namespace is not the empty
   string, of no static error, nor any module it imports.
3. No cycle of imports it closes holds modules of more than one
   namespace.
4. What depends on it still analyses and binds.

## PRIVILEGES

Its schema's owner. A registered XQuery module or XML Schema it imports
is found if that owner owns its schema too, or holds USAGE on it.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | There is none of its name, its target namespace is the empty string, or something depending on it would no longer analyse or bind |
| 2A000 | -104 | Its user does not own its schema |
| 10000 | -301 | The text is no library module, raises a static error of XQuery, or closes a cycle of modules of more than one namespace (`err:XQST0073`) |
| 0A000 | -900 | Its modules import modules more deeply than turborel allows |

## EXAMPLES

```sql
CREATE XQUERY MODULE EVEN AS 'module namespace m = "urn:m"; declare function m:even($n) { $n = 0 };';
CREATE XQUERY MODULE ODD AS
  'module namespace m = "urn:m"; import module "urn:m";'
  'declare function m:odd($n) { if ($n = 0) then fn:false() else m:even($n - 1) };';
ALTER XQUERY MODULE EVEN AS
  'module namespace m = "urn:m"; import module "urn:m";'
  'declare function m:even($n) { if ($n = 0) then fn:true() else m:odd($n - 1) };';
VALUES (XMLCAST(XMLQUERY('import module namespace m = "urn:m" at "ODD"; m:odd(7)' RETURNING CONTENT NULL ON EMPTY) AS VARCHAR(10)));
ALTER XQUERY MODULE EVEN AS 'module namespace m = "urn:m"; declare variable $m:zero := 0;';
DROP XQUERY MODULE EVEN RESTRICT;
```

```
OK
OK
OK

------------
 true
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): registered XQuery module ANN.ODD would no longer analyse or bind with registered XQuery module ANN.EVEN altered: XQuery error err:XPST0017: no function {urn:m}even of 1 arguments is in the static context [XQuery §3.1.5] (at character 20)
turborel: SQLCODE -103 (SQLSTATE 2A000): registered XQuery module ANN.ODD depends on registered XQuery module ANN.EVEN, so it is dropped only with CASCADE (at character 19)
```

ODD imported EVEN as it was registered, and EVEN, altered, imports ODD,
the other module of its namespace. EVEN cannot lose `m:even`, which ODD
calls, nor be dropped alone while ODD imports it.

## STANDARD

XQuery 1.0 §5.2.5 leaves where a module import finds its modules to the
implementation, and §4.11 allows a cycle of imports of modules of one
namespace; this statement is turborel's. See
[CONFORMANCE.md](../../../CONFORMANCE.md#sqlxml).

## SEE ALSO

[CREATE XQUERY MODULE](create-xquery-module.md), [DROP XQUERY MODULE](drop-xquery-module.md),
[XML Values](../19-xml.md#xquery-modules)
