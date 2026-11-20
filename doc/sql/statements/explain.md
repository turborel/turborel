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
[CREATE INDEX](create-index.md)
