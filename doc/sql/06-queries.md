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

A typed table is read with the rows of its subtables, cut to its own
columns. `ONLY (t)` reads its own rows alone. Of the tables of
[Structured values and references](04-expressions.md#structured-values-and-references):

```sql
SELECT LABEL FROM ITEMS ORDER BY LABEL;
SELECT LABEL FROM ONLY (ITEMS);
```

```
 LABEL
------------
 BOOK
 LAMP
(2 rows)
 LABEL
------------
 LAMP
(1 row)
```

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

### Reading a table's history

A system-versioned table ([CREATE TABLE](statements/create-table.md))
is read as its current rows. Read **FOR SYSTEM_TIME**, after its name
and before its correlation name, it is read with its historical rows,
those whose system-time period the clause names:

| Clause | Rows |
|---|---|
| `AS OF t` | those current at t: begun at t or before, ended after it |
| `BETWEEN t1 AND t2` | those current at some time from t1 to t2, both included; none if t1 is after t2 |
| `BETWEEN SYMMETRIC t1 AND t2` | as BETWEEN, t1 and t2 exchanged if t1 is after t2 |
| `FROM t1 TO t2` | those current at some time from t1, included, to t2, excluded; none unless t1 is before t2 |

Each point in time is a datetime, cast to the type of the period's
start column, and reads no column, of the query or one it is in. `AS OF
CURRENT_TIMESTAMP` reads the current rows, as the name alone does. Read
any other way, a table is no updatable table, and no constraint reads
it so. Of a table of fares, the first fare is still read once it is
updated:

```sql
CREATE TABLE FARES (ITEM INTEGER PRIMARY KEY, PRICE DECIMAL(9,2),
    VALID_FROM TIMESTAMP(6) GENERATED ALWAYS AS ROW START,
    VALID_TO TIMESTAMP(6) GENERATED ALWAYS AS ROW END,
    PERIOD FOR SYSTEM_TIME (VALID_FROM, VALID_TO)) WITH SYSTEM VERSIONING;
INSERT INTO FARES (ITEM, PRICE) VALUES (1, 9.99);
COMMIT;
UPDATE FARES SET PRICE = 12.50;
COMMIT;
SELECT ITEM, PRICE FROM FARES FOR SYSTEM_TIME FROM DATE '2000-01-01' TO DATE '3000-01-01' ORDER BY PRICE;
SELECT ITEM, PRICE FROM FARES FOR SYSTEM_TIME AS OF CURRENT_TIMESTAMP;
SELECT COUNT(*) FROM FARES FOR SYSTEM_TIME AS OF DATE '2000-01-01';
```

```
OK
OK
OK
OK
OK
         ITEM |       PRICE
--------------+-------------
            1 |        9.99
            1 |       12.50
(2 rows)
         ITEM |       PRICE
--------------+-------------
            1 |       12.50
(1 row)

----------------------
                    0
(1 row)
```

A system-versioned table of an application-time period too, a
bitemporal table, records both when a fact held and when it was
recorded. Of a post changed for 2025, both what is held now and every
row recorded of 2025-06-01:

```sql
CREATE TABLE POSTS (ID INTEGER, DEPT CHAR(3),
    VF DATE NOT NULL, VT DATE NOT NULL, PERIOD FOR VALID (VF, VT),
    S TIMESTAMP(6) GENERATED ALWAYS AS ROW START,
    E TIMESTAMP(6) GENERATED ALWAYS AS ROW END,
    PERIOD FOR SYSTEM_TIME (S, E),
    PRIMARY KEY (ID, VALID WITHOUT OVERLAPS)) WITH SYSTEM VERSIONING;
INSERT INTO POSTS (ID, DEPT, VF, VT) VALUES (1, 'ENG', DATE '2024-01-01', DATE '2026-01-01');
COMMIT;
UPDATE POSTS FOR PORTION OF VALID FROM DATE '2025-01-01' TO DATE '2026-01-01' SET DEPT = 'OPS';
COMMIT;
SELECT DEPT, VF, VT FROM POSTS ORDER BY VF;
SELECT DEPT, VF, VT, E < CURRENT_TIMESTAMP AS ENDED
    FROM POSTS FOR SYSTEM_TIME FROM DATE '2000-01-01' TO DATE '3000-01-01'
    WHERE VALID CONTAINS DATE '2025-06-01' ORDER BY S;
```

```
OK
OK
OK
OK
OK
 DEPT | VF         | VT
------+------------+------------
 ENG  | 2024-01-01 | 2025-01-01
 OPS  | 2025-01-01 | 2026-01-01
(2 rows)
 DEPT | VF         | VT         | ENDED
------+------------+------------+-------
 ENG  | 2024-01-01 | 2026-01-01 | TRUE
 OPS  | 2025-01-01 | 2026-01-01 | FALSE
(2 rows)
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

### How joins are planned *(extension)*

The rows of a join are the standard's whatever order its tables are read
in, so the planner chooses the order, and the method of each join, that
it estimates cheapest. Of three or more tables it searches every order
that keeps the query's result, bushy ones among them, where one join's
result is joined with another's. Inner joins are reordered freely, and
an outer join where its rows are kept, such as a LEFT join moved below
an inner join of its preserved side. The search is exact for a query of
up to 13 tables, or more while their connected subsets number no more
than PLANNER.DP_BUDGET, and past that it is narrowed to orders near a
greedy one, so a query of a hundred tables is still planned at once.
Each join is a hash join, building the smaller side, an index
nested-loop join probing the other table's index, or a merge join where
a side is read in its key's order already and that order pays, as when
the query asks for it, or a range join. The rows of each set of tables are estimated
from the distinct values of the joined columns, an equality that others
imply counted once. Two tables joined on several columns are estimated
by the values those columns hold together, so a join on all of a key's
columns, or on a foreign key, keeps each row once, and no estimate
passes the most rows the most frequent values could give. A query with a condition that can raise an error, as
a division can, is planned in the greedy order described next, which
keeps the errors raised the same whatever the order.

A hash join's keys need not be columns. An equality of an expression
of one table's columns with one of the other's, as
`Q.K = X.TAG + X.K` or `Q.D = CAST(X.K AS DECIMAL(12,2))`, is a hash
join on the two values, each computed once a row of its table, not once
a pair. EXPLAIN shows the computed key as a column of a projection
under the join. The expressions must be deterministic and hold no
subquery, and the condition must raise no error by the columns'
declared types, as a sum of two INTEGER columns cannot.

A range join answers a condition that compares a value of each table
by order, as a band `A.X BETWEEN B.Y - 5 AND B.Y + 5` or the overlap of
two periods does, without judging every pair. It sorts one table once by
the compared value, grouped by any equalities of the two, and each row
of the other finds its matches between its bounds by binary search. A
second comparison of other values, as the overlap's other half, prunes
the rows found. A `<period predicate>` of two periods named by their
tables is read as the comparisons of their bounds, so OVERLAPS, CONTAINS,
PRECEDES and SUCCEEDS join so, and EQUALS and the IMMEDIATELY ones as
hash joins on the bounds' equalities. The rows are the nested loop's. A
table too large for MEMORY.WORK_BYTES is sorted and searched a part at a
time, the other read again for each part, so the rows come part by part,
and only ORDER BY sets their order. A
condition that could raise an error keeps the nested loop. SET
PLANNER.RANGE_JOINS = 'OFF' turns range joins off.

A hash join tells the table it probes with which keys its built rows
have, so the probing table's rows no key matches are dropped as they are
read. Predicate transfer sends such filters the other way too. Before
the joins of a chain of hash joins and LEFT joins run, its tables are read in turn,
from the smaller ones to the largest and back, along the joins between
them, each read filtered by the keys of the tables read before it. A
selective condition on the largest table then shrinks the tables it is
joined with before they are built, and so does one two joins away. The
planner reads a chain so only where each filter is expected to drop at
least half the rows of the table it filters, and the rows dropped are
worth more than the filters cost. A LEFT join's preserved table is
never filtered by the table that supplies its nulls. The rows read are held within
MEMORY.WORK_BYTES, a table past it read again with its filters. The
rows, and their order, are the same either way. A chain of a condition
that could raise an error, one under FETCH FIRST without ORDER BY, and
one run in parallel are joined as planned. SET
PLANNER.PREDICATE_TRANSFER = 'OFF' turns it off.

An equality repeated in each operand of an OR joins its tables as if
written once, so `(P.K = L.K AND P.B = 'B1') OR (P.K = L.K AND P.B =
'B2')` is a join on `P.K = L.K`, not a product filtered afterward. Each
table is also filtered first by the OR of its own conditions, here
`P.B = 'B1' OR P.B = 'B2'`. OPTIMIZER.FACTOR_DISJUNCTIONS turns this off.

An index nested-loop join is taken only where its probes would stay
cheap even if the other side gave several times the rows estimated, at
most as many as the statistics allow, since an estimate too low makes
its probes far dearer than one hash join. PLANNER.ESTIMATE_MARGIN sets
how many times; 1 trusts the estimate. Where the other side reads a
range of an index, its rows are the fewer of the statistics' estimate
and the index's estimate of the range.

A table whose every column of a primary key or unique constraint is
held equal to a value, a parameter or a variable is read by that key's
probe, whatever its size or statistics, as at most one row matches. A
range of a primary or unique key, or of an index holding every column
read, whose first columns are held equal, finds only rows a scan would
read too, so it is never priced above that scan. Where the two are
priced alike, the range is read if it is estimated to read fewer rows
than the scan, and fewer than the table's rows over
PLANNER.ESTIMATE_MARGIN.

A few keys given as a set probe the table's index once each, where the
probes cost less than reading the table. An IN of a subquery, of a row
value or of VALUES, and an EXISTS whose subquery equates the key with its
own columns, probe once for each distinct key of the subquery's rows. A
key compared with a subquery that gives one row at most, an ungrouped
MIN or MAX or one fetched first, probes from that row, so `O = (SELECT
MIN(O) ...)` reads one key, and `O >= (SELECT MAX(O) - 4 ...)` the keys
from that bound on. A join whose condition bounds the key's next column
after its equalities by the other table's values, as `L.O < C.NEXT AND
L.O >= C.NEXT - 20` does, probes the keys between the bounds alone. An
UNNEST of arrays is estimated by their elements, so a LEFT JOIN or a
MERGE from a few array elements probes the key too, and a searched UPDATE
or DELETE whose condition is one of these finds its rows the same way.
EXPLAIN shows IndexNestedLoop, its probe's bounds among its conditions.
Of a table of 3000 orders, a set of keys, one named twice, probes the
key once for each distinct one.

```sql
CREATE TABLE NEW_ORDER (W INTEGER, D INTEGER, O INTEGER, PRIMARY KEY (W, D, O));
INSERT INTO NEW_ORDER WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT I + 1 FROM N WHERE I < 3000) SELECT 1, 1, I FROM N;
SELECT O FROM NEW_ORDER WHERE (W, D, O) IN (VALUES (1, 1, 7), (1, 1, 9), (1, 1, 9));
EXPLAIN SELECT O FROM NEW_ORDER WHERE (W, D, O) IN (VALUES (1, 1, 7), (1, 1, 9), (1, 1, 9));
```

```
OK
OK
            O
