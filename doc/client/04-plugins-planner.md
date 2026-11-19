# Optimizer Rules and Statistics

[Contents](README.md)

The planner takes plug-ins at two places. An optimizer rule rewrites
the logical plan of each statement as it is planned. Statistics cross
both ways: a storage engine or an index method tells the planner what it
knows of its rows, and a plug-in asks the database what it keeps of a
table, a column or an index. What each costs is a count of calls. A
rule costs one call a pass of every statement planned. An engine's or
an index's statistics cost one call when first asked, and one each time
they go stale. A plug-in asking for statistics costs one round trip a
question. Whether that is worth it is the operator's choice.

The examples run with the example plug-in, its manifest declaring these
objects.

```text
OPTIMIZER RULE CONTRADICTION PHASE BOUND;
OPTIMIZER RULE ERRING;
STORAGE ENGINE FILES;
INDEX METHOD SORTED SUPPORTS (EQUALITY, RANGE);
PROCEDURE SKEW(VARCHAR(128), VARCHAR(128)) MODIFIES SQL DATA;
```

```sql
CREATE EXTENSION EXAMPLE;
CREATE TABLE T (A INTEGER, B INTEGER);
INSERT INTO T VALUES (1, 1), (2, 2), (3, 3), (4, 0), (5, 1), (6, 2), (7, 3), (8, 0);
```

```
OK
OK
OK
```

## Optimizer rules

An optimizer rule is kind 37. It is declared with the phase it runs in
and the passes it makes at most.

```text
OPTIMIZER RULE <name> [ PHASE { BOUND | REWRITTEN } ] [ PASSES <count> ]
```

A rule of PHASE BOUND rewrites the plan as bound, before the built-in
rules. A rule of PHASE REWRITTEN, the default, rewrites the plan they
made, before the physical planner orders its joins and chooses its
paths. The rules of a phase run in the order of their extensions'
names, then in the order their manifest declares them. Each runs a
pass at a time until it answers no change, or until its PASSES (1
unless written) or OPTIMIZER.MAX_PASSES is reached.

A pass is one CALL of operation 1, REWRITE. Its payload is the plan in
the plan form, and its RESULT a byte 0 for no change or a byte 1 and
the plan rewritten. The example's CONTRADICTION finds a filter no row
passes, a column above one literal and below another, and makes it an
empty VALUES. EXPLAIN names it among the rules that fired, and no table
is read.

```sql
EXPLAIN SELECT A FROM T WHERE A > 5 AND A < 3;
```

```
 PLAN
---------------------------------------------------------
 -- bound:
 Project T.A
   Filter T.A > 5 AND T.A < 3
     Scan ANN.T
 -- rules: EXAMPLE.CONTRADICTION, EXAMPLE.ERRING skipped
 -- optimized:
 Project #0
   Values
 -- physical:
 Project #0
   Values
(11 rows)
```

A rule is an optimization, so its failure fails nothing. A rule whose
call fails, runs past PLUGINS.CALL_TIMEOUT_MS, or gives a plan that is
not valid is skipped for the statement, every pass it made undone.
EXPLAIN names it skipped, and the statement completes with the warning
SQLSTATE 01T02 ([Appendix A](../appendix/a-sqlstate.md)). The example's
ERRING always fails.

```sql
EXPLAIN SELECT A FROM T WHERE A > 6;
```

```
 PLAN
---------------------------------------
 -- bound:
 Project T.A
   Filter T.A > 6
     Scan ANN.T
 -- rules: EXAMPLE.ERRING skipped
 -- optimized:
 Project T.A
   Filter T.A > 6
     Scan ANN.T
 -- physical:
 Project T.A
   Filter T.A > 6  (est. rows=3)
     SeqScan ANN.T  (rows=8, cost=7.2)
(13 rows)
```

A prepared statement keeps its plan, so it makes no call when run
again. The subqueries a statement runs again for each row are planned
once with it, and a rule sees them as kept expressions.

## The plan form

The plan form is format 1, apart from the planner's own types, so a
program built against one release rewrites another's plans. A plan is a
tree of nodes. Each node has an operator code, the planner's id of it (0
for a node a rule made), whether it is kept, its output columns each
with its name and type, the planner's estimates of its rows and of a
row's width, a body, and its inputs.

```text
plan:   format u32 (1) | node
node:   op u8 | id u32 | kept u8 | columns: count u32 | (name str | type str)...
        | rows f64 | width f64 | body | inputs: count u32 | node...
```

| Code | Operator | Body |
|---|---|---|
| 1 | SCAN | table str, then each column's statistics: distinct f64, nulls f64, u8 0 or u8 1 and its least and greatest values |
| 2 | FILTER | condition |
| 3 | PROJECT | count u32, expressions |
| 4 | JOIN | kind u8 (0 cross, 1 left, 2 right, 3 full), u8 0, or u8 1 and its condition |
| 5 | AGGREGATE | kept alone |
| 6 | SORT | count u32, each column u32, descending u8, nulls first u8 |
| 7 | LIMIT | kept alone |
| 8 | DISTINCT | nothing |
| 9 | SET | op u8 (1 union, 2 except, 3 intersect), all u8 |
| 10 | VALUES | count u32 rows, each count u32 expressions |
| 11 to 18 | SYSTEM, RECURSIVE, WORK TABLE, TRANSITION, UNNEST, WINDOW, RECOGNIZE, POLYMORPHIC | kept alone |

