# SET (assignment)

[Statements](README.md)

## NAME

SET: assign a value to an SQL variable or parameter *(in a compound statement)*

## SYNOPSIS

```
<assignment statement> ::=
      SET <assignment target> = { <value expression> | NULL }
    | SET ( <assignment target> [ , <assignment target> ]... ) = <assigned row>

<assignment target> ::=
    <target> [ . <field name> ]...

<target> ::=
      <variable name>
    | <label> . <variable name>
    | <parameter name>
    | <routine name> . <parameter name>
    | <new transition variable name> . <column name>
    | <target> [ <position> ]

<assigned row> ::=
      ( <value expression> [ , <value expression> ]... )
    | ( <query expression> )
```

SET is a key word.

## DESCRIPTION

SET assigns a value to a variable of a [compound statement](begin.md),
an SQL parameter of the routine it is in, or a column of a BEFORE
trigger's new row. It stands in the body of a routine, in a trigger's
action, or in a compound statement run directly. Elsewhere, SET is one
of the session's statements, such as [SET SCHEMA](set-schema.md).

The value is stored in the target as INSERT stores a value in a column.
SET of NULL gives the target a null.

Several targets are assigned at once from a row of as many values.
The row is a list of values, or a subquery of one row. A subquery of
no row assigns each target a null. The values are all read before any
is assigned, so `SET (A, B) = (B, A)` swaps A and B.

A target of a row type may be followed by the name of one of its
fields, so SET assigns that field alone. A target may be one element of
an array, `A[i]`, as a target of SELECT INTO may be (see
[CREATE PROCEDURE](create-procedure.md)).

A target written `L.V` is the variable V of the compound statement
labelled L, though an inner one of the name hides it. The values may
read variables and parameters, and a scalar subquery may read tables.

## RULES

1. Each target is a variable or parameter in scope, or a new row's
   column in a BEFORE trigger.
2. Each value is of a type its target may store.
3. A row has as many values as there are targets.
4. A field is a field of its target's row type.

## PRIVILEGES

Those of the values, such as SELECT on a table a subquery reads.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -103 | A rule above is broken |
| 21000 | -811 | The row's subquery gives more than one row |
| 2202A | -301 | A field is assigned of a target whose row is null |
| 22001 | -301 | A value is too long for its target |
| 22003 | -301 | A value is out of its target's range |

## EXAMPLES

An outer variable named by its label, a row assigned at once, and a
count read from a table:

```sql
CREATE TABLE PAIRS (A INTEGER, B INTEGER);
INSERT INTO PAIRS VALUES (1, 10), (2, 20);
BEGIN
  DECLARE P, Q INTEGER;
  O: BEGIN
    DECLARE X INTEGER DEFAULT 1;
    IN1: BEGIN
      DECLARE X INTEGER DEFAULT 2;
      SET O.X = O.X + X * 10;
    END IN1;
    SET (P, Q) = (X, X + 1);
    INSERT INTO PAIRS VALUES (P, Q);
    SET (P, Q) = (SELECT A, B FROM PAIRS WHERE A = 2);
    INSERT INTO PAIRS VALUES (P + 100, Q);
    SET (P, Q) = (Q, P);
    INSERT INTO PAIRS VALUES (P, Q);
    SET Q = (SELECT COUNT(*) FROM PAIRS);
    SET P = NULL;
    INSERT INTO PAIRS VALUES (P, Q);
  END O;
END;
SELECT A, B FROM PAIRS ORDER BY A;
```

```
OK
OK
OK
            A |            B
--------------+--------------
            1 |           10
            2 |           20
           20 |            2
           21 |           22
          102 |           20
         NULL |            5
(6 rows)
```

Refused:

```sql
BEGIN DECLARE P, Q INTEGER; SET (P, Q) = (SELECT A, B FROM PAIRS); END;
BEGIN DECLARE P INTEGER; SET P = 'text'; END;
BEGIN DECLARE P, Q INTEGER; SET (P, Q) = (1, 2, 3); END;
```

```
turborel: SQLCODE -811 (SQLSTATE 21000): the assigned row's subquery gives more than one row [ISO/IEC 9075:1992 §7.11 GR1; ISO/IEC 9075-4:2006 draft §14.5 SR1f]
turborel: SQLCODE -103 (SQLSTATE 42000): a value of CHARACTER(4) cannot be stored in P (INTEGER) [ISO/IEC 9075-2:2023 §9.2 SR2] (at character 29)
turborel: SQLCODE -103 (SQLSTATE 42000): 2 targets are assigned a row of degree 3 [ISO/IEC 9075-4:2006 draft §14.5 SR1b] (at character 28)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §14.5, Feature P002. The list of targets is
Feature P006, `L.V` Feature P005. A field target is Feature T051, and
an element target Feature S097, of ISO/IEC 9075-2:2023. SQL-86 and
SQL-92 have no such statement.

## SEE ALSO

[BEGIN](begin.md), [CREATE PROCEDURE](create-procedure.md),
[CREATE TRIGGER](create-trigger.md), [SELECT](select.md)