--------------
            7
            9
(2 rows)
 PLAN
--------------------------------------------------------------
 -- bound:
 Project NEW_ORDER.O
   Filter (NEW_ORDER.W, NEW_ORDER.D, NEW_ORDER.O) = ANY (<subquery>)
     Scan ANN.NEW_ORDER
     Subquery
       Values (1, 1, 7), (1, 1, 9), (1, 1, 9)
 -- rules: semi_joins
 -- optimized:
 Project NEW_ORDER.O
   Semi join on NEW_ORDER.W = #0 AND NEW_ORDER.D = #1 AND NEW_ORDER.O = #2
     Scan ANN.NEW_ORDER
     Values (1, 1, 7), (1, 1, 9), (1, 1, 9)
 -- physical:
 Project NEW_ORDER.O
   IndexNestedLoop ANN.NEW_ORDER using ANN.NEW_ORDER.UNIQUE1 [btree]: #0 = NEW_ORDER.W AND #1 = NEW_ORDER.D AND #2 = NEW_ORDER.O  (est. rows each=1, cost=59.5)
     Distinct
       Project #0, #1, #2
         Values (1, 1, 7), (1, 1, 9), (1, 1, 9)
(18 rows)
```

A query whose select list is MIN and MAX alone, with no GROUP BY, needs
the values of the columns those functions read, not how often a join
repeats them. Once its joins are ordered, each join of a table nothing
above it reads is a semi join, which gives each row once at its first
match. Joins that would multiply the rows many times make none. EXPLAIN
shows HashSemiJoin and SemiIndexNestedLoop, and
OPTIMIZER.SEMI_REDUCTION turns this off.

SET PLANNER.JOIN_ENUMERATION = 'GREEDY' plans a session's joins in a
left-deep order instead, each next join the cheapest, as to compare the
two. EXPLAIN shows the order and the algorithm chosen
([EXPLAIN](statements/explain.md)).

## UNNEST

UNNEST in the FROM clause gives the elements of arrays as rows, a row of
each place from the first to the last of the longest array, each column
an array's element there, null past its last. WITH ORDINALITY adds a
column of the place, from 1, of type INTEGER. An array of rows gives a
column of each field. UNNEST reads the tables before it in the FROM
clause, each of their rows giving its own rows, so a table's array column
is unnested row by row; a LEFT join keeps a row whose array gives none.
UNNEST of a multiset gives a row of each element, of one multiset alone
and without WITH ORDINALITY, as a multiset's elements have no places.

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

## Lateral derived tables

A derived table written LATERAL reads the tables before it in the FROM
clause, as UNNEST does. Its query is computed for each of their rows,
which give the values of the columns it names of theirs. A LEFT join
keeps a row whose query gives none, and one of nothing before it is a
derived table as any other. It has a correlation name, and a RIGHT or
FULL join has no lateral table that reads its first operand. A lateral
set function of a key, as the oldest order of each district is, reads
one entry of the key for each row before it, and an IN of its rows
probes the key once for each.

```sql
CREATE TABLE DISTRICT_ORDERS (DISTRICT INTEGER, ID INTEGER, AMOUNT DECIMAL(6,2), PRIMARY KEY (DISTRICT, ID));
INSERT INTO DISTRICT_ORDERS VALUES (1, 10, 5.00), (1, 11, 7.50), (1, 12, 2.25), (2, 20, 9.00), (2, 21, 1.00);
CREATE TABLE SALES_DISTRICTS (ID INTEGER PRIMARY KEY);
INSERT INTO SALES_DISTRICTS VALUES (1), (2), (3);
SELECT D.ID, T.ID, T.AMOUNT FROM SALES_DISTRICTS D LEFT JOIN LATERAL (SELECT ID, AMOUNT FROM DISTRICT_ORDERS WHERE DISTRICT = D.ID ORDER BY AMOUNT DESC FETCH FIRST 2 ROWS ONLY) AS T ON TRUE ORDER BY D.ID, T.AMOUNT DESC;
SELECT D.ID, M.OLDEST FROM SALES_DISTRICTS D, LATERAL (SELECT MIN(ID) FROM DISTRICT_ORDERS WHERE DISTRICT = D.ID) AS M (OLDEST) ORDER BY D.ID;
DELETE FROM DISTRICT_ORDERS WHERE (DISTRICT, ID) IN (SELECT D.ID, M.OLDEST FROM SALES_DISTRICTS D, LATERAL (SELECT MIN(ID) FROM DISTRICT_ORDERS WHERE DISTRICT = D.ID) AS M (OLDEST));
SELECT DISTRICT, ID FROM DISTRICT_ORDERS ORDER BY DISTRICT, ID;
```

```
OK
OK
OK
OK
           ID |           ID |   AMOUNT
--------------+--------------+----------
            1 |           11 |     7.50
            1 |           10 |     5.00
            2 |           20 |     9.00
            2 |           21 |     1.00
            3 |         NULL |     NULL
(5 rows)
           ID |       OLDEST
--------------+--------------
            1 |           10
            2 |           20
            3 |         NULL
(3 rows)
OK
     DISTRICT |           ID
--------------+--------------
            1 |           11
            1 |           12
            2 |           21
(3 rows)
```

## Polymorphic table functions

TABLE ( an invocation ) in the FROM clause invokes a polymorphic table
function ([CREATE FUNCTION](statements/create-function.md)). Each table
argument is TABLE ( a table's name ), TABLE ( a query ), or an
invocation of another such function. Its correlation name, else its
table's name, names its columns in the query. A table argument of a
parameter of set semantics may be partitioned and ordered:

```
<table argument> ::=
    { TABLE ( <table name> ) | TABLE ( <query expression> ) | <routine invocation> }
    [ [ AS ] <correlation name> [ ( <column name> [ , ... ] ) ] ]
    [ PARTITION BY { <column reference> | ( <column reference> [ , ... ] ) } ]
    [ PRUNE WHEN EMPTY | KEEP WHEN EMPTY ]
    [ ORDER BY { <column reference> | ( <column reference> [ ASC | DESC ] [ NULLS FIRST | NULLS LAST ] [ , ... ] ) } ]

