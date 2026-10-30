# SELECT

[Statements](README.md)

## NAME

SELECT: read rows

## SYNOPSIS

```
<direct select statement: multiple rows> ::=
    <query expression> [ <order by clause> ] [ <updatability clause> ]

<query expression> ::=
      <query term>
    | <query expression> { UNION | EXCEPT } [ ALL ] [ <corresponding spec> ] <query term>

<query term> ::=
      <query primary>
    | <query term> INTERSECT [ ALL ] [ <corresponding spec> ] <query primary>

<query primary> ::=
      <query specification>
    | VALUES <row value constructor> [ { , <row value constructor> }... ]
    | TABLE <table name>
    | <joined table>
    | ( <query expression> )

<corresponding spec> ::= CORRESPONDING [ BY ( <column name> [ { , <column name> }... ] ) ]

<query specification> ::=
    SELECT [ ALL | DISTINCT ] <select list> <table expression>

<select list> ::=
      *
    | <select sublist> [ { , <select sublist> }... ]

<select sublist> ::=
      <qualifier> . *
    | <value expression> [ [ AS ] <column name> ]

<table expression> ::=
    FROM <table reference> [ { , <table reference> }... ]
    [ WHERE <search condition> ]
    [ GROUP BY <column reference> [ { , <column reference> }... ] ]
    [ HAVING <search condition> ]

<order by clause> ::=
    ORDER BY <sort specification> [ { , <sort specification> }... ]

<sort specification> ::= { <unsigned integer> | <column reference> } [ ASC | DESC ]

<updatability clause> ::= FOR READ ONLY
```

The `<table reference>`s, joins and search conditions are described in
[Query Expressions](../06-queries.md) and
[Predicates and Search Conditions](../05-predicates.md); value
expressions in [Value Expressions](../04-expressions.md).

## DESCRIPTION

A query, run on its own, gives its rows to the client, which shows them
as they come. TABLE T is `SELECT * FROM T`; a `<joined table>` on its
own is `SELECT * FROM` it.

ORDER BY sorts the rows by each sort specification in turn: a column of
the result, named or by its position from 1, ascending unless DESC is
written. Without ORDER BY the order of the rows is not defined. The null
value sorts as greater than every other value: last ascending, first
descending.

A query begins a transaction if none is active, and reads as its
isolation level says ([SET TRANSACTION](set-transaction.md)).

A query is not given a name its rows can be updated through, so FOR
UPDATE is refused; [DECLARE CURSOR](declare-cursor.md) names one, in a
program. SELECT ... INTO, which reads one row into a program's
parameters, is [a statement of a program too](#select-into).

## RULES

1. Each table and view named exists, and its user holds SELECT on it.
2. Each column named is of a table in scope; one named without a
   qualifier is of exactly one.
3. A `<sort specification>` names a column of the result, or gives its
   position.
4. In a query with GROUP BY or a set function, a column outside a set
   function is a grouping column.
5. The operands of a set operation have as many columns, of comparable
   types.

## PRIVILEGES

SELECT on each table and view the query names.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken: a table or column not found or ambiguous, a sort column not of the result, a column not grouped, operands of a set operation not alike; or FOR UPDATE is written |
| 2A000 | -104 | Its user lacks SELECT on a table or view |
| 21000 | -811 | A scalar subquery gives more than one row |
| 22xxx | -301 | A data exception as a value is computed: division by zero (22012), a number out of range (22003), and the others of [Appendix A](../../appendix/a-sqlstate.md) |
| 0A000 | -900 | Valid SQL not processed: a COLLATE clause, or a nesting deeper than the limits of [Appendix D](../../appendix/d-limits.md) |

An error is placed at the construct it is about, its character position
told.

## EXAMPLES

The table these examples read:

```sql
CREATE TABLE STAFF (ID INTEGER, NAME VARCHAR(12), DEPT CHAR(3), SALARY DECIMAL(8,2));
INSERT INTO STAFF VALUES
    (1, 'Ada', 'ENG', 7200.00), (2, 'Grace', 'ENG', 8100.00),
    (3, 'Edgar', 'OPS', 5400.00), (4, 'Barbara', NULL, NULL);
COMMIT;
```

Rows chosen and sorted:

```sql
SELECT NAME, SALARY FROM STAFF WHERE SALARY > 6000 ORDER BY SALARY DESC;
```

```
 NAME         |     SALARY
--------------+------------
 Grace        |    8100.00
 Ada          |    7200.00
(2 rows)
```

Groups:

```sql
SELECT DEPT, COUNT(*) AS PEOPLE, AVG(SALARY) FROM STAFF GROUP BY DEPT ORDER BY 1;
```

```
 DEPT |               PEOPLE |
------+----------------------+----------------------
 ENG  |                    2 |            7650.0000
 OPS  |                    1 |            5400.0000
 NULL |                    1 |                 NULL
(3 rows)
```

A whole table, and rows written out:

```sql
TABLE STAFF;
VALUES (1, 'one'), (2, 'two');
```

```
           ID | NAME         | DEPT |     SALARY
--------------+--------------+------+------------
            1 | Ada          | ENG  |    7200.00
            2 | Grace        | ENG  |    8100.00
            3 | Edgar        | OPS  |    5400.00
            4 | Barbara      | NULL |       NULL
(4 rows)
     |
-----+-----
   1 | one
   2 | two
(2 rows)
```

A column not found:

```sql
SELECT NOPE FROM STAFF;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): no table in the FROM clause has a column NOPE [X3.135-1986 §5.7 SR3b] (at character 7)
```

## SELECT INTO

```
<select statement: single row> ::=
    SELECT [ ALL | DISTINCT ] <select list>
    INTO <target specification> [ { , <target specification> }... ]
    <table expression>
```

SELECT ... INTO reads one row into a program's parameters, one target
for each column. It runs only in a program that embeds turborel, whose
procedure declares the parameters; it is not direct SQL, and the
`turborel` client does not run it. No row sets SQLCODE 100 (SQLSTATE
02000), the targets unchanged; more than one is SQLSTATE 21000, SQLCODE
-811; targets not as many as the columns are SQLSTATE 2A000, SQLCODE
-103. A value is assigned to its target as [FETCH](fetch.md) assigns
it: a string cut to fit is a warning, SQLSTATE 01004.

## STANDARD

ISO/IEC 9075:1992 §20.1, the `<direct select statement: multiple
rows>`, and its query expression [§7.10], query specification [§7.9] and
table expression [§7.3]; X3.135-1986 §5.25 and §8.3. SELECT ... INTO is
the `<select statement: single row>` [§13.5; X3.135-1986 §8.10]. FOR
READ ONLY, of a `<cursor specification>` [§13.1], is taken in a direct
SELECT too. Where the null value sorts is implementation-defined
[X3.135-1986 §8.3 GR3b vii]: here, as greater than every value.

## SEE ALSO

[DECLARE CURSOR](declare-cursor.md), [INSERT](insert.md),
[Query Expressions](../06-queries.md),
[Retrieving Data](../../user/04-queries.md)
