# Partitioned Tables *(extension)*

[Contents](README.md)

A partitioned table holds no rows of its own. Its rows are kept in its
partitions, each a table of the same columns, and each row in the one
partition whose bound holds the row's key. A statement names the
partitioned table and reads and writes its partitions. A read reads
only the partitions its conditions may find rows in. A partition may be
partitioned again, and a table may be attached as a partition or
detached to stand alone. The examples here make a table of sales,
partitioned by the day of each sale:

```sql
CREATE TABLE SALES (ID INTEGER NOT NULL, SOLD DATE NOT NULL, AMOUNT DECIMAL(9,2),
    PRIMARY KEY (ID, SOLD)) PARTITION BY RANGE (SOLD);
CREATE TABLE S2025 PARTITION OF SALES FOR VALUES FROM (DATE '2025-01-01') TO (DATE '2026-01-01');
CREATE TABLE S2026 PARTITION OF SALES FOR VALUES FROM (DATE '2026-01-01') TO (DATE '2027-01-01');
CREATE TABLE SOTHER PARTITION OF SALES DEFAULT;
INSERT INTO SALES VALUES (1, DATE '2025-03-01', 10), (2, DATE '2026-03-01', 20), (3, DATE '2030-01-01', 30);
```

```
OK
OK
OK
OK
OK
```

## Syntax

```
CREATE TABLE <table name> <table contents> PARTITION BY <strategy> ( <key part> [ , <key part> ]... )

CREATE TABLE <table name> PARTITION OF <table name> <partition bound>
  [ PARTITION BY <strategy> ( <key part> [ , <key part> ]... ) ] [ USING <engine name> ]

<key part> ::= <column name> | ( <value expression> )

<partition bound> ::= DEFAULT
  | FOR VALUES FROM ( <range datum list> ) TO ( <range datum list> )
  | FOR VALUES IN ( <list value> [ , <list value> ]... )
  | FOR VALUES WITH ( <option name> <literal> [ , <option name> <literal> ]... )

<range datum> ::= <literal> | MINVALUE | MAXVALUE

ALTER TABLE <table name> ATTACH PARTITION <table name> <partition bound>
ALTER TABLE <table name> DETACH PARTITION <table name>
```

PARTITION BY may follow WITH SYSTEM VERSIONING and ON COMMIT too. A
partition takes its columns, their defaults, its CHECK constraints and
its periods from the table it is a partition of, and names none of its
own. A key part that is an expression is written in parentheses and
must be deterministic and raise no exception. A key part may not be of a
collated character string type.

## Strategies

A strategy is how a partitioned table's rows are divided, and its name
is what PARTITION BY gives. Three are built in, and an embedding program
may register more ([Embedding](../client/03-embedding.md)).

| Strategy | Bound | A row belongs to the partition |
|---|---|---|
| RANGE | FROM (...) TO (...) | whose bound's key is at or below its key, compared part by part, and whose bound's TO key is above it. MINVALUE is below every value and MAXVALUE above. |
| LIST | IN (...) | that lists its key. NULL may be listed. A key of several parts is listed as a row value. |
| HASH | WITH (MODULUS m, REMAINDER r) | where the hash of its key, divided by m, leaves r. |

The bounds of two partitions of one table may not both hold a key. One
partition of a RANGE or LIST partitioned table may be DEFAULT, which
holds every row no other partition holds. A HASH partitioned table has
no DEFAULT. Its moduli divide each other, so the remainders of every
partition cover each hash once when each modulus present is used.

## Writing rows

An INSERT, an UPDATE or a MERGE of the partitioned table routes each row
down its tree to the partition whose bound holds it. A row no partition
holds fails the statement with SQLSTATE 23T01. An UPDATE or MERGE that
changes a row's key so another partition holds it deletes the row from
its partition and inserts it in the other. A row written to a partition
by name must be one its bound, and the bound of each table above it,
holds.

```sql
UPDATE SALES SET SOLD = DATE '2025-06-01' WHERE ID = 3;
SELECT ID FROM S2025 ORDER BY ID;
INSERT INTO S2026 VALUES (4, DATE '2025-01-01', 40);
```

```
OK
           ID
--------------
            1
            3
(2 rows)
turborel: SQLCODE -201 (SQLSTATE 23T01): the partition ANN.S2026 does not hold the row, by its bound (RULES.md D155)
```

