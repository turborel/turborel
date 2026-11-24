# RETURN

[Statements](README.md)

## NAME

RETURN: give the value of an SQL-invoked function

## SYNOPSIS

```
<return statement> ::= RETURN { <value expression> | NULL }
```

RETURN is read as a word, not a key word.

## DESCRIPTION

RETURN is the body of a function defined by
[CREATE FUNCTION](create-function.md), or a statement of a body that is
a program ([BEGIN](begin.md)), where it ends the function's execution
there. Its value, evaluated with the
function's parameters as its arguments give them, is stored in the
function's result type and is the value of the invocation. RETURN NULL
gives a null of the result type.

The value expression may read tables, through a subquery, if the
function is of READS SQL DATA.

## RULES

1. RETURN is the body of a function, or in one: not a statement of its
   own, nor in the body of a procedure.
2. Its value's type is one the result type may store.

## PRIVILEGES

Those of its value expression, held by the owner of the function's
schema.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -103 | It is not the body of a function, or its type is not one the result type stores |
| 22001 | -301 | Its value is too long for the result type |
| 22003 | -301 | Its value is out of the result type's range |

## EXAMPLES

```sql
CREATE FUNCTION HALF (N INTEGER) RETURNS DECIMAL(9,1) CONTAINS SQL RETURN N / 2.0;
CREATE FUNCTION NOTHING () RETURNS INTEGER CONTAINS SQL RETURN NULL;
SELECT HALF(7), NOTHING() FROM (VALUES (0)) AS ONE (X);
RETURN 1;
CREATE PROCEDURE ANSWER () CONTAINS SQL RETURN 1;
```

```
OK
OK
             |
-------------+--------------
         3.5 |         NULL
(1 row)
turborel: SQLCODE -103 (SQLSTATE 42000): a <return statement> shall be contained in the <SQL routine body> of an SQL-invoked function [ISO/IEC 9075-2:2023 §16.2 SR1] (at character 0)
turborel: SQLCODE -103 (SQLSTATE 42000): a <return statement> shall be contained in the <SQL routine body> of an SQL-invoked function [ISO/IEC 9075-2:2023 §16.2 SR1] (at character 40)
```

## STANDARD

ISO/IEC 9075-2:2023 §16.2, Feature T321-05, and in a program ISO/IEC
9075-4:2006 draft §11.2. SQL-86 and SQL-92 have no RETURN.

## SEE ALSO

[CREATE FUNCTION](create-function.md), [BEGIN](begin.md)
