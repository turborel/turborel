# Query Expressions

[Contents](README.md)

A query expression gives a table: of rows from tables, joined, filtered,
grouped and selected, or of rows written out, and of those combined by
set operations. [SELECT](statements/select.md) gives the syntax whole;
this chapter its parts. The examples are of STAFF, of [Value
Expressions](04-expressions.md), and of a table DEPT:

```sql
CREATE TABLE DEPT (DEPT CHAR(3) PRIMARY KEY, TITLE VARCHAR(20));
INSERT INTO DEPT VALUES ('ENG', 'Engineering'), ('HR', 'People');
```

```
OK
OK
```

## Table references and joins

The FROM clause names the tables a query reads. Of more than one, it
reads every combination of their rows, each row of one with each of the
other, which WHERE then filters. A table may be given a **correlation
name**, `STAFF S`, which then names it in the query.

A **derived table** is a query in parentheses, given a correlation name
and, if written, names for its columns: `(SELECT ...) AS C (D, N)`.

A **joined table** joins two tables in the FROM clause:

| Join | Rows |
|---|---|
| `A [INNER] JOIN B ON c` | each pair of rows for which c is true |
| `A LEFT [OUTER] JOIN B ON c` | those, and each row of A that pairs with none, B's columns null |
| `A RIGHT [OUTER] JOIN B ON c` | those, and each row of B that pairs with none |
| `A FULL [OUTER] JOIN B ON c` | those, and each row of either that pairs with none |
| `A CROSS JOIN B` | every pair |
| `A UNION JOIN B` | each row of A, B's columns null, and each of B, A's null |

`USING (c, ...)` joins on the columns named, equal in both; NATURAL on
every column of one name in both. Either gives each such column once.
Joins nest, and may be written in parentheses.

```sql
SELECT S.NAME, D.TITLE FROM STAFF S, DEPT D WHERE S.DEPT = D.DEPT ORDER BY S.NAME;
SELECT NAME, TITLE FROM STAFF NATURAL LEFT JOIN DEPT ORDER BY NAME;
SELECT NAME, TITLE FROM STAFF FULL JOIN DEPT ON STAFF.DEPT = DEPT.DEPT ORDER BY NAME, TITLE;
SELECT * FROM (SELECT DEPT, COUNT(*) FROM STAFF GROUP BY DEPT) AS C (D, N) WHERE N > 1;
```

```
 NAME                           | TITLE
--------------------------------+----------------------
 Ada                            | Engineering
 Grace                          | Engineering
(2 rows)
 NAME                           | TITLE
--------------------------------+----------------------
 Ada                            | Engineering
 Barbara                        | NULL
 Edsger                         | NULL
 Grace                          | Engineering
(4 rows)
 NAME                           | TITLE
--------------------------------+----------------------
 Ada                            | Engineering
 Barbara                        | NULL
 Edsger                         | NULL
 Grace                          | Engineering
 NULL                           | People
(5 rows)
 D   |                    N
-----+----------------------
 ENG |                    2
(1 row)
```

### Partitioned joins

An operand of an outer join may be **partitioned**: `B PARTITION BY
(c, ...)` splits B into partitions, one for each distinct value of its
columns c, a null value making one too. The other operand is then joined
with each partition in turn. A row that pairs with none of a partition's
rows is still given, with that partition's values in B's columns c and
nulls in the rest. That fills a series's gaps once for each group.

| Join | Partitioned operand |
|---|---|
| `A LEFT JOIN B PARTITION BY (c) ON ...` | the second |
| `A PARTITION BY (c) RIGHT JOIN B ON ...` | the first |
| `A FULL JOIN B PARTITION BY (c) ON ...` | either, or both |

The columns are of the partitioned table, which is a table, a derived
table, a collection derived table or a joined table in parentheses. A
partitioned table that is empty has no partitions, so the join gives no
row of the other.

```sql
SELECT S.NAME, D.DEPT, S.SALARY FROM DEPT D LEFT JOIN STAFF S PARTITION BY (NAME)
  ON S.DEPT = D.DEPT ORDER BY S.NAME, D.DEPT;
```

