# DROP ROUTINE

[Statements](README.md)

## NAME

DROP ROUTINE, DROP FUNCTION, DROP PROCEDURE: drop an SQL-invoked
routine

## SYNOPSIS

```
<drop routine statement> ::= DROP <specific routine designator> <drop behavior>

<specific routine designator> ::=
      SPECIFIC <routine type> <specific name>
    | <routine type> <schema qualified routine name> [ ( [ <data type> [ , <data type> ]... ] ) ]

<routine type> ::= ROUTINE | FUNCTION | PROCEDURE

<drop behavior> ::= CASCADE | RESTRICT
```

## DESCRIPTION

DROP ROUTINE drops a function or procedure defined by
[CREATE FUNCTION](create-function.md) or
[CREATE PROCEDURE](create-procedure.md), with the privileges on it. It
is named by its name, with its parameters' types or without, or by its
specific name. ROUTINE names either kind. FUNCTION and PROCEDURE name
only their own.

With RESTRICT, a routine anything invokes is not dropped. With CASCADE,
what invokes it is dropped too: views, constraints, assertions,
generated columns, triggers and other routines, and what invokes those,
at any remove.

A routine is dropped too, with CASCADE, with a table, view or column
its body reads, a distinct type it is typed by, its schema, or a
privilege its owner needs for its body. Without CASCADE such a drop or
revoke is refused.

DROP ROUTINE runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The routine exists and is of the routine type named, and its user
   owns its schema.
2. With RESTRICT, no view, constraint, assertion, generated column,
   trigger or routine invokes it.
3. The transaction is not READ ONLY.

## PRIVILEGES

The routine's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | No `<drop behavior>` is written |
| 2A000 | -103 | There is no such routine, or a rule above is broken |
| 2A000 | -104 | Its user does not own the routine's schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

```sql
CREATE FUNCTION DOUBLED (N INTEGER) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN N * 2;
CREATE VIEW DOUBLES (N, TWICE) AS SELECT X, DOUBLED(X) FROM (VALUES (1), (2)) AS T (X);
DROP PROCEDURE DOUBLED RESTRICT;
DROP FUNCTION DOUBLED (INTEGER) RESTRICT;
DROP FUNCTION DOUBLED CASCADE;
SELECT * FROM DOUBLES;
```

```
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): routine ANN.DOUBLED is not of the <routine type> named [ISO/IEC 9075-2:2023 §10.6 SR4] (at character 15)
turborel: SQLCODE -103 (SQLSTATE 2A000): view ANN.DOUBLES invokes routine ANN.DOUBLED: drop it with CASCADE [ISO/IEC 9075-2:2023 §11.62 SR4c] (at character 14)
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): table ANN.DOUBLES is not defined [X3.135-1986 §5.4 SR5] (at character 14)
```

## STANDARD

ISO/IEC 9075-2:2023 §11.62, §10.6, Feature T321. SQL-86 and SQL-92 have
no SQL-invoked routines.

## SEE ALSO

[CREATE FUNCTION](create-function.md), [CREATE PROCEDURE](create-procedure.md),
[REVOKE](revoke.md)
