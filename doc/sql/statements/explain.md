# EXPLAIN

[Statements](README.md)

## NAME

EXPLAIN: show how a query is planned *(extension)*

## SYNOPSIS

```
<explain statement> ::=
    EXPLAIN <cursor specification>
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
PLANNER.REORDER_GAIN does, has no note. A merge join shows as
`MergeJoin`, or `LeftMergeJoin` of a LEFT join, with its key.

A part of the physical plan run on several threads at once is headed
`Parallel` with its degree, the most threads it takes, the statement's
own among them; the planner runs a part so only if its tables hold
enough rows ([Parallel queries](../../operator/04-settings.md#parallel-queries)).

EXPLAIN is direct SQL, as a query is. The `turborel` client shows its
rows as it shows a query's. A program reaches the same text by the
library's `Session::explain` ([The Embedded Library](../../client/03-embedding.md#reading-rows)),
which also takes values for a query's parameters, as direct SQL has
none.

EXPLAIN is not a key word. A table may be named EXPLAIN, and EXPLAIN is
read as this statement only before SELECT, VALUES, TABLE, WITH or a
parenthesis.

## RULES

1. The query is one a cursor could be declared for, FOR UPDATE aside, as
   for a query run directly.
2. It is bound as a query run then would be, so it refers only to tables
   and columns that exist.

## PRIVILEGES

Those the query needs. EXPLAIN reads no row, but binding the query
checks them.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The query refers to a table or column that does not exist, or is otherwise invalid |
| 42000 | -104 | Its user lacks a privilege the query needs |

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

Three tables joined, their joins ordered by enumeration. The note on
the top join gives the order chosen, the offers of the suppliers in Oslo
first, then their parts by the index of its key, and the algorithm that
chose it:

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
 Project PARTS.NAME, SUPPLIERS.CITY, OFFERS.PRICE
   IndexNestedLoop ANN.PARTS using ANN.PARTS.UNIQUE1 [btree]: OFFERS.PART = PARTS.ID  (join order by DPccp: ((OFFERS SUPPLIERS) PARTS))  (est. rows each=1, cost=20.6)
     HashJoin OFFERS.SUPPLIER = SUPPLIERS.ID
       SeqScan ANN.OFFERS  (rows=5000, cost=5000.0)
       Filter SUPPLIERS.CITY = 'Oslo'  (est. rows=0)
         SeqScan ANN.SUPPLIERS  (rows=10, cost=10.0)
(26 rows)
```

