# Graph Algorithms

[Contents](README.md)

The functions of the schema GRAPH_ALGO run graph algorithms over a table
of edges *(extension)*. Each is a table reference of the FROM clause, a
row for each vertex and its value, so its result joins to the table the
vertices' keys are of. INTO PROPERTY writes a result to a column of that
table instead. The examples here make the tables of a small graph, the
people P and who follows whom, F:

```sql
CREATE TABLE P (ID INTEGER PRIMARY KEY, NAME VARCHAR(10));
INSERT INTO P VALUES (1, 'ann'), (2, 'bob'), (3, 'cy'), (4, 'dee'), (5, 'eve');
CREATE TABLE F (WHO INTEGER, WHOM INTEGER, COST DOUBLE PRECISION);
INSERT INTO F VALUES (1, 2, 1), (2, 3, 2), (3, 1, 1), (3, 4, 5), (1, 4, 10);
```

```
OK
OK
OK
OK
```

## Syntax

```
GRAPH_ALGO . <algorithm> ( <edges> [ , <argument> ]... )
  [ [ AS ] <correlation name> [ ( <derived column list> ) ] ]

<edges> ::= TABLE ( <table name> ) | TABLE ( <query expression> )
  | <property graph name>
<argument> ::= <value> | <parameter name> => <value>
```

The invocation may be written TABLE ( GRAPH_ALGO . ... ) too, as a
polymorphic table function's is. The edges are a table argument: the
first two columns of each row are an edge's source and destination, the
keys of its vertices. A row of a null key is no edge. The keys of every
table argument are of types that compare, and the result's vertex
columns are of the type of the edges' source.

The edges may be a property graph's name instead, of CREATE PROPERTY
GRAPH or a schema's SYS_GRAPH of its node and edge tables. Its vertex
tables' rows are then the vertices, and VERTICES is not written. Each
edge table's rows are the edges, each joining the vertices whose
columns its endpoints reference, so an edge of no such vertex is none,
and an undirected edge joins them both ways. COST may name a column
that every edge table has alike. A graph is read through its tables,
with the statement's privileges on them.

Of such a graph, a vertex is its identity, as ELEMENT_ID gives it: the
text `{"table":"ACCOUNT","key":[6]}` of its element table's name and its
key's values in order, of VARCHAR(8192). So keys of two vertex tables,
and keys of any number of columns, name vertices apart, and START is an
identity too. Of a SYS_GRAPH, a vertex is its `$NODE_ID`, the identity
its node table holds.

Arguments follow the edges in the order of the parameters, then by
name. Once one is named, the rest are too. An argument not written, or
NULL, takes its parameter's default. Without a correlation name a result
is named by its algorithm.

## The algorithms

| Algorithm | Parameters after EDGES | Columns of the result |
|---|---|---|
| PAGERANK | DAMPING 0.85, ITERATIONS 20, TOLERANCE 1E-6 | VERTEX, RANK |
| WCC | | VERTEX, COMPONENT |
| SCC | | VERTEX, COMPONENT |
| BFS_LEVELS | START | VERTEX, DEPTH |
| SSSP | START, COST | VERTEX, DISTANCE |
| TRIANGLE_COUNT | | VERTEX, TRIANGLES |
| LOCAL_CLUSTERING | | VERTEX, COEFFICIENT |
| BETWEENNESS | SAMPLE, SEED 0 | VERTEX, CENTRALITY |
| LOUVAIN | RESOLUTION 1, LEVELS 10, TOLERANCE 1E-7, SEED 0, COST | VERTEX, COMMUNITY |
| LABEL_PROPAGATION | ITERATIONS 10 | VERTEX, COMMUNITY |
| K_CORE | | VERTEX, CORE |
| JACCARD_SIMILARITY | TOP_K, THRESHOLD 0 | VERTEX1, VERTEX2, SIMILARITY |

Each takes VERTICES last, a table argument whose first column holds
keys of vertices to count though no edge has them.

PAGERANK gives each vertex its rank, the ranks summing to 1, the rank of
a vertex of no outgoing edge shared by all. It stops after ITERATIONS,
or sooner once the ranks move less than TOLERANCE in all. WCC and SCC
give each vertex its weakly or strongly connected component, named by
the least key in it.

BFS_LEVELS gives each vertex START reaches the least number of edges
from it. SSSP gives the least sum of the edges' COST, the name of a
column of the edges, or of edges counted as 1 without one. A negative
cost is taken, but not a cycle of them START reaches. A vertex not
reached has no row, nor does any if START is no vertex.

TRIANGLE_COUNT, LOCAL_CLUSTERING, LOUVAIN, LABEL_PROPAGATION, K_CORE and
JACCARD_SIMILARITY read the edges with no direction, each pair of
vertices joined once. TRIANGLE_COUNT gives each vertex the triangles it
is in, and LOCAL_CLUSTERING the share of pairs of its neighbours that
are joined. K_CORE gives each vertex the greatest k of a subgraph it is
in whose every vertex has k neighbours in it.

