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

A correlated subquery unnested ([Query Expressions](../06-queries.md#correlated-subqueries)),
over STAFF of [Value Expressions](../04-expressions.md). The rule
`unnest_subqueries` made the subquery a grouping of each department's
salaries, joined with the outer rows by their departments. Each
department is taken once, as a value and the count of its trailing
spaces, and SINGLE stands where the subquery stood:

```sql
EXPLAIN SELECT NAME FROM STAFF S WHERE SALARY > (SELECT AVG(SALARY) FROM STAFF T WHERE T.DEPT = S.DEPT);
```

```
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
     Filter S.SALARY > SINGLE(1, AVG(T.SALARY))
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
   Filter S.SALARY > SINGLE(1, AVG(T.SALARY))
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

## STANDARD

EXPLAIN is an extension of turborel. No part of ISO/IEC 9075 shows how
a statement is planned. In GET DIAGNOSTICS it is COMMAND_FUNCTION
`EXPLAIN`, code -14.

## SEE ALSO

[SELECT](select.md), [DECLARE CURSOR](declare-cursor.md),
[CREATE INDEX](create-index.md)