```
 NAME                           | DEPT |      SALARY
--------------------------------+------+-------------
 Ada                            | ENG  |     7200.00
 Ada                            | HR   |        NULL
 Barbara                        | ENG  |        NULL
 Barbara                        | HR   |        NULL
 Edsger                         | ENG  |        NULL
 Edsger                         | HR   |        NULL
 Grace                          | ENG  |     8100.00
 Grace                          | HR   |        NULL
(8 rows)
```

## UNNEST

UNNEST in the FROM clause gives the elements of arrays as rows, a row of
each place from the first to the last of the longest array, each column
an array's element there, null past its last. WITH ORDINALITY adds a
column of the place, from 1, of type INTEGER. An array of rows gives a
column of each field. UNNEST reads the tables before it in the FROM
clause, each of their rows giving its own rows, so a table's array column
is unnested row by row; a LEFT join keeps a row whose array gives none.

```sql
CREATE TABLE TEAMS (DEPT CHAR(3) PRIMARY KEY, MEMBERS VARCHAR(30) ARRAY[5]);
INSERT INTO TEAMS VALUES ('ENG', ARRAY['Ada', 'Grace']), ('OPS', ARRAY['Edsger']), ('HR', ARRAY[]);
SELECT T.DEPT, M.WHO, M.N FROM TEAMS T, UNNEST(T.MEMBERS) WITH ORDINALITY AS M (WHO, N) ORDER BY T.DEPT, M.N;
SELECT T.DEPT, M.WHO FROM TEAMS T LEFT JOIN UNNEST(T.MEMBERS) AS M (WHO) ON TRUE ORDER BY T.DEPT, M.WHO;
SELECT * FROM UNNEST(ARRAY[1, 2], ARRAY['a']) AS U (N, L);
```

```
OK
OK
 DEPT | WHO                            |            N
------+--------------------------------+--------------
 ENG  | Ada                            |            1
 ENG  | Grace                          |            2
 OPS  | Edsger                         |            1
(3 rows)
 DEPT | WHO
------+--------------------------------
 ENG  | Ada
 ENG  | Grace
 HR   | NULL
 OPS  | Edsger
(4 rows)
   N | L
-----+---
   1 | a
   2 | NULL
(2 rows)
```

## The WHERE, GROUP BY and HAVING clauses

**WHERE** keeps the rows its condition is true of.

**GROUP BY** makes groups of the rows, one of each value of its columns,
the nulls one group. A query then gives a row of each group: what it
selects is a grouping column, or of a set function, computed of the
group. **HAVING** keeps the groups its condition is true of; without
GROUP BY, it makes the whole table one group, even of no rows. A
grouped query of no groups gives no row, though it selects only set
functions: GROUP BY of no rows makes no groups, and HAVING may keep
none.

```sql
SELECT DEPT, COUNT(*) AS N, SUM(SALARY) AS TOTAL FROM STAFF
    WHERE SALARY IS NOT NULL GROUP BY DEPT HAVING COUNT(*) > 1;
SELECT DEPT, NAME FROM STAFF GROUP BY DEPT;
```

```
 DEPT |                    N |                TOTAL
------+----------------------+----------------------
 ENG  |                    2 |             15300.00
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): outside a set function, a grouped query may only reference grouping columns (found STAFF.NAME) [X3.135-1986 §5.25 SR7] (at character 13)
```

## The select list

The select list says the columns of the result: `*`, every column of
the FROM clause's tables in order; `S.*`, every column of one; or value
expressions, each named by `AS` or, if a column, as it. DISTINCT keeps
one of each row alike, nulls alike; ALL, the default, every row.

```sql
SELECT DISTINCT DEPT FROM STAFF ORDER BY DEPT;
SELECT S.*, D.TITLE FROM STAFF S, DEPT D WHERE S.DEPT = D.DEPT AND S.ID = 1;
SELECT NAME AS WHO, SALARY / 12 AS MONTHLY FROM STAFF WHERE ID = 1;
```