BETWEENNESS gives each vertex the share it is on of the shortest paths
between other vertices. With SAMPLE it starts from that many vertices,
drawn by SEED, and scales the sums up to estimate it.

LOUVAIN finds communities that raise the modularity at RESOLUTION,
over LEVELS levels of communities merged, with COST as the edges'
weights. LABEL_PROPAGATION finds them by each vertex taking the label
most of its neighbours have. Each names a community by the least key in
it, and each is the same on every run of its SEED.

JACCARD_SIMILARITY gives pairs of vertices that share neighbours and
the share of their neighbours both have, those of at least THRESHOLD.
With TOP_K it gives each vertex's TOP_K pairs, of both orders, and
without it each pair once, its lesser key first.

## Examples

```sql
SELECT P.NAME, R.COMPONENT
  FROM P JOIN GRAPH_ALGO.SCC(TABLE (F)) AS R ON P.ID = R.VERTEX
  ORDER BY P.NAME;
```

```
 NAME       |    COMPONENT
------------+--------------
 ann        |            1
 bob        |            1
 cy         |            1
 dee        |            4
(4 rows)
```

```sql
SELECT * FROM GRAPH_ALGO.SSSP(TABLE (F), 1, COST => 'COST') AS D ORDER BY VERTEX;
```

```
       VERTEX |                 DISTANCE
--------------+--------------------------
            1 |                      0E0
            2 |                      1E0
            3 |                      3E0
            4 |                      8E0
(4 rows)
```

```sql
SELECT * FROM GRAPH_ALGO.WCC(TABLE (F), VERTICES => TABLE (P)) AS W ORDER BY VERTEX;
```

```
       VERTEX |    COMPONENT
--------------+--------------
            1 |            1
            2 |            1
            3 |            1
            4 |            1
            5 |            5
(5 rows)
```

```sql
SELECT VERTEX, CAST(RANK * 1000 AS INTEGER) AS PERMILLE
  FROM GRAPH_ALGO.PAGERANK(TABLE (F), DAMPING => 0.85, ITERATIONS => 50) AS R
  ORDER BY VERTEX;
```

```
       VERTEX |     PERMILLE
--------------+--------------
            1 |          219
            2 |          197
            3 |          271
            4 |          312
(4 rows)
```

The edges are any query, so a subgraph is a WHERE:

```sql
SELECT * FROM GRAPH_ALGO.TRIANGLE_COUNT(TABLE (SELECT WHO, WHOM FROM F WHERE COST < 10)) AS T
  ORDER BY VERTEX;
```

```
       VERTEX |            TRIANGLES
--------------+----------------------
            1 |                    1
            2 |                    1
            3 |                    1
            4 |                    0
(4 rows)
```

A property graph's vertices are named by their identities, so a graph
of several vertex tables, here accounts and the cities they are in,
keeps them apart:

```sql
CREATE TABLE CITY (CID INTEGER PRIMARY KEY, CNAME VARCHAR(10));
CREATE TABLE LIVES (WHO INTEGER PRIMARY KEY, CID INTEGER);
INSERT INTO CITY VALUES (1, 'oslo'), (2, 'rome');
INSERT INTO LIVES VALUES (1, 1), (2, 1), (4, 2);
CREATE PROPERTY GRAPH SOCIAL
  VERTEX TABLES (P, CITY)
  EDGE TABLES (
    F KEY (WHO, WHOM) SOURCE KEY (WHO) REFERENCES P (ID) DESTINATION KEY (WHOM) REFERENCES P (ID),
    LIVES SOURCE KEY (WHO) REFERENCES P (ID) DESTINATION KEY (CID) REFERENCES CITY (CID))
  OPTIONS (TRUSTED MODE);
SELECT * FROM GRAPH_ALGO.WCC(SOCIAL) AS W ORDER BY VERTEX;
```

```
OK
OK
OK
OK
OK
 VERTEX                                                       | COMPONENT
--------------------------------------------------------------+--------------------------------------------------------------
 {"table":"CITY","key":[1]}                                   | {"table":"CITY","key":[1]}
 {"table":"CITY","key":[2]}                                   | {"table":"CITY","key":[1]}
 {"table":"P","key":[1]}                                      | {"table":"CITY","key":[1]}
 {"table":"P","key":[2]}                                      | {"table":"CITY","key":[1]}
 {"table":"P","key":[3]}                                      | {"table":"CITY","key":[1]}
 {"table":"P","key":[4]}                                      | {"table":"CITY","key":[1]}
 {"table":"P","key":[5]}                                      | {"table":"P","key":[5]}
(7 rows)
```

A schema's node and edge tables are its graph SYS_GRAPH, each vertex
its `$NODE_ID`, so a result joins to the node tables by it:

```sql
CREATE TABLE TOWN (NAME VARCHAR(10)) AS NODE;
CREATE TABLE ROAD (KM DOUBLE PRECISION) AS EDGE;
INSERT INTO TOWN VALUES ('alba'), ('bra'), ('cuneo');
INSERT INTO ROAD ($FROM_ID, $TO_ID, KM)
  SELECT A.$NODE_ID, B.$NODE_ID, 50 FROM TOWN AS A, TOWN AS B WHERE A.NAME < B.NAME;
SELECT TOWN.NAME, CAST(R.RANK * 1000 AS INTEGER) AS PERMILLE
  FROM GRAPH_ALGO.PAGERANK(SYS_GRAPH) AS R JOIN TOWN ON TOWN.$NODE_ID = R.VERTEX
  ORDER BY TOWN.NAME;
```

```
OK
OK
OK
OK
 NAME       |     PERMILLE
------------+--------------
 alba       |          198
 bra        |          282
 cuneo      |          521
(3 rows)
```

## INTO PROPERTY

```
GRAPH_ALGO . <algorithm> ( <edges> [ , <argument> ]... )
  INTO PROPERTY <column name> OF <table name> [ ( <key column name> ) ]
```

INTO PROPERTY writes each vertex's value to the column of the row of
the table whose key column holds its key. The key column is the table's
primary key of one column unless named. If the table has no column of
the name, it is added first, of DOUBLE PRECISION, BIGINT or the key's
type, as the result's value column is. A row of no vertex keeps the
value it had, null in a column just added. All of it is in the
statement's transaction, so ROLLBACK undoes it. JACCARD_SIMILARITY,
whose result is of pairs, writes no property.

```sql
GRAPH_ALGO.BFS_LEVELS(TABLE (F), 1) INTO PROPERTY HOPS OF P;
SELECT ID, NAME, HOPS FROM P ORDER BY ID;
```

```
OK
           ID | NAME       |                 HOPS
--------------+------------+----------------------
            1 | ann        |                    0
            2 | bob        |                    1
            3 | cy         |                    2
            4 | dee        |                    1
            5 | eve        |                 NULL
(5 rows)
```

## Runs

A run reads its edges in the statement's snapshot, through the
privileges the statement has on them. It runs on as many threads as
EXECUTOR.GRAPH_THREADS says, 1 unless set, and gives the same rows on
any number. Its arrays are held within MEMORY.WORK_BYTES, and one that
needs more is refused with SQLCODE -918. Cancelled, or past the
statement's timeout, it stops with SQLCODE -920.

INFORMATION_SCHEMA.GRAPH_ALGO_RUNS shows the latest 1000 runs, each with
its number, algorithm, arguments, user, start and time taken, its
graph's vertices and edges, its rows, its threads, and DONE or FAILED
with the message. The database's owner sees every run, another user
their own.

```sql
SELECT RUN_ID, ALGORITHM, ARGUMENTS, VERTICES, EDGES, ROWS_RETURNED, STATUS
  FROM INFORMATION_SCHEMA.GRAPH_ALGO_RUNS ORDER BY RUN_ID;
```

```
               RUN_ID | ALGORITHM                                                    | ARGUMENTS                                                    |             VERTICES |                EDGES |        ROWS_RETURNED | STATUS
----------------------+--------------------------------------------------------------+--------------------------------------------------------------+----------------------+----------------------+----------------------+--------
                    1 | SCC                                                          |                                                              |                    4 |                    5 |                    4 | DONE
                    2 | SSSP                                                         | START => 1, COST => 'COST'                                   |                    4 |                    5 |                    4 | DONE
                    3 | WCC                                                          |                                                              |                    5 |                    5 |                    5 | DONE
                    4 | PAGERANK                                                     | DAMPING => 8.5E-1, ITERATIONS => 50                          |                    4 |                    5 |                    4 | DONE
                    5 | TRIANGLE_COUNT                                               |                                                              |                    4 |                    4 |                    4 | DONE
                    6 | WCC                                                          |                                                              |                    7 |                    8 |                    7 | DONE
                    7 | PAGERANK                                                     |                                                              |                    3 |                    3 |                    3 | DONE
                    8 | BFS_LEVELS                                                   | START => 1                                                   |                    4 |                    5 |                    4 | DONE
(8 rows)
```

## Errors

| SQLSTATE | SQLCODE | Cause |
|---|---|---|
| 2A000 | -103 | GRAPH_ALGO has no algorithm of the name, an argument names no parameter, two are of one, one without a name follows a named one, START is missing, the edges are a value that names no property graph, VERTICES is written with a graph, or a key is not of a type that compares with the source's |
| 22000 | -301 | An argument is out of its range, as a DAMPING past 1 |
| 22W34 | -301 | GRF-0034, of a COST that names no column, is null or not a number, is negative where the algorithm takes no negative cost, or makes a negative cycle SSSP's START reaches |
| TB918 | -918 | The run needs more memory than MEMORY.WORK_BYTES |
| TB920 | -920 | The statement was cancelled or ran past its timeout |

## Standard

The functions of GRAPH_ALGO and INTO PROPERTY are the graph
implementation standard's §18, its run log GRAPH_ALGO_RUNS of §20, and
GRF-0034 of §22. Named arguments, `name => value`, are those of
ISO/IEC 9075-2:2023 §10.4, here mixed with positional ones as the graph
standard writes them.