A kept node has a label str as its body, for people. Its body is the
planner's, and the rule may move it to another place but not open it.
Its inputs stay open, and each must give the columns and types it gave.
A recursive query and a polymorphic table function keep their inputs
too.

| Code | Expression | Body |
|---|---|---|
| 1 | COLUMN | index u32 of the input's columns, the two of a join one row |
| 2 | LITERAL | type str, a value of protocol 1 |
| 3 | CALL | name str, count u32, arguments |
| 4 | OPERATOR | op u8, count u32, operands |
| 5 | CAST | type str, operand |
| 6 | OPAQUE | id u32, count u32, the columns it reads |

The operators are 1 AND, 2 OR, 3 NOT, 4 `=`, 5 `<>`, 6 `<`, 7 `>`, 8
`<=`, 9 `>=`, 10 IS NULL, 11 IS NOT NULL, 12 `+`, 13 `-`, 14 `*`, 15
`/` and 16 negation. A type is a `<data type>` as SQL writes it, empty
where the planner does not know it, and a number not known is negative.
A function is named by CALL only where its name finds it alone. An
expression the form cannot say is OPAQUE, as a subquery, a CASE or a
LIKE. A rule may name its columns again, as other columns of the same
row, and may move it, but may not open it or use it twice.

The planner checks the plan it is given back. Each node must have the
inputs its operator takes, each column must be one of its input's, each
table one the catalog holds, and each kept node and opaque expression
one it was given, used once. Each function must take its arguments,
each comparison must be of comparable types, and each condition must be
BOOLEAN. The plan must give the columns and types it gave. That the plan
returns the same rows is the rule's own contract, which the planner
cannot check.

## Statistics for the planner

A storage engine answers operation 8, STATISTICS, of a table's locator,
and an index method operation 9 of an index's.

```text
table:   u8 0 (none) | u8 1 | rows f64 | width f64 | refresh u64 | count u32 | column...
column:  distinct f64 | nulls f64 | u8 0, or u8 1 | least value | greatest value
         | count u32 | histogram bound values... | u8 0, or u8 1 | most f64 | median f64 | p90 f64 | p99 f64
index:   u8 0 (none) | u8 1 | entries f64 | distinct keys f64 | pages f64 | refresh u64
```

A histogram's values are its bounds, ascending, each bucket between two
of them holding as many values. The frequencies are how many times the
most repeated value appears, and the median, 90th and 99th percentile
of the repetitions. The first time the planner asks, the program is
asked, and its answer is kept, so later questions cost no call. It is
asked again at the planner's next question once the rows or entries
written since reach `refresh`, and when the table is analyzed
(`Database::analyze`). An answer of none, or a call that fails, is kept
as none until then, and the planner takes its defaults, as of a program
that answers no statistics. The example's FILES counts each column's
distinct values, so the planner estimates an equality on a column of
four values over 64 rows at 16.

```sql
CREATE TABLE F (K INTEGER, V INTEGER) USING FILES;
INSERT INTO F SELECT X.B, 10 * X.A + Y.A FROM T AS X, T AS Y;
COMMIT;
EXPLAIN SELECT V FROM F WHERE K = 1;
```

```
OK
OK
OK
 PLAN
-----------------------------------------
 -- bound:
 Project F.V
   Filter F.K = 1
     Scan ANN.F
 -- rules: EXAMPLE.ERRING skipped
 -- optimized:
 Project F.V
   Filter F.K = 1
     Scan ANN.F
 -- physical:
 Project F.V
   Filter F.K = 1  (est. rows=16)
     SeqScan ANN.F  (rows=64, cost=64.0)
(13 rows)
```

An index method's distinct keys estimate an equality on all its
columns, and its pages a probe's descent.

## Statistics a plug-in asks

While a call runs, its program may send STATISTICS, type 11, and the
database answers STATISTICSDONE, type 12, or ERROR.

```text
STATISTICS      call u64 | u8 1 table | name str
                         | u8 2 column | table name str | column name str
                         | u8 3 index | name str
STATISTICSDONE  call u64 | table statistics, of a table or a column, or index statistics
```

A table is written SCHEMA.TABLE. A column's answer is its table's, with
that column alone. The answer is what the newest state of the database
keeps, the statistics the planner reads. A table that was not analyzed,
or whose engine keeps none, answers none. A procedure or table function
asks by its user's privileges, checked by a query of the table that
reads no row. Another kind runs as the server, so it may ask of any
table. A question of a table there is not, or one the user may not
read, is answered ERROR. An optimizer rule needs to ask nothing, as each
scan of its plan form holds its table's statistics.

The example's SKEW writes the rows and distinct values the database
keeps of a column into SEEN.

```sql
CREATE TABLE SEEN (N DOUBLE PRECISION, D DOUBLE PRECISION);
COMMIT;
CALL SKEW('ANN.F', 'K');
SELECT * FROM SEEN;
```

```
OK
OK
OK
                        N |                        D
--------------------------+--------------------------
                    6.4E1 |                      4E0
(1 row, with a warning: SQLSTATE 01T02)
```

## The SDK

`Rule::rewrite(cx, form)` gives the form rewritten, or none
(`Plugin::rule`). The form's types are `turborel_plugin::plan`, and its
reading and writing are the SDK's. `Engine::statistics(locator)` and
`IndexMethod::statistics(locator)` answer the planner, a `TableAnswer`
or an `IndexAnswer`, and answer none unless written. A call asks the
database with `Cx::table_statistics`, `Cx::column_statistics` and
`Cx::index_statistics`.