<descriptor argument> ::= DESCRIPTOR ( <column name> [ <data type> ] [ , ... ] )

<co-partition clause> ::= COPARTITION ( <range variable> , <range variable> [ , ... ] ) [ , ( ... ) ]...
```

The function is run once for each partition. Tables co-partitioned share
their partitions, each value either has, and the partitions of
partitioned tables otherwise cross. The result has the function's own
columns, of its correlation name, then each table argument's columns of
PASS THROUGH, or else its partitioning columns, of its own name. A
function of RETURNS ONLY PASS THROUGH has no correlation name.

```sql
CREATE TABLE SALES (REGION CHAR(4), AMOUNT INTEGER);
INSERT INTO SALES VALUES ('EAST', 10), ('EAST', 30), ('WEST', 5);
CREATE PROCEDURE TOTAL_F (IN CR VARCHAR(128), IN PT VARCHAR(128), IN OD VARCHAR(128),
                          IN CU VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5))
  CONTAINS SQL
BEGIN
  DECLARE R CHAR(4);
  DECLARE A INTEGER;
  DECLARE N, T INTEGER DEFAULT 0;
  DECLARE DONE BOOLEAN DEFAULT FALSE;
  DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE;
  L: LOOP
    FETCH NEXT FROM PTF CU INTO R, A;
    IF DONE THEN LEAVE L; END IF;
    SET N = N + 1;
    SET T = T + A;
  END LOOP L;
  SET DESCRIPTOR PTF RR VALUE 1 DATA = N;
  SET DESCRIPTOR PTF RR VALUE 2 DATA = T;
  PIPE ROW PTF RR;
END;
CREATE FUNCTION TOTALS (T TABLE WITH SET SEMANTICS) RETURNS TABLE (ROWS_SEEN INTEGER, TOTAL INTEGER)
  CONTAINS SQL FULFILL WITH PROCEDURE TOTAL_F;
SELECT S.REGION, P.ROWS_SEEN, P.TOTAL
  FROM TABLE (TOTALS (TABLE (SALES) AS S PARTITION BY REGION)) AS P ORDER BY S.REGION;
```

```
OK
OK
OK
OK
 REGION |    ROWS_SEEN |        TOTAL
--------+--------------+--------------
 EAST   |            2 |           40
 WEST   |            1 |            5
(2 rows)
```

## Row pattern recognition

MATCH_RECOGNIZE after a table in the FROM clause finds runs of its rows
that match a pattern, such as a price falling and then rising. Its rows
are split by PARTITION BY and put in order by ORDER BY, and each
partition is searched on its own.

```
table MATCH_RECOGNIZE (
    [PARTITION BY column, ...] [ORDER BY sort key, ...]
    [MEASURES expression AS name, ...]
    [ONE ROW PER MATCH | ALL ROWS PER MATCH [SHOW EMPTY MATCHES |
        OMIT EMPTY MATCHES | WITH UNMATCHED ROWS]]
    [AFTER MATCH SKIP {PAST LAST ROW | TO NEXT ROW | TO FIRST v | TO LAST v}]
    PATTERN (pattern) [SUBSET u = (v, ...), ...]
    DEFINE v AS condition, ...
) [AS name]
```

PATTERN is a regular expression over row pattern variables. A variable
matches one row its DEFINE condition is true of, and one DEFINE leaves
out matches any row. Variables written one after another match rows one
after another, `|` takes either side, PERMUTE takes its variables in any
order, `^` and `$` match a partition's start and end, and `{- ... -}`
matches rows it leaves out of the output. A quantifier `*`, `+`, `?`,
`{n}`, `{n,}`, `{,m}` or `{n,m}` repeats, the most it can unless a `?`
follows it. Of the matches a pattern has from a row, the one taken is
the first in that order of preference, left before right and more before
fewer. SUBSET names a union of variables.

In DEFINE and MEASURES, `A.PRICE` is the PRICE of the row A matched, the
last such row so far, and a bare `PRICE` that of the current row. PREV
and NEXT read a row before or after it, by default one, and FIRST and
LAST the first or last row a variable matched. A set function reads
every row of a variable, as `AVG(DOWN.PRICE)` and `COUNT(A.*)`.
CLASSIFIER() gives the variable that matched the row, and MATCH_NUMBER()
numbers the matches of a partition from 1. In MEASURES, RUNNING reads the
match as far as the current row and FINAL the whole match. RUNNING is the
default, and DEFINE takes no FINAL.

ONE ROW PER MATCH, the default, gives a row of each match of the
partition columns and the measures. ALL ROWS PER MATCH gives each row of
each match, with its own columns and the measures. A match of no rows
gives the row it was sought at, its measures of no rows, unless OMIT
EMPTY MATCHES, and WITH UNMATCHED ROWS gives every row no match covered
too, its measures null.
After a match, AFTER MATCH SKIP says where the next is sought, past the
match's last row by default. Skipping to a variable that matched no row,
or to the match's first row, raises SQLSTATE 2202K or 2202L.

Below, PRICES has a ticker's price each day D, and each match is a fall of
one or more days and then a rise.

```sql
CREATE TABLE PRICES (TICKER CHAR(1), D INTEGER, PRICE INTEGER);
INSERT INTO PRICES VALUES ('A', 1, 10), ('A', 2, 9), ('A', 3, 8), ('A', 4, 9),
    ('A', 5, 10), ('A', 6, 11), ('A', 7, 10), ('A', 8, 9), ('A', 9, 12);
SELECT * FROM PRICES MATCH_RECOGNIZE (PARTITION BY TICKER ORDER BY D
    MEASURES STRT.D AS S, LAST(DOWN.D) AS BOTTOM, LAST(UP.D) AS E,
        MATCH_NUMBER() AS N
    PATTERN (STRT DOWN+ UP+)
    DEFINE DOWN AS PRICE < PREV(PRICE), UP AS PRICE > PREV(PRICE)) AS M
    ORDER BY S;
SELECT D, PRICE, C, N FROM PRICES MATCH_RECOGNIZE (ORDER BY D
    MEASURES CLASSIFIER() AS C, RUNNING COUNT(*) AS N
    ALL ROWS PER MATCH PATTERN (STRT DOWN+ UP+)
    DEFINE DOWN AS PRICE < PREV(PRICE), UP AS PRICE > PREV(PRICE))
    ORDER BY D;
```

```
OK
OK
 TICKER |            S |       BOTTOM |            E |                    N
--------+--------------+--------------+--------------+----------------------
 A      |            1 |            3 |            6 |                    1
 A      |            7 |            8 |            9 |                    2
(2 rows)
            D |        PRICE | C                                                            |                    N
--------------+--------------+--------------------------------------------------------------+----------------------
            1 |           10 | STRT                                                         |                    1
            2 |            9 | DOWN                                                         |                    2
            3 |            8 | DOWN                                                         |                    3
            4 |            9 | UP                                                           |                    4
            5 |           10 | UP                                                           |                    5
            6 |           11 | UP                                                           |                    6
            7 |           10 | STRT                                                         |                    1
            8 |            9 | DOWN                                                         |                    2
            9 |           12 | UP                                                           |                    3
(9 rows)
```

### Row patterns in a window

A window may hold a row pattern too, of MEASURES, AFTER MATCH SKIP,
INITIAL or SEEK, PATTERN, SUBSET and DEFINE after its frame. Its frame
is ROWS BETWEEN CURRENT ROW and a bound, and its pattern has no `^` and
no `$`. Each row's frame is narrowed to the match starting at the row,
or with SEEK the first match starting within the frame, and is empty of
a row with none or that a match skipped past. Window functions over the
window read that narrowed frame, and `m OVER w` gives the measure `m` of
it.

```sql
SELECT D, E OVER W AS E, COUNT(*) OVER W AS N FROM PRICES
    WINDOW W AS (ORDER BY D MEASURES LAST(UP.D) AS E
        ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING
        AFTER MATCH SKIP PAST LAST ROW PATTERN (STRT DOWN+ UP+)
        DEFINE DOWN AS PRICE < PREV(PRICE), UP AS PRICE > PREV(PRICE))
    ORDER BY D;
```

```
            D |            E |                    N