A cursor over a partitioned table is updatable. A positioned UPDATE or
DELETE changes the row the cursor is on in its partition, and a
positioned UPDATE that changes the row's key moves it to the partition
that holds the new key.

A row trigger of a partitioned table fires for the rows of its
partitions, written through it or to a partition. A row an UPDATE moves
is an update of the tables both partitions are under, a deletion from
the tables only its old partition is under and an insertion into those
only its new one is under.

## Reading rows

A read of a partitioned table reads the partitions its conditions may
find rows in, by the comparisons of its key with constants, IS NULL, IN
and OR of equalities. Each partition is read by its own indexes and
statistics. EXPLAIN shows the partitions read. A prepared statement is
planned with its parameters' values each time it runs, so it reads the
partitions they need.

```sql
EXPLAIN SELECT ID FROM SALES WHERE SOLD >= DATE '2026-01-01' AND SOLD < DATE '2026-07-01';
```

```
 PLAN
--------------------------------------------------------------
 -- bound:
 Project SALES.ID
   Filter SALES.SOLD >= DATE '2026-01-01' AND SALES.SOLD < DATE '2026-07-01'
     Scan ANN.SALES
 -- rules: none
 -- optimized:
 Project SALES.ID
   Filter SALES.SOLD >= DATE '2026-01-01' AND SALES.SOLD < DATE '2026-07-01'
     Scan ANN.SALES
 -- physical:
 Project SALES.ID
   Append
     Filter SALES.SOLD >= DATE '2026-01-01' AND SALES.SOLD < DATE '2026-07-01'  (est. rows=0)
       SeqScan ANN.S2026  (rows=1, cost=0.9)
     Filter SALES.SOLD >= DATE '2026-01-01' AND SALES.SOLD < DATE '2026-07-01'  (est. rows=0)
       SeqScan ANN.SOTHER  (rows=0, cost=0.0)
(16 rows)
```

A read in an order, such as ORDER BY with FETCH FIRST, reads each
partition in that order, by an index or a sort of its own, and merges
them, so it reads no more of each than the rows it gives need. A nearest
read of a vector, spatial or full-text index reads each partition's
index for its first rows, and the first rows of all are taken by the
exact distance of each.

A join that finds the rows of a partitioned table by an index, each row
of its other side probing it, probes the index each partition has of
the table's index, or a global index of the table (see Keys and
constraints). Each probe is routed by its values to the partitions that
may hold its rows, so a join on the partition key reads one partition's
index for each row of the other side. EXPLAIN shows the index's method
followed by "each partition's, each probe routed to one", or by how
many partitions each probe reads. A subquery that reads a partitioned
table with the values of an outer row reads the partitions they need.

## Keys and constraints

A PRIMARY KEY or UNIQUE constraint of a partitioned table holds across
all its partitions. Where the key holds every column of the partition
key, at each level, each partition's unique index enforces it. Otherwise
the table has a global index of the key, one index over the rows of
every partition, which each row written is checked in. ATTACH adds the
rows attached to it, and a key another partition holds fails the
statement. DETACH takes the rows detached out of it. A query whose
equalities fix the key reads the global index, one probe, rather than
each partition's index. A FOREIGN KEY may reference a
partitioned table and a partitioned table may have one. A partition
under a table a foreign key references is neither dropped nor detached.

```sql
INSERT INTO SALES VALUES (1, DATE '2025-03-01', 99);
```

```
turborel: SQLCODE -201 (SQLSTATE 23000): UNIQUE (ID, SOLD) of ANN.S2025 would be violated: a row of these values is there [X3.135-1986 §6.4 GR2]
```

```sql
ALTER TABLE SALES ADD CONSTRAINT ONE_ID UNIQUE (ID);
INSERT INTO SALES VALUES (1, DATE '2026-03-01', 5);
```

```
OK
turborel: SQLCODE -201 (SQLSTATE 23000): UNIQUE (ID) of ANN.S2026 would be violated: a row of these values is there [X3.135-1986 §6.4 GR2]
```

## Indexes