```
 DEPT
------
 ENG
 OPS
 NULL
(3 rows)
           ID | NAME                           | DEPT |      SALARY | TITLE
--------------+--------------------------------+------+-------------+----------------------
            1 | Ada                            | ENG  |     7200.00 | Engineering
(1 row)
 WHO                            |              MONTHLY
--------------------------------+----------------------
 Ada                            |               600.00
(1 row)
```

ORDER BY sorts by columns of the result alone, named or by position: a
column the query does not select is not a sort key.

`R.*` of a value R of a row type gives a column of each of its fields,
named as the fields are, and `R.* AS (A, B)` names them.

```sql
SELECT P.* AS (WHO_ID, WHO) FROM (SELECT ROW(ID, NAME) AS P FROM STAFF WHERE ID = 1) AS D;
```

```
       WHO_ID | WHO
--------------+--------------------------------
            1 | Ada
(1 row)
```

## Set operations

UNION gives the rows of either query, EXCEPT those of the first not in
the second, INTERSECT those of both; each once, unless ALL keeps each
as often as the operation gives it. INTERSECT binds tighter than UNION
and EXCEPT. The two queries are of one degree, their columns paired by
position, of comparable types; CORRESPONDING pairs them by name instead,
of the columns both have, or those of BY (...).

```sql
SELECT DEPT FROM STAFF UNION SELECT DEPT FROM DEPT ORDER BY 1;
SELECT DEPT FROM DEPT EXCEPT SELECT DEPT FROM STAFF;
SELECT ID, DEPT FROM STAFF UNION CORRESPONDING SELECT DEPT, TITLE FROM DEPT ORDER BY 1;
SELECT ID FROM STAFF UNION SELECT TITLE FROM DEPT;
```

```
 DEPT
------
 ENG
 HR
 OPS
 NULL
(4 rows)
 DEPT
------
 HR
(1 row)
 DEPT
------
 ENG
 HR
 OPS
 NULL
(4 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): character strings and numbers are brought together [ISO/IEC 9075:1992 §7.10 SR12b ii, §9.3 SR2, SR3a] (at character 21)
```

## Table value constructors

`VALUES (a, b), (c, d), ...` is a table of the rows written, its columns
of a type each row's values fit. It is a query of its own, and gives
INSERT its rows. `TABLE T` is the whole of T.

```sql
VALUES (1, 'one'), (2, 'two');
SELECT * FROM (VALUES (1, 'one'), (2, 'two')) AS T (N, W) WHERE N = 2;
TABLE DEPT;
```

```
     |
-----+-----
   1 | one
   2 | two
(2 rows)
   N | W
-----+-----
   2 | two
(1 row)
 DEPT | TITLE
------+----------------------
 ENG  | Engineering
 HR   | People
(2 rows)
```

A row of one value may be written without parentheses: `VALUES 1, 2` is
two rows.

## WITH and WITH RECURSIVE

WITH names queries for the query after it. Each is read where its name
is written as a table is, by the query and by each query after it in the
list, of the columns its column list names or else its query's.

```sql
WITH BIG (DEPT, PEOPLE) AS (SELECT DEPT, COUNT(*) FROM STAFF GROUP BY DEPT)
SELECT DEPT FROM BIG WHERE PEOPLE > 1;
```

```
 DEPT
------
 ENG
(1 row)
```

WITH RECURSIVE lets a query read itself, and the queries of the list
read each other. Each such query is a UNION of an anchor, which reads
none of them, and queries that read them. The anchor's rows come first;
then each query is evaluated again over the rows they gave, until it
gives no more. UNION keeps each row once, so a query over a graph with
cycles ends; UNION ALL keeps every row. Every column list is written.

```sql
CREATE TABLE BOSS (ID INTEGER, REPORTS_TO INTEGER);
INSERT INTO BOSS VALUES (1, NULL), (2, 1), (3, 1), (4, 3);
WITH RECURSIVE UNDER (ID, DEPTH) AS (
  SELECT ID, 0 FROM BOSS WHERE ID = 1
  UNION ALL
  SELECT BOSS.ID, DEPTH + 1 FROM UNDER, BOSS WHERE BOSS.REPORTS_TO = UNDER.ID)
SELECT * FROM UNDER ORDER BY ID;
```