--------------+--------------+----------------------
            1 |            6 |                    6
            2 |         NULL |                    0
            3 |         NULL |                    0
            4 |         NULL |                    0
            5 |         NULL |                    0
            6 |         NULL |                    0
            7 |            9 |                    3
            8 |         NULL |                    0
            9 |         NULL |                    0
(9 rows)
```

## The WHERE, GROUP BY and HAVING clauses

**WHERE** keeps the rows its condition is true of.

**GROUP BY** makes groups of the rows, one of each value of its columns,
the nulls one group. A query then gives a row of each group: what it
selects is a grouping column, a column functionally dependent on the
grouping columns, or of a set function, computed of the group. **HAVING** keeps the groups its condition is true of; without
GROUP BY, it makes the whole table one group, even of no rows. A
grouped query of no groups gives no row, though it selects only set
functions: GROUP BY a column of no rows makes no groups, and HAVING may
keep none.

```sql
SELECT DEPT, COUNT(*) AS N, SUM(SALARY) AS TOTAL FROM STAFF
    WHERE SALARY IS NOT NULL GROUP BY DEPT HAVING COUNT(*) > 1;
SELECT DEPT, NAME FROM STAFF GROUP BY DEPT;
```

```
 DEPT |                    N |                                    TOTAL
------+----------------------+------------------------------------------
 ENG  |                    2 |                                 15300.00
(1 row)
turborel: SQLCODE -103 (SQLSTATE 42000): outside a set function, a grouped query may only reference grouping columns and columns functionally dependent on them (found STAFF.NAME) [X3.135-1986 §5.25 SR7; ISO/IEC 9075-2:2023 §6.7 SR8] (at character 13)
```

### Functionally dependent columns

A column is functionally dependent on the grouping columns when their
values decide its value, so it has one value in each group. The rules of
the standard say which columns are, and turborel knows these:

- A table's PRIMARY KEY, or a UNIQUE constraint of columns NOT NULL or of
  NULLS NOT DISTINCT, decides each of its columns. A constraint that is
  DEFERRABLE decides none, as it may not hold until commit.
- A generated column is decided by the columns it is computed of.
- An equality AND-component of a WHERE or ON condition makes two columns
  decide each other, and a column compared with a literal has one value.
- A join keeps what each table decides, though of the table an outer join
  extends with nulls only what a column known not null decides. A
  NATURAL or USING join's common column is one of them.
- A view, a derived table and a WITH query keep what their query decides,
  and EXCEPT keeps its first operand's, INTERSECT both's, UNION nothing.

A column grouped with COLLATE decides nothing. Of grouping sets, a column
must be dependent on each set's columns. DEPT is the key of DEPT, so it
decides TITLE, and the join's condition makes it decide STAFF's DEPT too:

```sql
SELECT S.DEPT, D.TITLE, COUNT(*) AS N FROM STAFF S JOIN DEPT D ON S.DEPT = D.DEPT
    GROUP BY D.DEPT;
SELECT ID, NAME, SALARY FROM STAFF GROUP BY ID HAVING SALARY > 6000;
```

```
 DEPT | TITLE                |                    N
------+----------------------+----------------------
 ENG  | Engineering          |                    2
(1 row)
           ID | NAME                           |      SALARY
--------------+--------------------------------+-------------
            1 | Ada                            |     7200.00
            2 | Grace                          |     8100.00
(2 rows)
```

A grouping column that the other grouping columns decide adds nothing to
the grouping's work. Of `GROUP BY D.DEPT, D.TITLE` the groups are told
apart by DEPT alone, and TITLE is carried with each group as if GROUP BY
had not named it. For this turborel counts only the dependencies that
hold of values however they are told apart. One through an expression or
a generated column counts of exact numbers, truth values and datetimes
without a time zone, and an equality counts between two values of one
kind. Two character strings that differ in trailing spaces are equal and
yet differ in length, so a column computed of a string stays a grouping
column.

A grouping column that is an expression of other grouping columns is
not grouped by either. Of a derived table's `V` and `V + 1 AS NEXT`
grouped together, the groups are told apart by V alone, and NEXT is
computed of each group once the groups are made, of the group of nulls
too. This holds for an expression that gives the same value of the same
values each time and cannot raise an error, of columns that are a
table's own and of the kinds above, exact numbers, truth values and
datetimes without a time zone. A constant among the grouping columns is
computed so too. An expression that may raise, as a quotient may, stays
a grouping column and is computed of every row. ROLLUP, CUBE and
GROUPING SETS group by every column they name.

```sql
SELECT SALARY, YEARLY, COUNT(*) AS N FROM (SELECT SALARY, SALARY * 12 AS YEARLY FROM STAFF) AS T
    GROUP BY SALARY, YEARLY ORDER BY SALARY;
```

```
      SALARY |        YEARLY |                    N
-------------+---------------+----------------------
     5400.00 |      64800.00 |                    1
     7200.00 |      86400.00 |                    1
     8100.00 |      97200.00 |                    1
        NULL |          NULL |                    1
(4 rows)
```

Where such a query keeps its first rows with FETCH FIRST n ROWS ONLY,
the columns a table's key decides are read after the rows are chosen,
of the table by its key, unless a sort key, a HAVING condition or an
expression of the select list reads them.
[EXPLAIN](statements/explain.md) names the rules reduce_grouping_keys,
compute_keys_after_grouping and fetch_dependents_late where they rewrote
a plan.

### Grouping sets

GROUP BY may group the rows several ways in one query, each way a
grouping set. Each set's groups give their rows, and a column the set
leaves out is null in them.

| GROUP BY | Grouping sets |
|---|---|
| `ROLLUP (A, B)` | (A, B), (A), () |
| `CUBE (A, B)` | (A, B), (A), (B), () |
| `GROUPING SETS ((A), (B, C), ())` | (A), (B, C), () |
| `A, ROLLUP (B)` | (A, B), (A) |
| `()` | () |

The empty set () makes the whole table one group, even of no rows, so
it gives the grand total. Sets written one after another are combined,
each set of the first with each of the second. GROUPING(A) is 1 in the
rows of a set that leaves A out, else 0, which tells a total's null from
a null in the data. Of several columns its bits make a binary number,
the first column the highest. HAVING is judged of each set's groups.
GROUP BY DISTINCT keeps one of each set alike and one of each row alike.

A set function's argument sees the left-out column null too, as the
standard defines the rows, so SUM(SALARY) totals but SUM of a grouping
column is null on the rows of a set that leaves it out.

Below, the first NULL row is the staff of no department, G 0, and the
second the total of all four, G 1. The warning is of the null salary
SUM leaves out.

```sql
SELECT DEPT, COUNT(*) AS N, SUM(SALARY) AS TOTAL, GROUPING(DEPT) AS G
    FROM STAFF GROUP BY ROLLUP (DEPT) ORDER BY G, DEPT;
```

```
 DEPT |                    N |                                    TOTAL |                    G
------+----------------------+------------------------------------------+----------------------
 ENG  |                    2 |                                 15300.00 |                    0
 OPS  |                    1 |                                  5400.00 |                    0
 NULL |                    1 |                                     NULL |                    0
 NULL |                    4 |                                 20700.00 |                    1
