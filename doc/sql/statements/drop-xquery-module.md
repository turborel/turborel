# DROP XQUERY MODULE

[Statements](README.md)

## NAME

DROP XQUERY MODULE: drop a registered XQuery module

## SYNOPSIS

```
<drop XQuery module statement> ::=
    DROP XQUERY MODULE <XQuery module name> <drop behavior>

<drop behavior> ::= RESTRICT | CASCADE
```

## DESCRIPTION

DROP XQUERY MODULE drops an XQuery module CREATE XQUERY MODULE
registered, and the privileges on it.

A registered XQuery module that imported it as it was registered
depends on it, and so on through each that depends on that. So does a
view, a CHECK, a domain's constraint, an assertion, a trigger or a
routine whose query imports one of them. With RESTRICT the drop is
refused while anything depends on the one named. With CASCADE each that
does is dropped too: of modules of one namespace that import one another
([ALTER XQUERY MODULE](alter-xquery-module.md)), the whole cycle.

A generated column whose query imports one of them keeps it from being
dropped, even with CASCADE, as dropping it would change no value of the
column: drop or alter that first.

DROP SCHEMA with CASCADE drops the registered XQuery modules of the
schema, as this statement with CASCADE does. Dropping a registered XML
Schema with CASCADE drops the modules that import it.

DROP XQUERY MODULE runs within the session's transaction: ROLLBACK
undoes it.

## RULES

1. A registered XQuery module of its name exists.
2. With RESTRICT, nothing depends on it.

## PRIVILEGES

Its schema's owner.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | There is none of its name, or, with RESTRICT, something depends on it |
| 2A000 | -104 | Its user does not own its schema |

## EXAMPLES

```sql
CREATE XQUERY MODULE NOTES_LIB AS 'module namespace n = "urn:notes"; declare function n:shout($s) { fn:upper-case($s) };';
CREATE XQUERY MODULE LOUD_LIB AS 'module namespace l = "urn:loud"; import module namespace n = "urn:notes"; declare function l:twice($s) { n:shout($s) };';
DROP XQUERY MODULE NOTES_LIB RESTRICT;
DROP XQUERY MODULE NOTES_LIB CASCADE;
DROP XQUERY MODULE LOUD_LIB RESTRICT;
```

```
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): registered XQuery module ANN.LOUD_LIB depends on registered XQuery module ANN.NOTES_LIB, so it is dropped only with CASCADE (at character 19)
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no registered XQuery module ANN.LOUD_LIB (at character 19)
```

LOUD_LIB went with NOTES_LIB, as it imports it.

## STANDARD

XQuery 1.0 §5.2.5 leaves where a module import finds its modules to the
implementation; this statement is turborel's. See
[CONFORMANCE.md](../../../CONFORMANCE.md#sqlxml).

## SEE ALSO

[CREATE XQUERY MODULE](create-xquery-module.md), [XML Values](../19-xml.md#xquery-modules),
[DROP SCHEMA](drop-schema.md)