```
OK
OK
           ID | DEPTH
--------------+-------
            1 |     0
            2 |     1
            3 |     1
            4 |     2
(4 rows)
```

A recursive query's columns are of its anchor's types, so the anchor
gives the type a column grows in. Here `0` is NUMERIC(1,0), which holds
depths up to 9, and `CAST(0 AS INTEGER)` would hold more.

A query reads the queries of its list once in each query specification,
in its FROM clause or a derived table there, beside no set function, not
on the side of an outer join that nulls give, under no INTERSECT ALL or
EXCEPT ALL and not after EXCEPT; and each cycle of queries reading each
other passes through an anchor. A query that never stops growing is
refused once it has taken EXECUTOR.MAX_ITERATIONS iterations, a million
unless the session sets another number; 0 sets none.

```sql
SET EXECUTOR.MAX_ITERATIONS = 100;
WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT I + 1 FROM N)
SELECT COUNT(*) FROM N;
SET EXECUTOR.MAX_ITERATIONS = 1000000;
```

```
OK
turborel: SQLCODE -918 (SQLSTATE TB918): a recursive query reached no fixpoint in 100 iterations, as EXECUTOR.MAX_ITERATIONS allows [ISO/IEC 9075-2:2023 §7.17 GR2c ii 4]
OK
```

### SEARCH and CYCLE

A recursive query may be followed by a CYCLE clause, a SEARCH clause, or
both. CYCLE names columns whose values, met again along a path, close a
cycle. It adds a mark column, TRUE on a row that closes a cycle and FALSE
on the others, and a path column, an array of a row of those columns for
each step from the anchor's row. A row that closes a cycle is extended no
further, so UNION ALL over a graph with cycles ends. TO and DEFAULT give
other marks, two different one-character strings or TRUE and FALSE.

```sql
CREATE TABLE ROAD (A INTEGER, B INTEGER);
INSERT INTO ROAD VALUES (1, 2), (2, 3), (3, 1);
WITH RECURSIVE TRIP (TOWN) AS (
  VALUES (1)
  UNION ALL
  SELECT ROAD.B FROM TRIP, ROAD WHERE ROAD.A = TRIP.TOWN)
  CYCLE TOWN SET LOOPED USING ROUTE
SELECT * FROM TRIP;
```

```
OK
OK
 TOWN | LOOPED | ROUTE
------+--------+--------------------------------------------------------------
    1 | FALSE  | ARRAY[ROW(1)]
    2 | FALSE  | ARRAY[ROW(1), ROW(2)]
    3 | FALSE  | ARRAY[ROW(1), ROW(2), ROW(3)]
    1 | TRUE   | ARRAY[ROW(1), ROW(2), ROW(3), ROW(1)]
(4 rows)
```

SEARCH DEPTH FIRST adds a column of the path of rows of the columns BY
names, from the anchor's. SEARCH BREADTH FIRST adds a row of the level,
from 0, and those columns, so ordering by it gives the rows level by
level. An array is not ordered, so ORDER BY takes BREADTH FIRST's column
and not DEPTH FIRST's. UNION compares rows and cannot compare arrays, so
CYCLE and DEPTH FIRST need UNION ALL. A path holds at most 65,535 rows.

```sql
WITH RECURSIVE UNDER (ID) AS (
  SELECT ID FROM BOSS WHERE ID = 1
  UNION ALL
  SELECT BOSS.ID FROM UNDER, BOSS WHERE BOSS.REPORTS_TO = UNDER.ID)
  SEARCH BREADTH FIRST BY ID SET SEQ
SELECT ID, SEQ FROM UNDER ORDER BY SEQ;
```

```
           ID | SEQ
--------------+-----------------------------------
            1 | ROW(0, 1)
            2 | ROW(1, 2)
            3 | ROW(1, 3)
            4 | ROW(2, 4)
(4 rows)
```

The standard: ISO/IEC 9075:1992 §7.1–§7.11; X3.135-1986 §5.19–§5.25;
WITH, ISO/IEC 9075-2:2023 §7.17, and SEARCH and CYCLE, §7.18.