(4 rows, with a warning: SQLSTATE 01003)
```

### Correlated subqueries

A subquery that refers to a column of the query around it is
correlated, and its value is the one it has for that row's values
[X3.135-1986 §5.21 GR3]. turborel unnests it as it optimizes the query.
The subquery is computed once, for every set of values of the outer
columns at a time, and joined with the outer rows, rather than run for
each of them. Its rows, nulls and errors are those it would have run
for each row. A scalar subquery of two rows for a row still fails with
SQLCODE -811, and COUNT of a row with no match is still 0. A subquery
of one set function correlated by equalities alone, as
`(SELECT AVG(Q) FROM L WHERE L.K = P.K)`, is grouped once by the columns
it is correlated by, where its declared types show it cannot overflow.
A function that returns one value from a subquery of its parameters is
unnested the same way, inlined where it is invoked.

EXISTS, NOT EXISTS, IN and NOT IN of a subquery that is one of the
conditions ANDed in a WHERE or HAVING clause become a semi join or an
antijoin. Each outer row is kept at its first matching row, or kept for
having none, and no row of the subquery is read twice for it. NOT IN
keeps its exact meaning with nulls: a row is kept only if no value of
the subquery equals it or may equal it. EXPLAIN shows HashSemiJoin,
HashAntiJoin and their kin, and the setting OPTIMIZER.SEMI_JOINS turns
the rewrite off. HashRightSemiJoin and HashRightAntiJoin hold the outer
rows, the fewer, and read the subquery's rows once to mark the outer
rows they match. A large table under one is then shown as a Parallel
part, read by several threads at once.

EXISTS, IN, NOT IN and the comparisons with SOME, ANY and ALL of a
correlated subquery anywhere else, under OR or NOT, in a CASE or in the
select list, become a mark join. Each outer row is given once with a
mark of whether the subquery has a matching row, true, false or
unknown, and the condition reads the mark. The subquery's rows are held
once, by the values the outer rows are matched on, and no grouping of
them is made. IN is unknown, never false, when a null among the
subquery's values or the outer row's own null could have matched, as
the standard asks. EXPLAIN shows HashMarkJoin or NestedLoopMarkJoin, and
the setting OPTIMIZER.MARK_JOINS turns the rewrite off.

A subquery stays nested if it invokes a function that is not
deterministic or that may change data, takes a sequence's next value, is
of UNIQUE or MATCH, or stands where a CASE may leave it unevaluated and
can raise an error. [EXPLAIN](statements/explain.md) shows which
subqueries were unnested, and the settings OPTIMIZER.UNNEST_SUBQUERIES
and OPTIMIZER.INLINE_FUNCTIONS turn the rewrites off
([Settings](../appendix/c-settings.md)).

A subquery that stays nested runs once for each distinct set of its
outer values. Its first runs plan it with the values as constants, so
an index can answer a comparison with them, and also with the outer
references as parameters. Once a few runs in a row make one plan both
ways, the plan with parameters is kept, and each later run fills in its
values and runs it without planning again. A subquery whose values keep
choosing other plans is planned at each run.
EXECUTOR.KEEP_SUBQUERY_PLANS OFF plans every run, as to compare the two.

```sql
SELECT NAME, (SELECT COUNT(*) FROM STAFF T WHERE T.DEPT = S.DEPT) AS PEERS FROM STAFF S
    WHERE EXISTS (SELECT * FROM DEPT WHERE DEPT.DEPT = S.DEPT) ORDER BY NAME;
```

```
 NAME                           |                PEERS
--------------------------------+----------------------
 Ada                            |                    2
 Grace                          |                    2
(2 rows)
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
 WHO                            |         MONTHLY
--------------------------------+-----------------
 Ada                            |          600.00
(1 row)
```

ORDER BY sorts by columns of the result alone, named or by position: a
column the query does not select is not a sort key.

A key is ascending unless DESC follows it. Nulls sort after every value
ascending and before every value descending. NULLS FIRST or NULLS LAST
after a key puts them first or last in either direction.

```sql
SELECT DISTINCT DEPT FROM STAFF ORDER BY DEPT NULLS FIRST;
```

```
 DEPT
------
 NULL
 ENG
 OPS
(3 rows)
```

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

## Window functions

A window function computes a value for each row out of the rows around
it, and keeps every row, where a set function makes one row of each
group. It is written in the select list as a function followed by OVER
and a window.

A window is written in parentheses. PARTITION BY divides the rows into
partitions, each of the rows alike in its columns, nulls alike and
strings by their collation. ORDER BY orders each partition, and rows
alike in its keys are **peers**. Either may be left out. Without
PARTITION BY the whole table is one partition, and without ORDER BY
every row of a partition is a peer of every other.

| Function | Value |
|---|---|
| ROW_NUMBER() | the row's place in its partition, from 1 |
| RANK() | 1 more than the rows ordered before its peers |
| DENSE_RANK() | 1 more than the peer groups ordered before its own |
| PERCENT_RANK() | (RANK − 1) / (rows − 1), 0 of a partition of one row |
| CUME_DIST() | the rows up to its last peer, divided by the rows |
| NTILE(n) | the row's tile, the partition cut into n tiles in order |
| LAG(x [, k [, d]]) | x of the row k rows before, else d |
| LEAD(x [, k [, d]]) | x of the row k rows after, else d |
| FIRST_VALUE(x) | x of the frame's first row |
| LAST_VALUE(x) | x of the frame's last row |
| NTH_VALUE(x, n) | x of the frame's nth row |
| a set function | the set function of the row's frame |

Each of the four ranks needs an ORDER BY, as do NTILE, LAG and LEAD.
ROW_NUMBER without one numbers the rows in the order they are read.
ROW_NUMBER, RANK, DENSE_RANK and NTILE are of COUNT's type, and
PERCENT_RANK and CUME_DIST are DOUBLE PRECISION. LAG, LEAD and the three
value functions are of x's type.

A set function OVER a window computes over the row's **frame**. Without
ORDER BY the frame is the whole partition. With it the frame is the rows
up to the row's last peer, so SUM is a running total. Below, each row is
ranked by salary over the table and totalled over its department.

```sql
SELECT NAME, DEPT, SALARY, RANK() OVER (ORDER BY SALARY DESC NULLS LAST) AS R,
    SUM(SALARY) OVER (PARTITION BY DEPT) AS DEPT_TOTAL FROM STAFF ORDER BY R;
```

```
 NAME                           | DEPT |      SALARY |                    R |                               DEPT_TOTAL
--------------------------------+------+-------------+----------------------+------------------------------------------
 Grace                          | ENG  |     8100.00 |                    1 |                                 15300.00
 Ada                            | ENG  |     7200.00 |                    2 |                                 15300.00
 Edsger                         | OPS  |     5400.00 |                    3 |                                  5400.00
 Barbara                        | NULL |        NULL |                    4 |                                     NULL
(4 rows, with a warning: SQLSTATE 01003)
```

### Frames

A frame clause after ORDER BY says the frame itself. ROWS counts rows
from the current row, GROUPS counts peer groups, and RANGE takes the
rows whose key is within an offset of the current row's.

```
{ROWS | RANGE | GROUPS} BETWEEN start AND end [EXCLUDE ...]
```

A bound is UNBOUNDED PRECEDING, n PRECEDING, CURRENT ROW, n FOLLOWING
or UNBOUNDED FOLLOWING. The start's kind comes no later in that list
than the end's, so `ROWS BETWEEN CURRENT ROW AND 1 PRECEDING` is
refused. A start written alone ends at CURRENT ROW. CURRENT
ROW of RANGE or GROUPS is the row's peers, not the row alone.

An offset is a literal or a parameter, never negative or null, which is
SQLSTATE 22013. For ROWS and GROUPS it is an integer. RANGE with an
offset orders by one key, a number, a datetime or an interval, and the
offset is a number or an interval of it. A date moved by months to a day
its month lacks is the month's last day. A row whose RANGE key is null
has the rows of null keys as its peers and its frame.

EXCLUDE CURRENT ROW leaves the row out of its frame, EXCLUDE GROUP its
peers and itself, EXCLUDE TIES its peers but not itself, and EXCLUDE NO
OTHERS, the default, nothing. The ranks, NTILE, LAG and LEAD take no
frame.

A WINDOW clause after HAVING names windows. `OVER W` uses one as it is,
and `OVER (W ...)` adds to it an ORDER BY it has not, or a frame. Below,
NEAR is the mean of the row and those either side of it.

```sql
SELECT NAME, SALARY, ROW_NUMBER() OVER W AS N, SUM(SALARY) OVER W AS RUNNING,
    AVG(SALARY) OVER (W ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) AS NEAR
    FROM STAFF WINDOW W AS (ORDER BY ID) ORDER BY N;
SELECT NAME, SALARY, COUNT(*) OVER (ORDER BY SALARY
    RANGE BETWEEN 1000 PRECEDING AND 1000 FOLLOWING) AS NEARBY FROM STAFF ORDER BY NAME;
```

```
 NAME                           |      SALARY |                    N |                                  RUNNING |                                     NEAR
--------------------------------+-------------+----------------------+------------------------------------------+------------------------------------------
 Ada                            |     7200.00 |                    1 |                                  7200.00 |                                7650.0000
 Grace                          |     8100.00 |                    2 |                                 15300.00 |                                6900.0000
 Edsger                         |     5400.00 |                    3 |                                 20700.00 |                                6750.0000
 Barbara                        |        NULL |                    4 |                                 20700.00 |                                5400.0000
(4 rows, with a warning: SQLSTATE 01003)
 NAME                           |      SALARY |               NEARBY
--------------------------------+-------------+----------------------
 Ada                            |     7200.00 |                    2
 Barbara                        |        NULL |                    1
 Edsger                         |     5400.00 |                    1
 Grace                          |     8100.00 |                    2