A correlated subquery unnested ([Query Expressions](../06-queries.md#correlated-subqueries)),
over STAFF of [Value Expressions](../04-expressions.md). The rule
`unnest_subqueries` made the subquery a grouping of each department's
salaries, joined with the outer rows by their departments. Each
department is taken once, as a value and the count of its trailing
spaces. The subquery has one row for each department, a set function of
no grouping, so its value is read as it is, with no SINGLE:

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
 -- rules: unnest_subqueries
 -- optimized:
 Project S.NAME
   Project S.ID, S.NAME, S.DEPT, S.SALARY
     Filter S.SALARY > AVG(T.SALARY)
       Left outer join on NOT (S.DEPT IS DISTINCT FROM S.DEPT) AND NOT (TRAILING_SPACES(S.DEPT) IS DISTINCT FROM TRAILING_SPACES(S.DEPT))
         Project S.ID, S.NAME, S.DEPT, S.SALARY, S.DEPT, TRAILING_SPACES(S.DEPT)
           Scan ANN.STAFF
         Project S.DEPT, TRAILING_SPACES(S.DEPT), 1, AVG(T.SALARY)
           Project S.DEPT, TRAILING_SPACES(S.DEPT), AVG(T.SALARY)
             Aggregate group by [S.DEPT, TRAILING_SPACES(S.DEPT)] computing [AVG(T.SALARY)]
               Left outer join on NOT (S.DEPT IS DISTINCT FROM S.DEPT) AND NOT (TRAILING_SPACES(S.DEPT) IS DISTINCT FROM TRAILING_SPACES(S.DEPT))
                 Distinct
                   Project S.DEPT, TRAILING_SPACES(S.DEPT)
                     Scan ANN.STAFF
                 Project S.DEPT, TRAILING_SPACES(S.DEPT), T.ID, T.NAME, T.DEPT, T.SALARY, TRUE
                   Filter T.DEPT = S.DEPT
                     Product
                       Distinct
                         Project S.DEPT, TRAILING_SPACES(S.DEPT)
                           Scan ANN.STAFF
                       Scan ANN.STAFF
 -- physical:
 Project S.NAME
   Filter S.SALARY > AVG(T.SALARY)
     LeftOuterJoin NOT (S.DEPT IS DISTINCT FROM S.DEPT) AND NOT (TRAILING_SPACES(S.DEPT) IS DISTINCT FROM TRAILING_SPACES(S.DEPT))  (hashed on S.DEPT IS NOT DISTINCT FROM S.DEPT AND TRAILING_SPACES(S.DEPT) IS NOT DISTINCT FROM TRAILING_SPACES(S.DEPT))
       Project S.ID, S.NAME, S.DEPT, S.SALARY, S.DEPT, TRAILING_SPACES(S.DEPT)
         SeqScan ANN.STAFF  (rows=4, cost=3.8)
       Project S.DEPT, TRAILING_SPACES(S.DEPT), 1, AVG(T.SALARY)
         Project S.DEPT, TRAILING_SPACES(S.DEPT), AVG(T.SALARY)
           Aggregate group by [S.DEPT, TRAILING_SPACES(S.DEPT)] computing [AVG(T.SALARY)]
             LeftOuterJoin NOT (S.DEPT IS DISTINCT FROM S.DEPT) AND NOT (TRAILING_SPACES(S.DEPT) IS DISTINCT FROM TRAILING_SPACES(S.DEPT))  (hashed on S.DEPT IS NOT DISTINCT FROM S.DEPT AND TRAILING_SPACES(S.DEPT) IS NOT DISTINCT FROM TRAILING_SPACES(S.DEPT))
               Distinct
                 Project S.DEPT, TRAILING_SPACES(S.DEPT)
                   SeqScan ANN.STAFF  (rows=4, cost=3.8)
               Project S.DEPT, TRAILING_SPACES(S.DEPT), T.ID, T.NAME, T.DEPT, T.SALARY, TRUE
                 HashJoin S.DEPT = T.DEPT
                   Distinct
                     Project S.DEPT, TRAILING_SPACES(S.DEPT)
                       SeqScan ANN.STAFF  (rows=4, cost=3.8)
                   SeqScan ANN.STAFF  (rows=4, cost=3.6)
(50 rows)
```

A set function of most of the table's rows, read in morsels of 256 rows
on up to four threads:

```sql
SET EXECUTOR.DEGREE = 4;
SET EXECUTOR.MORSEL_ROWS = 256;
SET EXECUTOR.PARALLEL_MIN_ROWS = 0;
EXPLAIN SELECT COUNT(*), MAX(NAME) FROM PARTS WHERE ID > 100;
```

```
OK
OK
OK
 PLAN
-----------------------------------------------------------------
 -- bound:
 Project COUNT(*), MAX(PARTS.NAME)
   Aggregate group by [] computing [COUNT(*), MAX(PARTS.NAME)]
     Filter PARTS.ID > 100
       Scan ANN.PARTS
 -- rules: none
 -- optimized:
 Project COUNT(*), MAX(PARTS.NAME)
   Aggregate group by [] computing [COUNT(*), MAX(PARTS.NAME)]
     Filter PARTS.ID > 100
       Scan ANN.PARTS
 -- physical:
 Project COUNT(*), MAX(PARTS.NAME)
   Parallel  (degree=4)
     Aggregate group by [] computing [COUNT(*), MAX(PARTS.NAME)]
       Filter PARTS.ID > 100  (est. rows=333)
         SeqScan ANN.PARTS  (rows=1000, cost=1000.0)
(17 rows)
```

## STANDARD

EXPLAIN is an extension of turborel. No part of ISO/IEC 9075 shows how
a statement is planned. In GET DIAGNOSTICS it is COMMAND_FUNCTION
`EXPLAIN`, code -14.

## SEE ALSO

[SELECT](select.md), [DECLARE CURSOR](declare-cursor.md),
[CREATE INDEX](create-index.md), [SET](set-setting.md)