CREATE INDEX of a partitioned table makes an index of the same method,
options and key on each of its partitions, present and future. Every
method may be used this way. A template index is named in the CREATE
INDEX statement, and each partition's index is named by the template,
the partition's schema and its name, apart by a character no name holds,
so no two names meet. DROP INDEX and ALTER INDEX of the template drop or
alter each index it made. A table attached keeps an index of its own of
the template's definition as the template's, else one is built. A
partition detached keeps its indexes, renamed to the template's name
and its own after an underscore.

A full-text index of a partitioned table ranks a row by the statistics
of every partition's index together, as an index of the table not
partitioned would rank it. A hybrid index of a MODEL on a partitioned
table makes its full-text part on each partition and keeps one table of
chunks of the rows of every partition, and HYBRID_SEARCH reads them
all.

```sql
COMMIT;
CREATE INDEX SALES_AMOUNT ON SALES (AMOUNT);
SELECT PARTITION_NAME, METHOD_NAME FROM INFORMATION_SCHEMA.PARTITION_INDEXES
    WHERE TEMPLATE_NAME = 'SALES_AMOUNT' ORDER BY PARTITION_NAME;
```

```
OK
OK
 PARTITION_NAME                                               | METHOD_NAME
--------------------------------------------------------------+--------------------------------------------------------------
 S2025                                                        | btree
 S2026                                                        | btree
 SOTHER                                                       | btree
(3 rows)
```

## Attaching and detaching

ATTACH PARTITION makes a table of the same columns, in order, with the
same names, types, collations and nullability, a partition of the
partitioned table. Each of its rows must be one the bound holds, or the
statement fails with SQLSTATE 23T01. Both tables are of one schema, both
temporary of one kind or neither, and both system-versioned or neither.
DETACH PARTITION makes a partition a table of its own, its rows its own.

```sql
CREATE TABLE OLD (ID INTEGER NOT NULL, SOLD DATE NOT NULL, AMOUNT DECIMAL(9,2));
INSERT INTO OLD VALUES (9, DATE '2024-05-01', 90);
ALTER TABLE SALES ATTACH PARTITION OLD FOR VALUES FROM (DATE '2024-01-01') TO (DATE '2025-01-01');
SELECT COUNT(*) FROM SALES;
ALTER TABLE SALES DETACH PARTITION OLD;
SELECT COUNT(*) FROM SALES;
```

```
OK
OK
OK

----------------------
                    4
(1 row)
OK

----------------------
                    3
(1 row)
```

## Other tables

A partitioned table may be a global temporary table, its partitions of
the same kind. It may be system-versioned, each partition keeping its
historical rows in a history of its own, which FOR SYSTEM_TIME reads
together. Each partition chooses its engine, the column store among
them. A typed table and a node or edge table are not partitioned, as a
typed table's rows are its subtables' too and a node or edge table's are
its graph's. The columns and periods of a table of a tree are not
altered, and a partitioned table is truncated and given storage options
by its partitions.

A view of a partitioned table reads its partitions, pruned by the view's
and the query's conditions. A privilege on a partitioned table is a
privilege on its rows read through it, not on its partitions.

## The Information Schema

INFORMATION_SCHEMA.PARTITIONED_TABLES shows each partitioned table, its
strategy, its key and how many partitions it has. PARTITIONS shows each
partition, the table it is a partition of, its bound and its level.
PARTITION_INDEXES shows each index a template made
([The Information Schema](07-information-schema.md)).

```sql
SELECT TABLE_NAME, PARENT_TABLE_NAME, BOUND FROM INFORMATION_SCHEMA.PARTITIONS ORDER BY TABLE_NAME;
```

```
 TABLE_NAME                                                   | PARENT_TABLE_NAME                                            | BOUND
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 S2025                                                        | SALES                                                        | FOR VALUES FROM (DATE '2025-01-01') TO (DATE '2026-01-01')
 S2026                                                        | SALES                                                        | FOR VALUES FROM (DATE '2026-01-01') TO (DATE '2027-01-01')
 SOTHER                                                       | SALES                                                        | DEFAULT
(3 rows)
```

## Errors

| SQLSTATE | SQLCODE | Cause |
|---|---|---|
| 23T01 | -201 | No partition holds a row written to a partitioned table, or a row written to a partition or attached is not one its bound holds |
| 2A000 | -103 | A bound overlaps another's, a second DEFAULT, a DEFAULT of HASH, a key of a column not there, a strategy not registered, a table attached not of the partitioned table's columns, schema, kind or periods, or an ALTER TABLE of a tree's columns |