(4 rows)
```

Below, G counts the rows of a row's peer group and the next, less the
row itself.

```sql
SELECT NAME, DEPT, DENSE_RANK() OVER (ORDER BY DEPT) AS D,
    PERCENT_RANK() OVER (ORDER BY DEPT) AS P, CUME_DIST() OVER (ORDER BY DEPT) AS C,
    COUNT(*) OVER (ORDER BY DEPT GROUPS BETWEEN CURRENT ROW AND 1 FOLLOWING
        EXCLUDE CURRENT ROW) AS G FROM STAFF ORDER BY D, NAME;
```

```
 NAME                           | DEPT |                    D |                        P |                        C |                    G
--------------------------------+------+----------------------+--------------------------+--------------------------+----------------------
 Ada                            | ENG  |                    1 |                      0E0 |                     5E-1 |                    2
 Grace                          | ENG  |                    1 |                      0E0 |                     5E-1 |                    2
 Edsger                         | OPS  |                    2 |     6.666666666666666E-1 |                   7.5E-1 |                    1
 Barbara                        | NULL |                    3 |                      1E0 |                      1E0 |                    0
(4 rows)
```

### Offsets and values

NTILE cuts each partition into n tiles of rows in its order, the first
tiles one row longer where the rows do not divide evenly. With more
tiles than rows, each row is a tile of its own. n is a literal or a
parameter. A null n makes every tile null, and an n below 1 is SQLSTATE
22014.

LAG and LEAD take x of the row k rows before or after the current one in
its partition, k being 1 if left out. k is an unsigned integer literal,
and 0 takes the current row's own x. Past either end of the partition
they take the default d, computed for the current row, or null if d is
left out. d is cast to x's type, so it is of a type each assignable to
the other, such as a number of a number.

FIRST_VALUE, LAST_VALUE and NTH_VALUE take x of a row of the frame, so a
frame clause and EXCLUDE apply to them as to a set function. Of the
frame without one, LAST_VALUE takes the current row's last peer. NTH_VALUE
counts from the frame's first row, or from its last after FROM LAST. Its
n is a literal or a parameter, and a frame of fewer rows gives null. A
null n gives null, and an n below 1 is SQLSTATE 22016.

IGNORE NULLS after the parentheses counts only the rows whose x is not
null, for LAG, LEAD and the three value functions. RESPECT NULLS, the
default, counts every row. A window function may be in the argument of
any of these, so `LAG(ROW_NUMBER() OVER W) OVER W` is the row number
before.

```sql
SELECT ID, NAME, SALARY, NTILE(2) OVER W AS HALF, LAG(SALARY) OVER W AS BEFORE,
    LEAD(SALARY, 1, 0) OVER W AS AFTER,
    LAG(SALARY) IGNORE NULLS OVER (ORDER BY ID DESC) AS NEXT_PAID
    FROM STAFF WINDOW W AS (ORDER BY ID) ORDER BY ID;
SELECT ID, NAME, DEPT, FIRST_VALUE(NAME) OVER D AS FIRST_IN,
    LAST_VALUE(NAME) OVER D AS LAST_SO_FAR,
    NTH_VALUE(NAME, 2) OVER (D ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) AS RUNNER_UP
    FROM STAFF WINDOW D AS (PARTITION BY DEPT ORDER BY SALARY DESC) ORDER BY ID;
SELECT NAME, LAG(SALARY) OVER () FROM STAFF;
```

```
           ID | NAME                           |      SALARY |                 HALF |      BEFORE |       AFTER |   NEXT_PAID
--------------+--------------------------------+-------------+----------------------+-------------+-------------+-------------
            1 | Ada                            |     7200.00 |                    1 |        NULL |     8100.00 |     8100.00
            2 | Grace                          |     8100.00 |                    1 |     7200.00 |     5400.00 |     5400.00
            3 | Edsger                         |     5400.00 |                    2 |     8100.00 |        NULL |        NULL
            4 | Barbara                        |        NULL |                    2 |     5400.00 |        0.00 |        NULL
(4 rows)
           ID | NAME                           | DEPT | FIRST_IN                       | LAST_SO_FAR                    | RUNNER_UP
--------------+--------------------------------+------+--------------------------------+--------------------------------+--------------------------------
            1 | Ada                            | ENG  | Grace                          | Ada                            | Ada
            2 | Grace                          | ENG  | Grace                          | Grace                          | Ada
            3 | Edsger                         | OPS  | Edsger                         | Edsger                         | NULL
            4 | Barbara                        | NULL | Barbara                        | Barbara                        | NULL
(4 rows)
turborel: SQLCODE -103 (SQLSTATE 42000): the window of LEAD or LAG has an ordering [ISO/IEC 9075-2:2023 §9.23 SR5a] (at character 13)
```

### Nested window functions

A set function OVER a window may read, in its argument, where a row is
in its partition and the values of other rows. Each of these nested
window functions names a row by a marker.

| Marker | Row |
|---|---|
| BEGIN_PARTITION | the partition's first row |
| END_PARTITION | the partition's last row |
| CURRENT_ROW | the row the window function is computed for |
| FRAME_ROW | the row of the frame the set function is taking |
| BEGIN_FRAME | the frame's first row, after EXCLUDE |
| END_FRAME | the frame's last row, after EXCLUDE |

`ROW_NUMBER(marker)` is that row's place in the partition, from 1, of
COUNT's type. `VALUE_OF(x AT marker)` is x read at that row, and `AT
marker + k` or `AT marker - k` moves k rows on or back, k an integer
literal or a parameter. Past either end of the partition, or of a null
k, VALUE_OF takes the default after a comma, or null if it is left out.
x is read at the row named, but the default and the rest of the argument
at the frame's row. VALUE_OF is of the type x and the default have in
common, both cast to it. One VALUE_OF may hold another in its x.

Below, WEIGHTED counts the row's salary twice and the row before once,
AFTER is the next row's salary, and LAST_ID names the partition's last
row.

```sql
SELECT ID, NAME, SALARY,
    SUM(SALARY * (2 - ROW_NUMBER(CURRENT_ROW) + ROW_NUMBER(FRAME_ROW))) OVER (ORDER BY ID
        ROWS BETWEEN 1 PRECEDING AND CURRENT ROW) AS WEIGHTED,
    MAX(VALUE_OF(SALARY AT CURRENT_ROW + 1, 0)) OVER (ORDER BY ID ROWS CURRENT ROW) AS AFTER,
    MIN(VALUE_OF(NAME AT END_PARTITION)) OVER (ORDER BY ID) AS LAST_ID
    FROM STAFF ORDER BY ID;
SELECT ROW_NUMBER(CURRENT_ROW) FROM STAFF;
```

```
           ID | NAME                           |      SALARY |                                 WEIGHTED |       AFTER | LAST_ID
--------------+--------------------------------+-------------+------------------------------------------+-------------+--------------------------------
            1 | Ada                            |     7200.00 |                                 14400.00 |     8100.00 | Barbara
            2 | Grace                          |     8100.00 |                                 23400.00 |     5400.00 | Barbara
            3 | Edsger                         |     5400.00 |                                 18900.00 |        NULL | Barbara
            4 | Barbara                        |        NULL |                                  5400.00 |        0.00 | Barbara
(4 rows, with a warning: SQLSTATE 01003)
turborel: SQLCODE -103 (SQLSTATE 42000): a nested window function is only in the argument of a set function a window function contains [ISO/IEC 9075-2:2023 §6.11 SR1] (at character 7)
```

A nested window function is only in the argument of a set function OVER
a window, and not in a set function of the groups within it. An empty
frame reads no argument, so it gives what a set function of no rows
gives. An argument that reads none of CURRENT_ROW, BEGIN_FRAME and
END_FRAME is computed once for each row. Any other is computed again for
each row of each frame, so it costs the rows times the frame's length.

### Where windows are computed

Windows are computed after WHERE, GROUP BY and HAVING, and before
DISTINCT and ORDER BY. In a grouped query they are over the groups, so a
window function's arguments and keys may be the group's set functions.
A window function is only in a select list, so a query that filters on
one filters a derived table or a view of it.

COUNT(DISTINCT x) is taken over any window, each value of a frame's
rows counted once. Over a frame that moves, the count of each value
among the last frame's rows is kept, spilled past the work area, and
only the rows a frame gains or loses are counted, so a partition costs
about its rows. LISTAGG and the other set functions of DISTINCT are
taken only over a window of no ORDER BY or frame, else SQLCODE -103.

```sql
SELECT DEPT, SUM(SALARY) AS TOTAL, RANK() OVER (ORDER BY SUM(SALARY) DESC) AS R
    FROM STAFF GROUP BY DEPT ORDER BY R;
