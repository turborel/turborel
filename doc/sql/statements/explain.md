# EXPLAIN

[Statements](README.md)

## NAME

EXPLAIN: show how a query or a data change is planned, or run it and count what each step did *(extension)*

## SYNOPSIS

```
<explain statement> ::=
    EXPLAIN [ ANALYZE ] <cursor specification>
  | EXPLAIN [ ANALYZE ] <data change statement>

<data change statement> ::=
    <insert statement>
  | <update statement: searched>
  | <delete statement: searched>
  | <update statement: positioned>
  | <delete statement: positioned>
  | <merge statement>
```

## DESCRIPTION

EXPLAIN shows how turborel would run a query, without running it. Its
rows are the lines of the plan, in one column, PLAN, each a line of text.
They show four stages in turn, each headed by a comment line. The plan as
bound comes first, then the rewrite rules that changed it, then the plan
they left, and last the physical plan that runs, with the indexes it
reads and how it joins.

The joins of three or more tables are ordered by enumeration, each
join's method chosen with the order, unless PLANNER.JOIN_ENUMERATION is
GREEDY ([Settings](../../appendix/c-settings.md)). The top join of a
plan so made notes the order taken, as nested pairs of its tables, and
the algorithm that found it, DPccp or DPhyp of a query small enough to
search whole, linearized DP or GOO-DP of a larger one. A plan that keeps
the FROM clause's order, as one estimated no cheaper by
PLANNER.REORDER_GAIN does, has no note. Two tables equalities join only
through a third, as `A.K = C.K AND B.K = C.K` do, may be joined first,
by the key those imply, `A.K = B.K`, never by a product. A merge join shows as
`MergeJoin`, or `LeftMergeJoin` of a LEFT join, with its key. A range
join shows as `RangeJoin`, or `LeftRangeJoin`, with its condition and
what it searches by: the value its right operand is sorted on, the
equalities that group those rows if any, and the one more comparison
pruned by if any ([Query Expressions](../06-queries.md#how-joins-are-planned-extension)).

A part of the physical plan run on several threads at once is headed
`Parallel` with its degree, the most threads it takes, the statement's
own among them; the planner runs a part so only if its tables hold
enough rows ([Parallel queries](../../operator/04-settings.md#parallel-queries)).

A query that only counts a table's rows, `SELECT COUNT(*) FROM T` with
no WHERE clause or grouping, may be answered by the table's storage
without reading each row. The row store sums the rows of the blocks of
rows every transaction sees alike, and reads only the others. The column
store adds the rows of its frozen groups as its directory records them,
and reads only the rows not frozen and those changed or deleted since.
Either way the count is the one a scan makes at any isolation level. The
plan then ends with a line `-- counted by storage:` naming the table. A
COUNT of a column declared NOT NULL, or of a literal that is not null,
counts rows as COUNT(*) does, and is answered the same way.

A query of only MIN, MAX and COUNT(*) of a table of the column store,
with no WHERE clause or grouping, takes each column's least and greatest
values of each frozen group from the group's summary, held in memory,
and reads only the groups changed since they were frozen and the rows
not yet frozen. MIN and MAX are taken so of numbers of exact type and of
dates and times. The plan then ends with a line `-- summarized by
storage:` naming the table. A query with EXECUTOR.BATCHES OFF reads
every row and has neither line.

EXPLAIN is direct SQL, as a query is. The `turborel` client shows its
rows as it shows a query's. A program reaches the same text by the
library's `Session::explain` ([The Embedded Library](../../client/03-embedding.md#reading-rows)),
which also takes values for a query's parameters, as direct SQL has
none.

EXPLAIN is not a key word. A table may be named EXPLAIN, and EXPLAIN is
read as this statement only before SELECT, VALUES, TABLE, WITH or a
parenthesis, or before INSERT, UPDATE, DELETE, or MERGE followed by INTO
or WITH.

### EXPLAIN of a data change

EXPLAIN also takes INSERT, UPDATE, DELETE and MERGE, and shows what the
statement would do without running it. It reads no row, locks no row or
table, writes nothing and fires no trigger, so the database's files
stay byte for byte as they were. The statement is bound as preparing it
binds it, and its rows are planned as running it would plan them.

The text is the one EXPLAIN ANALYZE shows of the statement, without
what a run counts. The first line after `-- physical:` names the
statement and its table. Below it is the plan of the rows the statement
would read: an INSERT's query, the rows an UPDATE or DELETE changes,
with the probe of an index that finds them, or the rows MERGE joins. An
INSERT of a row of values that reads no table has no plan, as its
values are evaluated. A positioned UPDATE or DELETE shows its cursor's
row, `CurrentRow of cursor C`, its cursor declared, open or not. Direct
SQL declares no cursor, so a positioned one is shown through the
library or a module.

Lines after the plan say what the change would do beside:

- `-- maintains:` names the indexes whose entries it would write. An
  INSERT or a DELETE writes every index of the table. An UPDATE writes
  those whose keys read a column it sets, and every index where a BEFORE
  row trigger may set any column.
- `-- cascades:` names each referential action that would write the rows
  referencing the table: ON DELETE or ON UPDATE CASCADE, SET NULL or SET
  DEFAULT, the table it writes and the constraint. The tables they write
  are followed in turn.
- `-- trigger:` names each trigger the change, or an action it cascades
  to, would fire, with its time, event, level and table. A change of a
  view that an INSTEAD OF trigger takes in place shows that trigger
  alone.
- `-- point path:` says whether a path past the executor's operators
  would run it. A searched UPDATE or DELETE that finds its row by a
  whole unique key runs as one read of that index when it is prepared
  or a procedure's (EXECUTOR.POINT_OPERATIONS). Of a table no trigger or
  referential action reaches, a procedure's body compiled writes the row
  straight (EXECUTOR.COMPILED_PROCEDURES), and so an INSERT's rows. An
  INSERT of values says they are evaluated. Otherwise it says `none`.

A program reaches the same text by `Session::explain`, which takes
values for the parameters a statement of a module or a procedure names.

### EXPLAIN ANALYZE

EXPLAIN ANALYZE runs the query, reads all its rows and drops them. It
shows the physical plan, and each operator's line is followed by what
the operator did. An operator whose line has no estimate gets one first,
as `(est. rows=E)`. Then comes `(actual rows=N)`, the rows it gave.
`loops=L` is added when it was opened more than once, as the inner input
of a nested loop is. An index nested loop adds `probes=P`, one probe for
each row of its outer input. An operator that never ran shows
`(never run)`, as does a scan whose storage counted its table. A last
line gives the query's rows.

An operator adds what it did beside its rows, each only when it did
any. One run by batches adds `batches=B, mean rows=M`, the batches it
gave and their rows on average. One that held more than its work area
(MEMORY.WORK_BYTES) adds `spilled=N bytes, partitions=P, passes=Q`. The
bytes are those it wrote to its spill area. The partitions are the runs
and tables it wrote them to. The passes are the times it read back what
it wrote, once for partitions written then read, once more for each
level of a sort's merge. A read of an index adds `entries=E`, the
entries it took, `fetched=F`, the rows it read from the table by the
entries' row ids,
and `rechecked=R`, the rows whose condition it judged again because the
index could not decide it of the entry. An operator that read the
leaves of a B-tree adds `leaf pages=L`, an index's leaves or those of
the tree its table of the row store is kept in. A grouping under another
that computes DISTINCT set functions beside plain ones adds `side groups=G`
where it keeps the plain functions by the outer grouping's columns alone:
the groups the first of its rows made by those columns. An operator of a part run on
several threads notes what its workers did.

Each operator's time, its inputs' among it, is added as `time=X ms` when
the database's clock moved while the query ran. A clock that stands
still, as tests give, shows no time, and so do the examples below. An
operator of a part run on several threads counts its rows over all the
threads. Its pipeline's steps have no time of their own.

A sort that asks for its first rows takes them from a grouping run on
several threads without reading every group. The grouping then shows the
groups it made, and a projection between the two shows the rows the
threads kept for the sort.

The query runs in the session's transaction and takes the locks a cursor
of it would take, so it changes nothing a query would not. A query that
raises an exception makes EXPLAIN ANALYZE raise it. A program reaches
the same text by `Session::explain_analyze`, and each operator's line,
estimate, rows, loops and time by `Session::analyzed`. A program may
also give the planner its own row counts in place of the estimates, with
`Session::with_cardinalities`, to test its plans ([The Embedded Library](../../client/03-embedding.md#reading-rows)).

### EXPLAIN ANALYZE of a data change

EXPLAIN ANALYZE also takes INSERT, UPDATE, DELETE and MERGE. The
statement runs for real, in the session's transaction, as it would run
alone. Its changes stay until the transaction ends, so a ROLLBACK
undoes them. The first line after `-- physical:` names the statement
and its table with `actual rows changed=N`. Below it is the plan of the
rows the statement read, counted as a query's plan is: an INSERT's
query, the rows an UPDATE or DELETE changes, or the rows MERGE joins. A
statement that reads no rows by a plan, as an INSERT of VALUES, shows
the first line alone. The last line gives the rows changed. A statement
that raises an exception makes EXPLAIN ANALYZE raise it, and changes
nothing. The library's `Session::analyzed` gives the statement as its
first operator, its rows those it changed.

ANALYZE is not a key word either. It is read as this option only right
after EXPLAIN and before a query's or a data change's first token. Before a parenthesis it
is read so only when the next token begins a query too. A table named
ANALYZE is read as before.

## RULES

1. The query is one a cursor could be declared for, FOR UPDATE aside, as
   for a query run directly.
2. It is bound as a query run then would be, so it refers only to tables
   and columns that exist.
3. EXPLAIN and EXPLAIN ANALYZE take a query, an INSERT, a searched or
   positioned UPDATE or DELETE, or a MERGE.
4. A positioned UPDATE or DELETE names a cursor the session declared.

## PRIVILEGES

Those the query needs. EXPLAIN reads no row, but binding the query
checks them. EXPLAIN ANALYZE reads the rows, as the query would.

Of a data change, EXPLAIN needs the privileges preparing the statement
needs, those of its access rules, such as UPDATE on the columns it sets
and SELECT on those it reads. It is shown in a READ ONLY transaction
too, where the change itself may not run. EXPLAIN ANALYZE needs those
running the statement needs.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -103 | The query or data change refers to a table, column or cursor that does not exist, or is otherwise invalid |
| 42000 | -104 | Its user lacks a privilege the query, or preparing the data change, needs |
| any | any | EXPLAIN ANALYZE raises any exception the query or data change raises as it runs |

## EXAMPLES

A query the planner answers from an index:

```sql
CREATE TABLE PARTS (ID INTEGER PRIMARY KEY, NAME VARCHAR(20));
INSERT INTO PARTS
  WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT I + 1 FROM N WHERE I < 1000)
  SELECT I, 'part ' || CAST(I AS VARCHAR(4)) FROM N;
COMMIT;
CREATE INDEX PARTS_NAME ON PARTS (NAME);
EXPLAIN SELECT ID FROM PARTS WHERE NAME = 'part 7';
```

```
OK
OK
OK
OK
 PLAN
--------------------------------------------------------------
 -- bound:
 Project PARTS.ID
   Filter PARTS.NAME = 'part 7'
     Scan ANN.PARTS
 -- rules: none
 -- optimized:
 Project PARTS.ID
   Filter PARTS.NAME = 'part 7'
     Scan ANN.PARTS
 -- physical:
 Project PARTS.ID
   IndexScan ANN.PARTS using PARTS_NAME [btree]: PARTS.NAME = 'part 7'  (est. rows=1, cost=20.6)
(12 rows)
```

A count of the rows of PARTS, a table of the row store, which its
storage counts:

```sql
EXPLAIN SELECT COUNT(*) FROM PARTS;
```

```
 PLAN
------------------------------------------------
 -- bound:
 Project COUNT(*)
   Aggregate group by [] computing [COUNT(*)]
     Scan ANN.PARTS
 -- rules: none
 -- optimized:
 Project COUNT(*)
   Aggregate group by [] computing [COUNT(*)]
     Scan ANN.PARTS
 -- physical:
 Project COUNT(*)
   Aggregate group by [] computing [COUNT(*)]
     SeqScan ANN.PARTS  (rows=1000, cost=800.0)
 -- counted by storage: ANN.PARTS
(14 rows)
```

The parts' names grouped, the first three of them. Each name is its own
group, so sorting the rows by NAME costs less than hashing them and then
sorting the groups. The grouping is streamed over the sorted rows, a
group made at a time as they come (RULES.md D550), and it stops once
three groups are given:

```sql
EXPLAIN SELECT NAME, COUNT(*) FROM PARTS GROUP BY NAME ORDER BY NAME FETCH FIRST 3 ROWS ONLY;
```

```
 PLAN
--------------------------------------------------------------
 -- bound:
 Limit fetch 3
   Sort PARTS.NAME
     Project PARTS.NAME, COUNT(*)
       Aggregate group by [PARTS.NAME] computing [COUNT(*)]
         Scan ANN.PARTS
 -- rules: none
 -- optimized:
 Limit fetch 3
   Sort PARTS.NAME
     Project PARTS.NAME, COUNT(*)
       Aggregate group by [PARTS.NAME] computing [COUNT(*)]
         Scan ANN.PARTS
 -- physical:
 Limit fetch 3
   Project PARTS.NAME, COUNT(*)
     Aggregate streamed group by [PARTS.NAME] computing [COUNT(*)]
       Sort PARTS.NAME
         SeqScan ANN.PARTS  (rows=1000, cost=900.0)
(19 rows)
```

Three tables joined, their joins ordered by enumeration. The note on
the top join gives the order chosen, the offers of the suppliers in Oslo
joined first, and the algorithm that chose it. Their parts are then
hashed rather than probed by the index of their key: the offers could
be many times more than estimated, and so many probes would cost more
than reading the parts once (PLANNER.ESTIMATE_MARGIN). Above the joins,
a phase of predicate transfer reads the three tables first, the
suppliers in Oslo filtering the offers by a Bloom filter of their keys
and those offers the parts, so each join is given only the rows that
can match (PLANNER.PREDICATE_TRANSFER). Its line tells the chain's
tables and the filters it makes. The offers' suppliers and the
suppliers' keys are read by their join alone, so the projection over it
gives them as NULL and the join above carries no copy of them:

```sql
CREATE TABLE SUPPLIERS (ID INTEGER PRIMARY KEY, CITY VARCHAR(20));
INSERT INTO SUPPLIERS VALUES (1, 'Oslo'), (2, 'Lima'), (3, 'Pune'), (4, 'Kyiv'), (5, 'Oslo'),
  (6, 'Lima'), (7, 'Pune'), (8, 'Kyiv'), (9, 'Rome'), (10, 'Rome');
CREATE TABLE OFFERS (PART INTEGER, SUPPLIER INTEGER, PRICE INTEGER);
INSERT INTO OFFERS
  WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT I + 1 FROM N WHERE I < 5000)
  SELECT I - I / 1000 * 1000 + 1, I - I / 10 * 10 + 1, I FROM N;
COMMIT;
EXPLAIN SELECT PARTS.NAME, SUPPLIERS.CITY, OFFERS.PRICE FROM PARTS, OFFERS, SUPPLIERS
  WHERE PARTS.ID = OFFERS.PART AND OFFERS.SUPPLIER = SUPPLIERS.ID AND SUPPLIERS.CITY = 'Oslo';
```

```
OK
OK
OK
OK
OK
 PLAN
--------------------------------------------------------------
 -- bound:
 Project PARTS.NAME, SUPPLIERS.CITY, OFFERS.PRICE
   Filter PARTS.ID = OFFERS.PART AND OFFERS.SUPPLIER = SUPPLIERS.ID AND SUPPLIERS.CITY = 'Oslo'
     Product
       Product
         Scan ANN.PARTS
         Scan ANN.OFFERS
       Scan ANN.SUPPLIERS
 -- rules: push_filter_into_product
 -- optimized:
 Project PARTS.NAME, SUPPLIERS.CITY, OFFERS.PRICE
   Filter OFFERS.SUPPLIER = SUPPLIERS.ID
     Product
       Filter PARTS.ID = OFFERS.PART
         Product
           Scan ANN.PARTS
           Scan ANN.OFFERS
       Filter SUPPLIERS.CITY = 'Oslo'
         Scan ANN.SUPPLIERS
 -- physical:
 PredicateTransfer of 3 leaves, 2 steps
   Project PARTS.NAME, SUPPLIERS.CITY, OFFERS.PRICE
     HashJoin PARTS.ID = OFFERS.PART  (join order by DPccp: (PARTS (OFFERS SUPPLIERS)))
       SeqScan ANN.PARTS  (rows=1000, cost=1000.0)
       Project OFFERS.PART, NULL, OFFERS.PRICE, NULL, SUPPLIERS.CITY
         HashJoin OFFERS.SUPPLIER = SUPPLIERS.ID
           SeqScan ANN.OFFERS  (rows=5000, cost=5000.0)
           Filter SUPPLIERS.CITY = 'Oslo'  (est. rows=0)
             SeqScan ANN.SUPPLIERS  (rows=10, cost=10.0)
(29 rows)
```

A correlated subquery unnested ([Query Expressions](../06-queries.md#correlated-subqueries)),
over STAFF of [Value Expressions](../04-expressions.md). The rule
`unnest_subqueries` made the subquery a grouping of every department's
salaries, computed once and joined with the outer rows by their
departments. The subquery has one row for each department, a set
function of no grouping, so its value is read as it is, with no SINGLE.
A department with no row would read the value AVG has of no rows, a
null, which the CASE gives. The comparison is never true of that null,
so `push_filter_into_outer_join` made the outer join the inner join:

```sql
CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(30), DEPT CHAR(3), SALARY DECIMAL(9,2));
INSERT INTO STAFF VALUES (1, 'Ada', 'ENG', 7200), (2, 'Grace', 'ENG', 8100),
                         (3, 'Edsger', 'OPS', 5400), (4, 'Barbara', NULL, NULL);
COMMIT;
EXPLAIN SELECT NAME FROM STAFF S WHERE SALARY > (SELECT AVG(SALARY) FROM STAFF T WHERE T.DEPT = S.DEPT);
```

```
OK
OK
OK
 PLAN
--------------------------------------------------------------
 -- bound:
 Project S.NAME
   Filter S.SALARY > (<subquery>).1
     Scan ANN.STAFF
     Subquery
       Project AVG(T.SALARY)
         Aggregate group by [] computing [AVG(T.SALARY)]
           Filter T.DEPT = S.DEPT
             Scan ANN.STAFF
 -- rules: unnest_subqueries, push_filter_into_outer_join, merge_filters
 -- optimized:
 Project S.NAME
   Project S.ID, S.NAME, S.DEPT, S.SALARY
     Filter S.DEPT = T.DEPT AND S.SALARY > CASE WHEN 1 IS NULL THEN NULL ELSE AVG(T.SALARY) END
       Product
         Project S.ID, S.NAME, S.DEPT, S.SALARY, S.DEPT
           Scan ANN.STAFF
         Project T.DEPT, 1, AVG(T.SALARY)
           Project T.DEPT, AVG(T.SALARY)
             Project AVG(T.SALARY), T.DEPT
               Project AVG(T.SALARY), T.DEPT
                 Aggregate group by [T.DEPT] computing [AVG(T.SALARY)]
                   Project T.ID, T.NAME, T.DEPT, T.SALARY, T.DEPT
                     Scan ANN.STAFF
 -- physical:
 Project S.NAME
   Filter S.SALARY > CASE WHEN 1 IS NULL THEN NULL ELSE AVG(T.SALARY) END
     HashJoin S.DEPT = T.DEPT
       Project NULL, S.NAME, NULL, S.SALARY, S.DEPT
         SeqScan ANN.STAFF  (rows=4, cost=3.8)
       Project T.DEPT, 1, AVG(T.SALARY)
         Project T.DEPT, AVG(T.SALARY)
           Aggregate group by [T.DEPT] computing [AVG(T.SALARY)]
             Project T.ID, T.NAME, T.DEPT, T.SALARY, T.DEPT
               SeqScan ANN.STAFF  (rows=4, cost=3.6)
(35 rows)
```

Set functions of all the table's rows, read in morsels of 256 rows
on up to two threads:

```sql
SET EXECUTOR.DEGREE = 2;
SET EXECUTOR.MORSEL_ROWS = 256;
SET EXECUTOR.PARALLEL_MIN_ROWS = 0;
EXPLAIN SELECT SUM(ID), MAX(NAME) FROM PARTS;
```

```
OK
OK
OK
 PLAN
--------------------------------------------------------------
 -- bound:
 Project SUM(PARTS.ID), MAX(PARTS.NAME)
   Aggregate group by [] computing [SUM(PARTS.ID), MAX(PARTS.NAME)]
     Scan ANN.PARTS
 -- rules: none
 -- optimized:
 Project SUM(PARTS.ID), MAX(PARTS.NAME)
   Aggregate group by [] computing [SUM(PARTS.ID), MAX(PARTS.NAME)]
     Scan ANN.PARTS
 -- physical:
 Project SUM(PARTS.ID), MAX(PARTS.NAME)
   Parallel  (degree=2)
     Aggregate group by [] computing [SUM(PARTS.ID), MAX(PARTS.NAME)]
       SeqScan ANN.PARTS  (rows=1000, cost=1000.0)
(14 rows)
```

A query of a DISTINCT set function, which the rule distinct_aggregates
computes by two groupings. The first groups the rows by REGION and
CUSTOMER and makes SUM in parts, and the second groups those groups by
REGION, counting each CUSTOMER once and merging the parts of SUM
([Set functions](../04-expressions.md#set-functions)):

```sql
CREATE TABLE SALES (REGION VARCHAR(10), CUSTOMER INTEGER, AMOUNT DECIMAL(9, 2));
INSERT INTO SALES VALUES ('north', 1, 10.00), ('north', 1, 5.00), ('north', 2, 7.50), ('south', 3, 2.25);
COMMIT;
EXPLAIN SELECT REGION, COUNT(DISTINCT CUSTOMER), SUM(AMOUNT) FROM SALES GROUP BY REGION;
```

```
OK
OK
OK
 PLAN
--------------------------------------------------------------
 -- bound:
 Project SALES.REGION, COUNT(DISTINCT SALES.CUSTOMER), SUM(SALES.AMOUNT)
   Aggregate group by [SALES.REGION] computing [COUNT(DISTINCT SALES.CUSTOMER), SUM(SALES.AMOUNT)]
     Scan ANN.SALES
 -- rules: distinct_aggregates
 -- optimized:
 Project SALES.REGION, COUNT(DISTINCT SALES.CUSTOMER), SUM(SALES.AMOUNT)
   Aggregate group by [SALES.REGION] computing [COUNT(DISTINCT SALES.CUSTOMER), SUM(SALES.AMOUNT)]
     Aggregate group by [SALES.REGION, SALES.CUSTOMER] computing [PARTIAL SUM(SALES.AMOUNT)]
       Project SALES.REGION, SALES.AMOUNT, SALES.CUSTOMER
         Scan ANN.SALES
 -- physical:
 Project SALES.REGION, COUNT(DISTINCT SALES.CUSTOMER), SUM(SALES.AMOUNT)
   Aggregate group by [SALES.REGION] computing [COUNT(DISTINCT SALES.CUSTOMER), SUM(SALES.AMOUNT)]
     Aggregate group by [SALES.REGION, SALES.CUSTOMER] computing [PARTIAL SUM(SALES.AMOUNT)]
       Project SALES.REGION, SALES.AMOUNT, SALES.CUSTOMER
         SeqScan ANN.SALES  (rows=4, cost=4.0)
(17 rows)
```

Sums of one column, each with a constant added or subtracted, which
the rule share_shifted_sums computes of one sum and one count of the
column, each sum then the column's sum and its constant times the count:

```sql
EXPLAIN SELECT SUM(CUSTOMER + 1), SUM(CUSTOMER + 2), SUM(10 - CUSTOMER) FROM SALES;
```

```
 PLAN
--------------------------------------------------------------
 -- bound:
 Project SUM((SALES.CUSTOMER + 1)), SUM((SALES.CUSTOMER + 2)), SUM((10 - SALES.CUSTOMER))
   Aggregate group by [] computing [SUM((SALES.CUSTOMER + 1)), SUM((SALES.CUSTOMER + 2)), SUM((10 - SALES.CUSTOMER))]
     Scan ANN.SALES
 -- rules: share_shifted_sums
 -- optimized:
 Project (SUM(SALES.CUSTOMER) + (1 * COUNT(SALES.CUSTOMER))), (SUM(SALES.CUSTOMER) + (2 * COUNT(SALES.CUSTOMER))), ((10 * COUNT(SALES.CUSTOMER)) - SUM(SALES.CUSTOMER))
   Project (SUM(SALES.CUSTOMER) + (1 * COUNT(SALES.CUSTOMER))), (SUM(SALES.CUSTOMER) + (2 * COUNT(SALES.CUSTOMER))), ((10 * COUNT(SALES.CUSTOMER)) - SUM(SALES.CUSTOMER))
     Aggregate group by [] computing [SUM(SALES.CUSTOMER), COUNT(SALES.CUSTOMER)]
       Scan ANN.SALES
 -- physical:
 Project (SUM(SALES.CUSTOMER) + (1 * COUNT(SALES.CUSTOMER))), (SUM(SALES.CUSTOMER) + (2 * COUNT(SALES.CUSTOMER))), ((10 * COUNT(SALES.CUSTOMER)) - SUM(SALES.CUSTOMER))
   Project (SUM(SALES.CUSTOMER) + (1 * COUNT(SALES.CUSTOMER))), (SUM(SALES.CUSTOMER) + (2 * COUNT(SALES.CUSTOMER))), ((10 * COUNT(SALES.CUSTOMER)) - SUM(SALES.CUSTOMER))
     Aggregate group by [] computing [SUM(SALES.CUSTOMER), COUNT(SALES.CUSTOMER)]
       SeqScan ANN.SALES  (rows=4, cost=3.5)
(15 rows)
```

A query that reads only the employees, of a LEFT join to their
department on its key, which the rule eliminate_joins removes: each
employee joins at most one department, and nothing above reads it
([How joins are planned](../06-queries.md#how-joins-are-planned-extension)):

```sql
CREATE TABLE DEPT (ID INTEGER PRIMARY KEY, NAME VARCHAR(10));
CREATE TABLE EMP (ID INTEGER PRIMARY KEY, DEPT INTEGER NOT NULL REFERENCES DEPT, PAY INTEGER);
INSERT INTO DEPT VALUES (1, 'north'), (2, 'south');
INSERT INTO EMP VALUES (1, 1, 100), (2, 2, 200), (3, 2, 300);
COMMIT;
EXPLAIN SELECT E.ID, E.PAY FROM EMP E LEFT JOIN DEPT D ON E.DEPT = D.ID;
```

```
OK
OK
OK
OK
OK
 PLAN
---------------------------------------
 -- bound:
 Project E.ID, E.PAY
   Left outer join on E.DEPT = D.ID
     Scan ANN.EMP
     Scan ANN.DEPT
 -- rules: eliminate_joins
 -- optimized:
 Project E.ID, E.PAY
   Scan ANN.EMP
 -- physical:
 Project E.ID, E.PAY
   SeqScan ANN.EMP  (rows=3, cost=3.0)
(12 rows)
```

Each department's pay, grouped by the department's key and its name.
The rule reduce_grouping_keys groups by the key alone and carries the
name, which the key decides
([Functionally dependent columns](../06-queries.md#functionally-dependent-columns)):

```sql
CREATE TABLE OFFICE (ID INTEGER PRIMARY KEY, NAME VARCHAR(10));
CREATE TABLE HAND (ID INTEGER PRIMARY KEY, DEPT INTEGER NOT NULL, PAY INTEGER);
INSERT INTO OFFICE VALUES (1, 'north'), (2, 'south');
INSERT INTO HAND VALUES (1, 1, 100), (2, 2, 200), (3, 2, 300);
COMMIT;
EXPLAIN SELECT D.ID, D.NAME, SUM(E.PAY) FROM OFFICE D JOIN HAND E ON E.DEPT = D.ID GROUP BY D.ID, D.NAME;
```

```
OK
OK
OK
OK
OK
 PLAN
--------------------------------------------------------------
 -- bound:
 Project D.ID, D.NAME, SUM(E.PAY)
   Aggregate group by [D.ID, D.NAME] computing [SUM(E.PAY)]
     Filter E.DEPT = D.ID
       Product
         Scan ANN.OFFICE
         Scan ANN.HAND
 -- rules: reduce_grouping_keys
 -- optimized:
 Project D.ID, D.NAME, SUM(E.PAY)
   Project D.ID, D.NAME, SUM(E.PAY)
     Aggregate group by [D.ID] computing [SUM(E.PAY), D.NAME]
       Filter E.DEPT = D.ID
         Product
           Scan ANN.OFFICE
           Scan ANN.HAND
 -- physical:
 Project D.ID, D.NAME, SUM(E.PAY)
   Aggregate group by [D.ID] computing [SUM(E.PAY), D.NAME]
     Project D.ID, D.NAME, E.ID, E.DEPT, E.PAY
       HashJoin E.DEPT = D.ID
         SeqScan ANN.HAND  (rows=3, cost=2.8)
         SeqScan ANN.OFFICE  (rows=2, cost=2.0)
(23 rows)
```

The department of the greatest pay, of the same tables. The rule
fetch_dependents_late leaves the name out of the grouping and the sort,
and reads it of OFFICE by its key for the one row FETCH FIRST keeps. The
rows are sorted again above that join:

```sql
EXPLAIN SELECT D.ID, D.NAME, SUM(E.PAY) AS PAID FROM OFFICE D JOIN HAND E ON E.DEPT = D.ID
    GROUP BY D.ID, D.NAME ORDER BY PAID DESC FETCH FIRST 1 ROW ONLY;
```

```
 PLAN
--------------------------------------------------------------
 -- bound:
 Limit fetch 1
   Sort SUM(E.PAY) DESC
     Project D.ID, D.NAME, SUM(E.PAY)
       Aggregate group by [D.ID, D.NAME] computing [SUM(E.PAY)]
         Filter E.DEPT = D.ID
           Product
             Scan ANN.OFFICE
             Scan ANN.HAND
 -- rules: reduce_grouping_keys, fetch_dependents_late
 -- optimized:
 Sort SUM(E.PAY) DESC
   Project D.ID, D.NAME, SUM(E.PAY)
     Filter D.ID = D.ID
       Product
         Limit fetch 1
           Sort SUM(E.PAY) DESC
             Project D.ID, SUM(E.PAY), D.ID
               Project D.ID, SUM(E.PAY), D.ID
                 Aggregate group by [D.ID] computing [SUM(E.PAY)]
                   Filter E.DEPT = D.ID
                     Product
                       Scan ANN.OFFICE
                       Scan ANN.HAND
         Scan ANN.OFFICE
 -- physical:
 Sort SUM(E.PAY) DESC
   Project D.ID, D.NAME, SUM(E.PAY)
     HashJoin D.ID = D.ID
       SeqScan ANN.OFFICE  (rows=2, cost=2.0)
       Limit fetch 1
         Sort SUM(E.PAY) DESC
           Project D.ID, SUM(E.PAY), D.ID
             Aggregate group by [D.ID] computing [SUM(E.PAY)]
               Project D.ID, D.NAME, E.ID, E.DEPT, E.PAY
                 HashJoin E.DEPT = D.ID
                   SeqScan ANN.HAND  (rows=3, cost=2.8)
                   SeqScan ANN.OFFICE  (rows=2, cost=1.8)
(38 rows)
```

The hands of each department counted, grouped by the department and by
a number computed of it in a derived table. The rule
compute_keys_after_grouping groups by the department alone and computes
the number of each group, as a query that wrote the expression in its
select list would
([Functionally dependent columns](../06-queries.md#functionally-dependent-columns)):

```sql
EXPLAIN SELECT DEPT, ROOM, COUNT(*) FROM (SELECT DEPT, DEPT + 100 AS ROOM FROM HAND) AS H GROUP BY DEPT, ROOM;
```

```
 PLAN
--------------------------------------------------------------
 -- bound:
 Project HAND.DEPT, (HAND.DEPT + 100), COUNT(*)
   Aggregate group by [HAND.DEPT, (HAND.DEPT + 100)] computing [COUNT(*)]
     Project HAND.DEPT, (HAND.DEPT + 100)
       Scan ANN.HAND
 -- rules: compute_keys_after_grouping x2
 -- optimized:
 Project HAND.DEPT, (HAND.DEPT + 100), COUNT(*)
   Aggregate group by [HAND.DEPT] computing [COUNT(*)]
     Scan ANN.HAND
 -- physical:
 Project HAND.DEPT, (HAND.DEPT + 100), COUNT(*)
   Aggregate group by [HAND.DEPT] computing [COUNT(*)]
     SeqScan ANN.HAND  (rows=3, cost=2.6)
(14 rows)
```

The department of the most hands, of the same query. The rule
compute_after_fetch_first computes the number above the FETCH FIRST, of
the one row kept, and the grouping below is sorted by its count as if
the query had selected the department and the count alone:

```sql
EXPLAIN SELECT DEPT, ROOM, COUNT(*) AS N FROM (SELECT DEPT, DEPT + 100 AS ROOM FROM HAND) AS H GROUP BY DEPT, ROOM
    ORDER BY N DESC FETCH FIRST 1 ROW ONLY;
```

```
 PLAN
--------------------------------------------------------------
 -- bound:
 Limit fetch 1
   Sort COUNT(*) DESC
     Project HAND.DEPT, (HAND.DEPT + 100), COUNT(*)
       Aggregate group by [HAND.DEPT, (HAND.DEPT + 100)] computing [COUNT(*)]
         Project HAND.DEPT, (HAND.DEPT + 100)
           Scan ANN.HAND
 -- rules: compute_keys_after_grouping x2, compute_after_fetch_first
 -- optimized:
 Project HAND.DEPT, (HAND.DEPT + 100), COUNT(*)
   Limit fetch 1
     Sort COUNT(*) DESC
       Project HAND.DEPT, COUNT(*)
         Aggregate group by [HAND.DEPT] computing [COUNT(*)]
           Scan ANN.HAND
 -- physical:
 Project HAND.DEPT, (HAND.DEPT + 100), COUNT(*)
   Limit fetch 1
     Sort COUNT(*) DESC
       Project HAND.DEPT, COUNT(*)
         Aggregate group by [HAND.DEPT] computing [COUNT(*)]
           SeqScan ANN.HAND  (rows=3, cost=2.6)
(22 rows)
```

A join of SALES and PARTS, run on one thread. Each row of SALES probes
the index of PARTS's key:

```sql
SET EXECUTOR.DEGREE = 1;
EXPLAIN ANALYZE SELECT S.REGION, P.NAME FROM SALES S, PARTS P WHERE P.ID = S.CUSTOMER;
```

```
OK
 PLAN
--------------------------------------------------------------
 -- physical:
 Project S.REGION, P.NAME  (est. rows=4)  (actual rows=4)
   IndexNestedLoop ANN.PARTS using ANN.PARTS.UNIQUE1 [btree]: S.CUSTOMER = P.ID  (est. rows each=1, cost=82.4)  (actual rows=4, probes=4, entries=4, leaf pages=4, fetched=4)
     SeqScan ANN.SALES  (rows=4, cost=3.7)  (actual rows=4, leaf pages=1)
 -- ran: 4 rows
(5 rows)
```

A query that stops after its first rows, its scan reading 16 of the
1000 rows it holds. Read by batches, the scan counts the rows of each
batch it gave, which may be more than the operator above it took, and
shows its one batch:

```sql
EXPLAIN ANALYZE SELECT ID FROM PARTS FETCH FIRST 3 ROWS ONLY;
```

```
 PLAN
--------------------------------------------------------------
 -- physical:
 Limit fetch 3  (est. rows=3)  (actual rows=3)
   Project PARTS.ID  (est. rows=1000)  (actual rows=3)
     SeqScan ANN.PARTS  (rows=1000, cost=900.0)  (actual rows=16, batches=1, mean rows=16)
 -- ran: 3 rows
(5 rows)
```

The parts found by the index of their names, the index's reads shown:
the entry it took, the leaf of its tree it read, and the row it fetched
by the entry:

```sql
EXPLAIN ANALYZE SELECT ID FROM PARTS WHERE NAME = 'part 7';
```

```
 PLAN
--------------------------------------------------------------
 -- physical:
 Project PARTS.ID  (est. rows=1)  (actual rows=1)
   IndexScan ANN.PARTS using PARTS_NAME [btree]: PARTS.NAME = 'part 7'  (est. rows=1, cost=20.6)  (actual rows=1, entries=1, leaf pages=1, fetched=1)
 -- ran: 1 row
(4 rows)
```

The offers sorted by price in a work area too small to hold them. The
sort shows what it spilled, the runs it wrote and the passes it read
them back in:

```sql
SET MEMORY.WORK_BYTES = 65536;
EXPLAIN ANALYZE SELECT PART, PRICE FROM OFFERS ORDER BY PRICE DESC;
SET MEMORY.WORK_BYTES = 268435456;
```

```
OK
 PLAN
--------------------------------------------------------------
 -- physical:
 Sort OFFERS.PRICE DESC  (est. rows=5000)  (actual rows=5000, spilled=250783 bytes, partitions=5, passes=3)
   Project OFFERS.PART, OFFERS.PRICE  (est. rows=5000)  (actual rows=5000, batches=9, mean rows=556)
     SeqScan ANN.OFFERS  (rows=5000, cost=4666.7)  (actual rows=5000, batches=9, mean rows=556)
 -- ran: 5000 rows
(5 rows)
OK
```

An UPDATE run by EXPLAIN ANALYZE, which raises the 50 lowest prices in
the session's transaction. The first line counts the rows it changed,
the plan below it the rows it read. The change is seen until the
ROLLBACK undoes it:

```sql
EXPLAIN ANALYZE UPDATE OFFERS SET PRICE = PRICE + 1 WHERE PRICE <= 50;
SELECT MIN(PRICE) FROM OFFERS;
ROLLBACK;
SELECT MIN(PRICE) FROM OFFERS;
```

```
 PLAN
--------------------------------------------------------------
 -- physical:
 Update ANN.OFFERS  (actual rows changed=50)
   Filter OFFERS.PRICE <= 50  (est. rows=51)  (actual rows=50)
     SeqScan ANN.OFFERS  (rows=5000, cost=5000.0)  (actual rows=5000, leaf pages=29)
 -- ran: 50 rows changed
(5 rows)

--------------
            2
(1 row)
OK

--------------
            1
(1 row)
```

A band join of PARTS and SALES, each part paired with the sales whose
customer is within two of its key. The right operand, SALES, is sorted
on its customer once, and each part searches it between its bounds:

```sql
EXPLAIN SELECT S.REGION, P.NAME FROM PARTS P, SALES S
  WHERE S.CUSTOMER BETWEEN P.ID - 2 AND P.ID + 2;
```

```
 PLAN
--------------------------------------------------------------
 -- bound:
 Project S.REGION, P.NAME
   Filter S.CUSTOMER >= (P.ID - 2) AND S.CUSTOMER <= (P.ID + 2)
     Product
       Scan ANN.PARTS
       Scan ANN.SALES
 -- rules: none
 -- optimized:
 Project S.REGION, P.NAME
   Filter S.CUSTOMER >= (P.ID - 2) AND S.CUSTOMER <= (P.ID + 2)
     Product
       Scan ANN.PARTS
       Scan ANN.SALES
 -- physical:
 Project S.REGION, P.NAME
   RangeJoin S.CUSTOMER >= (P.ID - 2) AND S.CUSTOMER <= (P.ID + 2)  (sorted on S.CUSTOMER)
     SeqScan ANN.PARTS  (rows=1000, cost=1000.0)
     SeqScan ANN.SALES  (rows=4, cost=3.7)
(18 rows)
```

An INSERT of a row of values, shown without running it. Its values are
evaluated, with no plan, and its row is written to the index of PARTS's
key and to PARTS_NAME. A procedure's body would write its row straight
on the table, which no trigger or referential action reaches:

```sql
EXPLAIN INSERT INTO PARTS VALUES (1001, 'part 1001');
SELECT COUNT(*) FROM PARTS;
```

```
 PLAN
--------------------------------------------------------------
 -- physical:
 Insert into ANN.PARTS
 -- maintains: ANN.PARTS.UNIQUE1 [btree], PARTS_NAME [btree]
 -- point path: its values evaluated, with no plan, its rows written straight in a procedure
(4 rows)

----------------------
                 1000
(1 row)
```

An INSERT of a query's rows shows the query's plan:

```sql
EXPLAIN INSERT INTO SALES SELECT 'west', SUPPLIER, PRICE FROM OFFERS WHERE PART = 1;
```

```
 PLAN
---------------------------------------------------------
 -- physical:
 Insert into ANN.SALES
   Project 'west', OFFERS.SUPPLIER, OFFERS.PRICE
     Filter OFFERS.PART = 1  (est. rows=4999)
       SeqScan ANN.OFFERS  (rows=5000, cost=5000.0)
 -- maintains: none
 -- point path: its rows written straight in a procedure
(7 rows)
```

An UPDATE of a part by its key. The plan probes the key's index, and the
UPDATE writes PARTS_NAME, whose key reads the column it sets. Prepared,
or in a procedure, its row would be read by one probe of the key:

```sql
EXPLAIN UPDATE PARTS SET NAME = 'bolt' WHERE ID = 7;
```

```
 PLAN
--------------------------------------------------------------
 -- physical:
 Update ANN.PARTS
   IndexScan ANN.PARTS using ANN.PARTS.UNIQUE1 [btree]: PARTS.ID = 7  (est. rows=1, cost=20.6)
 -- maintains: PARTS_NAME [btree]
 -- point path: one read of ANN.PARTS.UNIQUE1 when prepared or in a procedure, its row written straight in a procedure
(5 rows)
```

Orders whose rows go with their part, and a trigger that notes each
order deleted. A DELETE of a part cascades to its orders, and the
trigger would fire for them. Nothing is deleted, and GONE stays empty:

```sql
CREATE TABLE ORDERS (ID INTEGER PRIMARY KEY, PART INTEGER REFERENCES PARTS ON DELETE CASCADE);
CREATE TABLE GONE (ID INTEGER);
CREATE TRIGGER ORDER_GONE AFTER DELETE ON ORDERS REFERENCING OLD ROW AS O
  FOR EACH ROW INSERT INTO GONE VALUES (O.ID);
INSERT INTO ORDERS VALUES (1, 7), (2, 7), (3, 8);
COMMIT;
EXPLAIN DELETE FROM PARTS WHERE ID = 7;
SELECT COUNT(*) FROM ORDERS;
SELECT COUNT(*) FROM GONE;
```

```
OK
OK
OK
OK
OK
 PLAN
--------------------------------------------------------------
 -- physical:
 Delete from ANN.PARTS
   IndexScan ANN.PARTS using ANN.PARTS.UNIQUE1 [btree]: PARTS.ID = 7  (est. rows=1, cost=20.6)
 -- maintains: ANN.PARTS.UNIQUE1 [btree], PARTS_NAME [btree]
 -- cascades: ON DELETE CASCADE to ANN.ORDERS by ANN.ORDERS_FOREIGN1
 -- trigger: ANN.ORDER_GONE, AFTER DELETE FOR EACH ROW on ANN.ORDERS
 -- point path: one read of ANN.PARTS.UNIQUE1 when prepared or in a procedure
(7 rows)

----------------------
                    3
(1 row)

----------------------
                    0
(1 row)
```

A MERGE of the sales into the parts, by their customers. The plan is
that of the rows it joins, sorted by the target's rows, and its clauses
would write both indexes of PARTS:

```sql
EXPLAIN MERGE INTO PARTS P USING SALES S ON P.ID = S.CUSTOMER
  WHEN MATCHED THEN UPDATE SET NAME = S.REGION
  WHEN NOT MATCHED THEN INSERT VALUES (S.CUSTOMER, S.REGION);
```

```
 PLAN
--------------------------------------------------------------
 -- physical:
 Merge into ANN.PARTS
   Sort row id
     Project P.ID, P.NAME, S.REGION, S.CUSTOMER, S.AMOUNT, row id
       LeftOuterIndexNestedLoop ANN.PARTS using ANN.PARTS.UNIQUE1 [btree]: S.CUSTOMER = P.ID  (est. rows each=1, cost=82.4)
         Project S.REGION, S.CUSTOMER, S.AMOUNT
           SeqScan ANN.SALES  (rows=4, cost=4.0)
 -- maintains: ANN.PARTS.UNIQUE1 [btree], PARTS_NAME [btree]
 -- point path: none
(9 rows)
```

## STANDARD

EXPLAIN is an extension of turborel. No part of ISO/IEC 9075 shows how
a statement is planned. EXPLAIN of a data change checks what preparing
it checks, its Format, Syntax Rules and Access Rules [ISO/IEC 9075-2:2023
§20.7 GR6]. In GET DIAGNOSTICS it is COMMAND_FUNCTION
`EXPLAIN`, code -14, with or without ANALYZE.

## SEE ALSO

[SELECT](select.md), [DECLARE CURSOR](declare-cursor.md),
[INSERT](insert.md), [UPDATE](update.md), [DELETE](delete.md), [MERGE](merge.md),
[CREATE INDEX](create-index.md), [SET](set-setting.md)
