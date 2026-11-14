# SELECT

[Statements](README.md)

## NAME

SELECT: read rows

## SYNOPSIS

```
<direct select statement: multiple rows> ::=
    <query expression> [ <order by clause> ] [ <updatability clause> ]

<query expression> ::=
    [ <with clause> ] <query expression body>

<with clause> ::=
    WITH [ RECURSIVE ] <with list element> [ { , <with list element> }... ]

<with list element> ::=
    <query name> [ ( <column name> [ { , <column name> }... ] ) ] AS ( <query expression> )
    [ <search clause> ] [ <cycle clause> ]

<search clause> ::=
    SEARCH { DEPTH | BREADTH } FIRST BY <column name> [ { , <column name> }... ]
    SET <sequence column>

<cycle clause> ::=
    CYCLE <column name> [ { , <column name> }... ] SET <cycle mark column>
    [ TO <cycle mark value> DEFAULT <non-cycle mark value> ] USING <path column>

<query expression body> ::=
      <query term>
    | <query expression body> { UNION | EXCEPT } [ ALL ] [ <corresponding spec> ] <query term>

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
    | <value expression> . * [ AS ( <column name> [ { , <column name> }... ] ) ]
    | <value expression> [ [ AS ] <column name> ]

<table expression> ::=
    FROM <table reference> [ { , <table reference> }... ]
    [ WHERE <search condition> ]
    [ GROUP BY [ ALL | DISTINCT ] <grouping element> [ { , <grouping element> }... ] ]
    [ HAVING <search condition> ]
    [ WINDOW <window name> AS <window specification> [ { , <window name> AS <window specification> }... ] ]

<grouping element> ::=
      <ordinary grouping set>
    | ROLLUP ( <ordinary grouping set> [ { , <ordinary grouping set> }... ] )
    | CUBE ( <ordinary grouping set> [ { , <ordinary grouping set> }... ] )
    | GROUPING SETS ( <grouping element> [ { , <grouping element> }... ] )
    | ( )

<ordinary grouping set> ::=
      <grouping column reference>
    | ( <grouping column reference> [ { , <grouping column reference> }... ] )

<grouping column reference> ::= <column reference> [ COLLATE <collation name> ]

<window function> ::=
    { ROW_NUMBER ( ) | RANK ( ) | DENSE_RANK ( ) | PERCENT_RANK ( ) | CUME_DIST ( ) | <set function> [ <filter clause> ]
    | NTILE ( <number of rows> )
    | { LEAD | LAG } ( <value expression> [ , <unsigned integer> [ , <value expression> ] ] ) [ <null treatment> ]
    | { FIRST_VALUE | LAST_VALUE } ( <value expression> ) [ <null treatment> ]
    | NTH_VALUE ( <value expression> , <number of rows> ) [ FROM { FIRST | LAST } ] [ <null treatment> ] }
    OVER { <window name> | <window specification> }

<number of rows> ::= <simple value specification> | ?

<null treatment> ::= RESPECT NULLS | IGNORE NULLS

<filter clause> ::= FILTER ( WHERE <search condition> )

<hypothetical set function> ::=
    { RANK | DENSE_RANK | PERCENT_RANK | CUME_DIST } ( <value expression> [ { , <value expression> }... ] )
    WITHIN GROUP ( ORDER BY <window sort key> [ { , <window sort key> }... ] ) [ <filter clause> ]

<inverse distribution function> ::=
    { PERCENTILE_CONT | PERCENTILE_DISC } ( <value expression> )
    WITHIN GROUP ( ORDER BY <window sort key> ) [ <filter clause> ] [ OVER { <window name> | <window specification> } ]

<nested window function> ::=
      ROW_NUMBER ( <row marker> )
    | VALUE_OF ( <value expression> AT <row marker> [ { + | - } <number of rows> ] [ , <value expression> ] )

<row marker> ::= BEGIN_PARTITION | BEGIN_FRAME | CURRENT_ROW | FRAME_ROW | END_FRAME | END_PARTITION

<window specification> ::=
    ( [ <window name> ]
      [ PARTITION BY <column reference> [ COLLATE <collation name> ] [ { , <column reference> [ COLLATE <collation name> ] }... ] ]
      [ ORDER BY <window sort key> [ { , <window sort key> }... ] ]
      [ { ROWS | RANGE | GROUPS } { <frame bound> | BETWEEN <frame bound> AND <frame bound> }
        [ EXCLUDE { CURRENT ROW | GROUP | TIES | NO OTHERS } ] ] )

<window sort key> ::= <value expression> [ COLLATE <collation name> ] [ ASC | DESC ] [ NULLS { FIRST | LAST } ]

<frame bound> ::=
    UNBOUNDED PRECEDING | <offset> PRECEDING | CURRENT ROW | <offset> FOLLOWING | UNBOUNDED FOLLOWING

<order by clause> ::=
    ORDER BY <sort specification> [ { , <sort specification> }... ]

<sort specification> ::= { <unsigned integer> | <column reference> } [ COLLATE <collation name> ] [ ASC | DESC ] [ NULLS { FIRST | LAST } ]

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

WITH names queries for the query after it, each read where its name is
written as a table is, and by each query after it in the list. WITH
RECURSIVE lets them read themselves and each other, each evaluated again
over the rows it gave until it gives no more, as [Query
Expressions](../06-queries.md#with-and-with-recursive) shows; past
EXECUTOR.MAX_ITERATIONS iterations the query is refused. SEARCH adds a
column that orders a recursive query's rows depth first or breadth first,
and CYCLE a column marking each row that closes a cycle and a column of
the path to it, ending the path there ([SEARCH and
CYCLE](../06-queries.md#search-and-cycle)).

A window function computes a value of each row out of the rows of its
partition, ordered, or of a frame of rows around it, as [Window
functions](../06-queries.md#window-functions) shows. Windows are
computed after HAVING and before DISTINCT and ORDER BY.

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
   function is a grouping column. GROUPING, in the select list or HAVING
   of its query and in no set function, names grouping columns of that
   query, at most 59 of them.
5. The operands of a set operation have as many columns, of comparable
   types.
6. The queries of a WITH have names of their own, and columns of names
   of their own, or a column list of as many names. Of WITH RECURSIVE,
   each has a column list; each that reads itself or another is a UNION
   of a query that reads none of them; and each reads them once in a
   query specification, in its FROM clause and not on the side of an
   outer join that nulls give, beside no set function, under no
   INTERSECT ALL or EXCEPT ALL and not after EXCEPT.
7. A query of WITH RECURSIVE with SEARCH or CYCLE reads itself and no
   other query of its list, and is a UNION of a query that does not read
   it and a query specification that reads it in its FROM clause. The
   columns BY and CYCLE name are of its column list, each once. The
   sequence, cycle mark and path columns are new names, each different.
   The marks are two different one-character strings, or TRUE and FALSE.
8. A partitioned operand is of a LEFT, RIGHT or FULL join: the second
   of a LEFT join, the first of a RIGHT join, either or both of a FULL
   join. Each column PARTITION BY names is one column of it, not a
   column NATURAL or USING joins on, and a collection derived table in a
   partitioned second operand reads nothing of the first.
9. A window function is in the select list of a query specification
   alone. The WINDOW clause names each window once, and a window name
   names one of them. A window built on another adds no PARTITION BY, no
   ORDER BY where the other has one, and builds on none with a frame.
   RANK, DENSE_RANK, PERCENT_RANK, CUME_DIST, NTILE, LEAD, LAG and a
   frame of GROUPS need an ORDER BY, and no rank, NTILE, LEAD or LAG
   takes a frame. The number of tiles of NTILE and the row of NTH_VALUE
   are integers. The default of LEAD or LAG is of a type each assignable
   to its first argument's, to whose type it is cast. A frame's start is not after
   its end. An offset is a literal or a parameter, an integer of ROWS and
   GROUPS. RANGE with an offset orders by one key, a number, a datetime
   or an interval, the offset a number or an interval of it. A nested
   window function is only in the argument of a set function a window
   function contains, outside any set function of the groups. The
   number of rows after its marker is an integer, and VALUE_OF's value
   and default have a type in common.
10. A `<filter clause>` follows a set function, COUNT(*), ARRAY_AGG or
    LISTAGG, not GROUPING, and holds no set function, subquery, window
    function, nested window function or outer reference. That of a set
    function of an enclosing query names no column.
11. A hypothetical set function has as many values as sort keys, each
    value and its key of compatible types, and holds no set function,
    subquery, window function or nested window function. Its values
    name only grouping columns of their query and outer references. One
    whose sort keys' columns are all outer references is the query's of
    those columns, and is where that query may have a set function. It
    is followed by no OVER.
12. An inverse distribution function has one sort key, a number or an
    interval, and its fraction is a number. It holds no set function,
    subquery, window function or nested window function. Its fraction
    names only grouping columns of its query and outer references. One
    whose sort key's columns are all outer references is the query's of
    those columns, as rule 11 says. Its window has no ORDER BY and no
    frame.

## PRIVILEGES

SELECT on each table and view the query names.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken: a table or column not found or ambiguous, a sort column not of the result, a column not grouped, operands of a set operation not alike; or FOR UPDATE is written |
| 2A000 | -104 | Its user lacks SELECT on a table or view |
| 21000 | -811 | A scalar subquery gives more than one row |
| TB918 | -918 | A recursive query takes more iterations than EXECUTOR.MAX_ITERATIONS |
| 22xxx | -301 | A data exception as a value is computed: division by zero (22012), a number out of range (22003), the fraction of PERCENTILE_CONT or PERCENTILE_DISC below 0 or above 1 (22003), a window frame's offset negative or null (22013), a number of tiles of NTILE below 1 (22014), a row of NTH_VALUE below 1 (22016), and the others of [Appendix A](../../appendix/a-sqlstate.md) |
| 2202F | -301 | A path or a DEPTH FIRST sequence would hold more than 65,535 rows |
| 0A000 | -900 | Valid SQL not processed: a nesting deeper than the limits of [Appendix D](../../appendix/d-limits.md); a GROUP BY of more than 4,096 grouping sets, or GROUPING of more than 59 columns; an anchor of WITH RECURSIVE of UNION CORRESPONDING without BY, or within its query's body; SEARCH or CYCLE of a UNION CORRESPONDING, or of a query that selects `*` of a NATURAL join or a join USING columns; COUNT(DISTINCT ...) over a window of an ordering or a frame, Feature T627 |

An error is placed at the construct it is about, its character position
told.

A condition is evaluated on every row it applies to, so whether an error
is raised does not depend on how the query is run. Where several rows or
conditions would raise one, which of their errors is raised may change
with the plan: as the tables grow, or a setting changes.

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
(3 rows, with a warning: SQLSTATE 01003)
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
table expression [§7.3]; X3.135-1986 §5.25 and §8.3. WITH and WITH
RECURSIVE are the `<with clause>` of ISO/IEC 9075-2:2023 §7.17, Features
T121, T122, T131 and T132, and SEARCH and CYCLE its `<search or cycle
clause>` [§7.18], with Feature T133. The fields of a row, `R.*`, are its `<all
fields reference>` [§7.16], and UNNEST its `<collection derived table>`
[§7.6]. PARTITION BY on an operand of a join is its `<partitioned join
table>` [§7.10], Feature F403. ROLLUP, CUBE, GROUPING SETS and the empty
grouping set are the `<group by clause>` of §7.13 and GROUPING its
`<grouping operation>` [§6.9], Features T431, T432 and T433, and GROUP BY
DISTINCT Feature T434. Window functions are the `<window function>` of
§6.10 and the WINDOW clause its `<window clause>` [§7.15], Features T611
and T620, with the window parts of T612. NTILE is Feature T614, LEAD
and LAG T615, IGNORE NULLS T616, FIRST_VALUE and LAST_VALUE T617,
NTH_VALUE T618, and the `<nested window function>` [§6.11] T619. SELECT
... INTO is
the `<select statement: single row>` [§13.5; X3.135-1986 §8.10]. FOR
READ ONLY, of a `<cursor specification>` [§13.1], is taken in a direct
SELECT too. Where the null value sorts is implementation-defined
[X3.135-1986 §8.3 GR3b vii]: here, as greater than every value. NULLS
FIRST and NULLS LAST are the `<null ordering>` of §10.10, Feature T611.

## SEE ALSO

[DECLARE CURSOR](declare-cursor.md), [INSERT](insert.md),
[Query Expressions](../06-queries.md),
[Retrieving Data](../../user/04-queries.md)