SELECT NAME FROM STAFF WHERE RANK() OVER (ORDER BY SALARY) = 1;
SELECT ID, COUNT(DISTINCT DEPT) OVER (ORDER BY ID ROWS 1 PRECEDING) AS DEPTS
    FROM STAFF ORDER BY ID;
SELECT SUM(DISTINCT SALARY) OVER (ORDER BY ID) FROM STAFF;
```

```
 DEPT |                                    TOTAL |                    R
------+------------------------------------------+----------------------
 NULL |                                     NULL |                    1
 ENG  |                                 15300.00 |                    2
 OPS  |                                  5400.00 |                    3
(3 rows, with a warning: SQLSTATE 01003)
turborel: SQLCODE -103 (SQLSTATE 42000): a window function is only in the select list of a query specification [ISO/IEC 9075-2:2023 §6.10 SR2b] (at character 29)
           ID |                DEPTS
--------------+----------------------
            1 |                    1
            2 |                    1
            3 |                    2
            4 |                    1
(4 rows, with a warning: SQLSTATE 01003)
turborel: SQLCODE -103 (SQLSTATE 42000): SUM(DISTINCT ...) is over a window of no ordering and no frame [ISO/IEC 9075-2:2023 §9.23 SR8d ii] (at character 7)
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
turborel: SQLCODE -103 (SQLSTATE 42000): character strings and numbers are brought together [ISO/IEC 9075:1992 §7.10 SR12b ii, §9.3 SR2, SR3a] (at character 21)
```

## ORDER BY

ORDER BY sorts the rows a query gives by each sort key in turn. A key is
a position of a column of the result, from 1, or any value expression
that reads a column. Of a query specification, a key may read the columns
its select list leaves out, and they are taken away after the sort. A key
alike an item of the select list sorts by that item's column, so it is
computed once. In a key, a name of a result column is that column, so
`N` below is `NAME`.

```sql
SELECT NAME FROM STAFF ORDER BY SALARY DESC NULLS LAST;
SELECT NAME AS N FROM STAFF ORDER BY CHAR_LENGTH(N) DESC, ID;
SELECT ID, SALARY - 1000 * ID AS SCORE FROM STAFF ORDER BY SALARY - 1000 * ID NULLS FIRST;
```

```
 NAME
--------------------------------
 Grace
 Ada
 Edsger
 Barbara
(4 rows)
 N
--------------------------------
 Barbara
 Edsger
 Grace
 Ada
(4 rows)
           ID |               SCORE
--------------+---------------------
            4 |                NULL
            3 |             2400.00
            2 |             6100.00
            1 |             6200.00
(4 rows)
```

A window function is a key too, of the rows before the sort. A key holds
no subquery, set function or NEXT VALUE FOR, so a set function to sort by
is named in the select list. A query that is DISTINCT, or of a set
function in its select list, HAVING or window clause, sorts by columns of
its result alone, since a column it leaves out may have several values in
one of its rows.

A grouped query sorts by its groups. A key may read a grouping column the
select list leaves out, or a column [functionally
dependent](#functionally-dependent-columns) on the grouping columns, which
has one value in each group. A key that reads any other column is SQLCODE
-103.

```sql
SELECT NAME FROM STAFF ORDER BY ROW_NUMBER() OVER (ORDER BY SALARY DESC NULLS LAST);
SELECT DEPT, MAX(SALARY) AS TOP FROM STAFF GROUP BY DEPT ORDER BY TOP DESC NULLS LAST;
SELECT DEPT FROM DEPT GROUP BY DEPT ORDER BY TITLE DESC;
SELECT DEPT FROM STAFF GROUP BY DEPT ORDER BY MAX(SALARY);
SELECT DISTINCT DEPT FROM STAFF ORDER BY ID;
SELECT DEPT FROM STAFF GROUP BY DEPT ORDER BY ID;
```

```
 NAME
--------------------------------
 Grace
 Ada
 Edsger
 Barbara
(4 rows)
 DEPT |         TOP
------+-------------
 ENG  |     8100.00
 OPS  |     5400.00
 NULL |        NULL
(3 rows, with a warning: SQLSTATE 01003)
 DEPT
------
 HR
 ENG
(2 rows)
turborel: SQLCODE -103 (SQLSTATE 42000): a sort key contains no set function; name it in the select list, and sort by that column [ISO/IEC 9075-2:2023 §7.17 SR26b] (at character 46)
turborel: SQLCODE -103 (SQLSTATE 42000): SELECT DISTINCT is ordered by columns of its result alone [ISO/IEC 9075-2:2023 §7.17 SR26 d i 9 B II] (at character 41)
turborel: SQLCODE -103 (SQLSTATE 42000): a sort key of a grouped query that its select list leaves out may only reference grouping columns and columns functionally dependent on them (found STAFF.ID) [ISO/IEC 9075-2:2023 §7.17 SR26 d i 9 B I] (at character 46)
```

The keys of a set operation, a VALUES list or any other query that is
not a query specification read the columns of its result alone.

```sql
SELECT DEPT FROM STAFF UNION SELECT DEPT FROM DEPT ORDER BY CASE DEPT WHEN 'HR' THEN 0 ELSE 1 END, DEPT;
SELECT DEPT FROM STAFF UNION SELECT DEPT FROM DEPT ORDER BY TITLE;
```

```
 DEPT
------
 HR
 ENG
 OPS
 NULL
(4 rows)
turborel: SQLCODE -103 (SQLSTATE 42000): the result has no column TITLE [ISO/IEC 9075:1992 §13.1 SR10a] (at character 60)
```

### OFFSET and FETCH FIRST

After ORDER BY, `OFFSET n ROWS` passes over the first n rows and `FETCH
FIRST n ROWS ONLY` keeps at most n of those after. ROW and ROWS are one,
as are FIRST and NEXT, and FETCH FIRST ROW ONLY keeps one row. Either
clause may be written without ORDER BY, which leaves which rows they
keep to the order the rows come in.

```sql
SELECT NAME FROM STAFF ORDER BY ID OFFSET 1 ROW FETCH FIRST 2 ROWS ONLY;
SELECT NAME FROM STAFF ORDER BY SALARY DESC NULLS LAST FETCH NEXT ROW ONLY;
```

```
 NAME
--------------------------------
 Grace
 Edsger
(2 rows)
 NAME
--------------------------------
 Grace
(1 row)
```

The three clauses end any query expression, not the statement alone. A
subquery, a derived table, a view's query, a WITH element's query and an
INSERT's query may each be sorted and limited, and so may a query in
parentheses, before a set operation takes its rows.

```sql
SELECT DEPT, (SELECT NAME FROM STAFF S WHERE S.DEPT = D.DEPT ORDER BY SALARY DESC FETCH FIRST ROW ONLY) AS TOP FROM DEPT D ORDER BY DEPT;
(SELECT NAME FROM STAFF ORDER BY ID FETCH FIRST ROW ONLY) UNION ALL (SELECT NAME FROM STAFF ORDER BY ID DESC FETCH FIRST ROW ONLY);
```

```
 DEPT | TOP
------+--------------------------------
 ENG  | Grace
 HR   | NULL
(2 rows)
 NAME
--------------------------------
 Ada
 Barbara
(2 rows)
```

`FETCH FIRST n PERCENT ROWS ONLY` keeps n percent of the rows, counted
before OFFSET passes over any, rounded up: CEILING(n × rows / 100.0E0).
`WITH TIES` in place of ONLY keeps, after the last row its count keeps,
the rows that sort as its peers, equal by every key, nulls equal to each
other. WITH TIES needs an ORDER BY.

```sql
SELECT NAME FROM STAFF ORDER BY ID FETCH FIRST 50 PERCENT ROWS ONLY;
SELECT NAME, DEPT FROM STAFF ORDER BY DEPT FETCH FIRST ROW WITH TIES;
```

```
 NAME
--------------------------------
 Ada
 Grace
(2 rows)
 NAME                           | DEPT
--------------------------------+------
 Ada                            | ENG
 Grace                          | ENG
(2 rows)
```

A count is a literal, a parameter, or a `?` of a prepared statement,
read once as the rows are first asked for. It is a whole number, else
SQLCODE -103, and a percentage any number. An offset below 0 is SQLSTATE
2201X, and a count of FETCH FIRST below 1 is 2201W, as is a percentage
whose count is, of a table of no rows too. A null count raises the same.
A view or a cursor of OFFSET or FETCH FIRST is not updatable, though one
of ORDER BY alone is. A recursive query has no FETCH FIRST, nor does a
query in one that reads it.

A sort of which FETCH FIRST keeps only the first rows holds only those
as it reads, with OFFSET's and the peers of WITH TIES, so ORDER BY with
FETCH FIRST 10 ROWS ONLY over a large table needs little memory. A table
of the column store read in its COLUMNSTORE.ORDER order sorts no more
than those first rows of the parts of it not yet sorted. No row past
those kept is read, unless computing one can raise an exception, as a
division by zero does. Then every row is read, and the exception is
raised as it would be if every row were kept.

An expression of the select list that cannot raise an exception and
gives the same value of the same values each time is computed of the
rows kept alone, after the sort, where the sort is by columns of the
select list. The rows and their order are the same.
[EXPLAIN](statements/explain.md) names the rule
compute_after_fetch_first where it rewrote a plan.

```sql
SELECT NAME, SALARY * 12 AS YEARLY FROM STAFF ORDER BY NAME FETCH FIRST 2 ROWS ONLY;
```

```
 NAME                           |        YEARLY
--------------------------------+---------------
 Ada                            |      86400.00
 Barbara                        |          NULL
(2 rows)
```

Over GROUP BY with no ORDER BY, the clauses keep some of the groups, and
which is left to the order the groups come in. Each row kept is a whole
group, its set functions of every row of it. The grouping then makes
only as many groups as OFFSET and FETCH FIRST count together, the first
it meets, and passes over every row of another group, so a few groups
of a large table need little memory. It makes every group as before
where a row passed over could have raised an exception or a warning, as
a SUM that may pass its range or a set function of a column that may
hold a null does, under HAVING, of grouping sets, and of a DISTINCT set
function.

```sql
SELECT DEPT, COUNT(*) FROM STAFF GROUP BY DEPT FETCH FIRST 1 ROW ONLY;
```

```
 DEPT |
------+----------------------
 ENG  |                    2
(1 row)
```

```sql
SELECT NAME FROM STAFF FETCH FIRST 0 ROWS ONLY;
SELECT NAME FROM STAFF OFFSET 1.5 ROWS;
SELECT NAME FROM STAFF FETCH FIRST 2 ROWS WITH TIES;
```

```
turborel: SQLCODE -301 (SQLSTATE 2201W): the fetch first row count is 0, less than 1 [ISO/IEC 9075-2:2023 §7.17 GR7c]
turborel: SQLCODE -103 (SQLSTATE 42000): the offset row count is an exact number of scale 0, not NUMERIC(2,1) [ISO/IEC 9075-2:2023 §7.17 SR27] (at character 30)
turborel: SQLCODE -103 (SQLSTATE 42000): FETCH FIRST WITH TIES is of a query expression with an ORDER BY [ISO/IEC 9075-2:2023 §7.17 SR30] (at character 23)
```

### FETCH APPROX and FETCH EXACT *(extension)*

`FETCH APPROX FIRST n ROWS ONLY` asks for the n rows nearest by the
distance ORDER BY sorts on, and lets an approximate index find them. It
may miss a row an exact search would keep, but never returns one the
WHERE clause does not, and raises any exception an exact search would.
APPROXIMATE is the same word. `FETCH EXACT` always reads exactly, and
so does a FETCH of neither, unless the setting VECTOR.APPROX_DEFAULT is
ON.

```
<fetch first clause> ::=
    FETCH [ APPROX | APPROXIMATE | EXACT ] { FIRST | NEXT }
    [ <fetch first quantity> ] { ROW | ROWS } { ONLY | WITH TIES }
    [ <target accuracy clause> ] [ <vector hints> ]

<target accuracy clause> ::=
      WITH TARGET ACCURACY <unsigned integer>
    | WITH TARGET ACCURACY PARAMETERS
      ( <search parameter> [ { , <search parameter> }... ] )

<search parameter> ::=
      EFSEARCH <unsigned integer>
    | NEIGHBOR PARTITION PROBES <unsigned integer>
    | SEARCH_LIST <unsigned integer>
    | BEAM_WIDTH <unsigned integer>

<vector hints> ::= HINT ( <vector hint> [ { , <vector hint> }... ] )

<vector hint> ::=
      VECTOR_INDEX ( <table name> <index name> )
    | NO_VECTOR_INDEX ( <table name> )
    | VECTOR_PREFILTER | VECTOR_INFILTER | VECTOR_POSTFILTER
    | VECTOR_WAIT_FRESH
```

APPROX is a request, not a guarantee. An index is read only when ORDER
BY is one distance of an indexed column against a constant, a parameter
or an outer reference, and the index serves the distance's metric. The
distance is VECTOR_DISTANCE, its metric named before or after its
vectors, or a shorthand such as COSINE_DISTANCE or L2_DISTANCE. An index
serves its own metric, and EUCLIDEAN and EUCLIDEAN_SQUARED serve each
other. An index whose column is declared NORMALIZED serves any of
COSINE, DOT, EUCLIDEAN and EUCLIDEAN_SQUARED, since on vectors of norm 1
they order rows alike
([CREATE VECTOR INDEX](statements/create-vector-index.md)). A `?` in a
distance takes the type of the column it is measured against. With no
such index the query runs exactly, as here:

```sql
SELECT NAME, SALARY FROM STAFF WHERE SALARY IS NOT NULL
ORDER BY (SALARY - 7000) * (SALARY - 7000) FETCH APPROX FIRST 2 ROWS ONLY WITH TARGET ACCURACY 90;
```

```
 NAME                           |      SALARY
--------------------------------+-------------
 Ada                            |     7200.00
 Grace                          |     8100.00
(2 rows)
```

A target accuracy is a percentage from 1 to 100, and overrides the
index's own TARGET_ACCURACY. A query of neither aims at the index's,
unless the index's own search list, EF_SEARCH or PROBES is set, which it
then searches by. PARAMETERS sets the search instead: EFSEARCH and SEARCH_LIST
the candidates a graph keeps, BEAM_WIDTH the nodes it reads at once, and
NEIGHBOR PARTITION PROBES the partitions it reads. The settings
VECTOR.SEARCH_LIST and VECTOR.BEAM_WIDTH hold when a query names none.

Each row an index finds is read, checked against the whole WHERE
clause, and its distance computed again exactly. Rows of equal distance
come in the order they were inserted. When too few rows are left, the
index is asked again more widely, up to four times, and then the table
is read exactly, unless VECTOR.APPROX_FALLBACK is OFF, when the query
fails with SQLSTATE 0A000 (SQLCODE -900). A condition that the distance
is below a constant tells the index how far to search.

How the WHERE clause is applied follows the share of rows it keeps.
Under 1%, the rows are found first and their distances computed
exactly. Up to half, the index skips rows that fail it as it searches.
Above half, the index finds more rows than asked and the clause filters
them. VECTOR_PREFILTER, VECTOR_INFILTER and VECTOR_POSTFILTER choose one
of these, VECTOR_INDEX names the index of a table, and NO_VECTOR_INDEX
keeps a table from its approximate indexes. VECTOR_WAIT_FRESH changes
nothing, as each index is changed with its table. EXPLAIN shows which
was chosen ([EXPLAIN](statements/explain.md)).

The table function VECTOR_SEARCH writes such a query in a FROM clause.
`VECTOR_SEARCH(TABLE = t AS a, COLUMN = c, SIMILAR_TO = q, METRIC =
'cosine', TOP_N = n) AS s` is the table of the n rows of `t` nearest
`q` by `VECTOR_DISTANCE(c, q, 'cosine')`, found with FETCH APPROX. Each
row has the columns of `t` and DISTANCE, under the correlation name `s`,
or `a` if none is written. METRIC may be left out, and the arguments
may come in any order.

APPROX is of a count of rows ONLY, so neither WITH TIES nor PERCENT, and
a target accuracy is of APPROX alone. Either is SQLCODE -103 (SQLSTATE
42000).

```sql
SELECT NAME FROM STAFF ORDER BY SALARY FETCH APPROX FIRST 2 ROWS WITH TIES;
SELECT NAME FROM STAFF ORDER BY SALARY FETCH FIRST 2 ROWS ONLY WITH TARGET ACCURACY 90;
```

```
turborel: SQLCODE -103 (SQLSTATE 42000): FETCH APPROX is of a count of rows ONLY: not WITH TIES, nor PERCENT (at character 39)
turborel: SQLCODE -103 (SQLSTATE 42000): a target accuracy is of FETCH APPROX (at character 63)
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
