# Property Graphs *(extension)*

[Contents](README.md)

A property graph presents tables as vertices and edges. A vertex table's
rows are vertices, and an edge table's rows are edges, each joining the
vertex its source key names to the one its destination key names. Each
element has labels, and properties its labels give it. GRAPH_TABLE
matches a pattern of vertices and edges in a graph and gives a row of
its COLUMNS for each match, to the query around it. The examples here
read the graph BANK of accounts and the transfers between them.

```sql
CREATE TABLE ACCOUNT (ID VARCHAR(4) PRIMARY KEY, OWNER VARCHAR(20));
CREATE TABLE TRANSFER (ID VARCHAR(4) PRIMARY KEY,
    SRC VARCHAR(4) NOT NULL REFERENCES ACCOUNT, DST VARCHAR(4) NOT NULL REFERENCES ACCOUNT,
    AMOUNT INTEGER NOT NULL);
INSERT INTO ACCOUNT VALUES ('a1', 'Scott'), ('a2', 'Aretha'), ('a3', 'Mike'), ('a4', 'Jay');
INSERT INTO TRANSFER VALUES ('t1', 'a1', 'a3', 8), ('t2', 'a3', 'a2', 10),
    ('t3', 'a2', 'a4', 10), ('t4', 'a4', 'a1', 5), ('t5', 'a3', 'a4', 6);
CREATE PROPERTY GRAPH BANK
    VERTEX TABLES (ACCOUNT LABEL ACCOUNT PROPERTIES (ID, OWNER))
    EDGE TABLES (TRANSFER
        SOURCE KEY (SRC) REFERENCES ACCOUNT (ID)
        DESTINATION KEY (DST) REFERENCES ACCOUNT (ID)
        LABEL TRANSFER PROPERTIES (ID, AMOUNT));
COMMIT;
```

```
OK
OK
OK
OK
OK
OK
```

[CREATE PROPERTY GRAPH](statements/create-property-graph.md) defines a
graph. As the transaction that creates an ENFORCED graph commits, each
of its base edge tables is given an adjacency index
([CREATE ADJACENCY INDEX](statements/create-adjacency-index.md)), so a
search follows its edges without a join.

## Syntax

```
GRAPH_TABLE ( <graph name>
    MATCH [ <match mode> ] <path pattern> [ , <path pattern> ]...
      [ WHERE <search condition> ]
    [ ONE ROW PER MATCH | ONE ROW PER VERTEX ( <variable> )
      | ONE ROW PER STEP ( <variable> , <variable> , <variable> ) ]
    COLUMNS ( { <value expression> [ AS <column name> ] | <variable>.* } [ , ... ] )
    [ OPTIONS ( <option> [ , ... ] ) ] )
  [ FOR SYSTEM_TIME AS OF <value expression> ] [ [ AS ] <correlation name> ]

<match mode> ::= REPEATABLE ELEMENTS | DIFFERENT EDGES | DIFFERENT VERTICES

<path pattern> ::= [ <variable> = ] [ <selector> ] [ <restrictor> ] <path expression>

<selector> ::= ANY | ANY <k> | ANY SHORTEST | ALL SHORTEST | SHORTEST <k> [ GROUPS ]
  | { ANY | ALL | <k> } CHEAPEST [ COST { <property> | ( <value expression> ) } ]

<restrictor> ::= WALK | TRAIL | ACYCLIC | SIMPLE

<option> ::= MAX_PATH_LENGTH <n> | MAX_PATHS_PER_PARTITION <n>
  | MAX_SEARCH_STATES <n> | ON LIMIT { ERROR | TRUNCATE } | DETERMINISTIC_PATHS
```

A path expression is a sequence of vertex patterns `(v :label WHERE
...)` and edge patterns between them: `-[e]->` from left to right,
`<-[e]-` from right to left, `~[e]~` undirected, `-[e]-` either way, and
`-`, `->` and `<-` of an edge with no variable. A part may be quantified
by `*`, `+`, `?` or `{n,m}`, and alternatives are joined by `|`, of
their bindings' union, or `|+|`, each kept.

## Matches

Each match binds each variable to an element. A vertex pattern of no
label matches every vertex, and a label expression of `&`, `|`, `!` and
`%` (any label) narrows it. A WHERE in an element pattern is tested as
the element is bound:

```sql
SELECT * FROM GRAPH_TABLE (BANK
    MATCH (a WHERE a.owner = 'Mike')-[t:Transfer]->(b)
    COLUMNS (t.id AS T, b.owner AS TO_OWNER, t.amount AS AMOUNT))
  ORDER BY T;
```

```
 T    | TO_OWNER             |       AMOUNT
------+----------------------+--------------
 t2   | Aretha               |           10
 t5   | Jay                  |            6
(2 rows)
```

A quantified edge matches paths. A selector keeps some of the paths
between each start and end, and a restrictor keeps paths of no repeated
edge (TRAIL), no repeated vertex (ACYCLIC), or none but the first at the
end (SIMPLE). An unbounded quantifier needs one of them. A set function
of a variable inside the quantifier reads its elements in path order:

```sql
SELECT * FROM GRAPH_TABLE (BANK
    MATCH ANY SHORTEST (a WHERE a.owner = 'Scott')-[t:Transfer]->+(b WHERE b.owner = 'Jay')
    COLUMNS (ARRAY_AGG(t.id) AS PATH, SUM(t.amount) AS TOTAL));
```

```
 PATH                                                         |                TOTAL
--------------------------------------------------------------+----------------------
 ARRAY['t1', 't5']                                            |                   14
(1 row)
```

ANY CHEAPEST keeps a path of least COST, the sum of its expression over
the edges, which may not be negative. Here t5 costs 100 and every other
transfer 1:

```sql
SELECT * FROM GRAPH_TABLE (BANK
    MATCH ANY CHEAPEST COST (CASE WHEN t.id = 't5' THEN 100 ELSE 1 END) (a WHERE a.owner = 'Scott')-[t:Transfer]->+(b WHERE b.owner = 'Jay')
    COLUMNS (ARRAY_AGG(t.id) AS PATH));
```

```
 PATH
--------------------------------------------------------------
 ARRAY['t1', 't2', 't3']
(1 row)
```

A WHERE after the pattern is the postfilter, tested of each whole match.
A match of two path patterns joins them on the variables they share. The
match mode REPEATABLE ELEMENTS, the default, keeps every join, and
DIFFERENT EDGES only those binding no edge twice.

## Functions

| Function | Value |
|---|---|
| `ELEMENT_ID(v)` | the element's identity, the JSON text of its table and key |
| `VERTEX_ID(v)`, `EDGE_ID(e)` | the same as a JSON value |
| `LABELS(v)` | an array of its labels |
| `PROPERTY_EXISTS(v, name)` | whether one of its labels gives it the property |
| `PATH_LENGTH(p)` | the edges of path variable p |
| `VERTICES(p)`, `EDGES(p)` | arrays of their identities, in path order |
| `IS_ACYCLIC(p)`, `IS_TRAIL(p)`, `IS_SIMPLE(p)` | whether the path is so |
| `PATH_TO_JSON(p)` | a JSON array of the identities of its elements |

The predicates are `SAME(a, b)`, `ALL_DIFFERENT(a, b, ...)`, `v IS
SOURCE OF e`, `v IS DESTINATION OF e`, `e IS DIRECTED` and `v IS LABELED
label`, each with NOT. `COUNT(e.*)` counts a group variable's elements:

```sql
SELECT * FROM GRAPH_TABLE (BANK
    MATCH TRAIL p = (a WHERE a.owner = 'Scott')-[t]->+(b WHERE b.owner = 'Scott')
    COLUMNS (PATH_LENGTH(p) AS N, COUNT(t.*) AS C, IS_SIMPLE(p) AS S))
  ORDER BY N;
```

```
            N |                    C | S
--------------+----------------------+-------
            3 |                    3 | TRUE
            4 |                    4 | TRUE
(2 rows)
```

## Rows

ONE ROW PER MATCH, the default, gives a row of each match. ONE ROW PER
VERTEX (v) gives a row of each vertex of its paths, v bound to it, and
ONE ROW PER STEP (v1, e, v2) a row of each edge with the vertices before
and after it:

```sql
SELECT * FROM GRAPH_TABLE (BANK
    MATCH ANY SHORTEST (a WHERE a.owner = 'Scott')-[t]->+(b WHERE b.owner = 'Jay')
    ONE ROW PER STEP (x, e, y)
    COLUMNS (x.owner AS FROM_OWNER, e.id AS T, y.owner AS TO_OWNER))
  ORDER BY T;
```

```
 FROM_OWNER           | T    | TO_OWNER
----------------------+------+----------------------
 Scott                | t1   | Mike
 Mike                 | t5   | Jay
(2 rows)
```

A GRAPH_TABLE may read the columns of the table references before it,
each row its own matches; of a LEFT JOIN, a row of none is kept, null
extended. FOR SYSTEM_TIME AS OF reads every element table as it was
then.

## Limits

A search is bounded by its OPTIONS, and else by the settings GRAPH.*
([Appendix C](../appendix/c-settings.md)). Passing a bound fails with
SQLSTATE 54061, or ON LIMIT TRUNCATE ends the search with the paths it
found and warning 01W61:

```sql
SELECT COUNT(*) AS PATHS FROM GRAPH_TABLE (BANK
    MATCH TRAIL (a WHERE a.owner = 'Scott')-[t]->*(b)
    COLUMNS (b.id AS B)
    OPTIONS (MAX_PATH_LENGTH 1));
```

```
turborel: SQLCODE -900 (SQLSTATE 54061): [GS §20] GRF-0061: the path search passed MAX_PATH_LENGTH, 1
```

## Plans

A pattern of no quantifier is a join of its element tables, which the
planner orders by its estimates like any other join. Each edge is read
through its adjacency index from the vertex bound before it, shown as
Expand, or from its destination, shown as Expand (reverse), or between
two bound vertices, shown as ExpandInto. An edge table of no adjacency
index is read whole once in each join, so a pattern needing more than
four such reads is searched instead:

```sql
SELECT * FROM GRAPH_TABLE (BANK
    MATCH (a WHERE a.owner = 'Scott')-[t]->(b)
    COLUMNS (b.owner AS TO_WHOM, t.amount AS AMOUNT));
EXPLAIN SELECT * FROM GRAPH_TABLE (BANK
    MATCH (a WHERE a.owner = 'Mike')<-[t]-(b)
    COLUMNS (b.owner AS FROM_WHOM));
```

```
 TO_WHOM              |       AMOUNT
----------------------+--------------
 Mike                 |            8
(1 row)
 PLAN
--------------------------------------------------------------
 -- bound:
 Project OWNER
   Project OWNER
     Filter SRC = ID AND DST = ID AND OWNER = 'Mike'
       Product
         Product
           Scan ANN.ACCOUNT
           Scan ANN.TRANSFER
         Scan ANN.ACCOUNT
 -- rules: push_filter_into_product x2
 -- optimized:
 Project OWNER
   Project OWNER
     Filter SRC = ID
       Product
         Filter DST = ID
           Product
             Filter OWNER = 'Mike'
               Scan ANN.ACCOUNT
             Scan ANN.TRANSFER
         Scan ANN.ACCOUNT
 -- physical:
 Project OWNER
   IndexNestedLoop ANN.ACCOUNT using ANN.ACCOUNT.UNIQUE1 [btree]: SRC = ID  (est. rows each=1, cost=0.6)
     Expand (reverse) ANN.TRANSFER using BANK_TRANSFER_ADJACENCY [adjacency]: ID = DST  (est. rows each=0, cost=0.5)
       Filter OWNER = 'Mike'  (est. rows=0)
         SeqScan ANN.ACCOUNT  (rows=4, cost=4.0)
(27 rows)
```

A pattern whose edges close a cycle is matched by a worst-case optimal
join: each next vertex is one of those every bound vertex tied to it
reaches, the intersection of their neighbour lists. The hint NO_WCOJ
joins it pairwise instead:

```sql
SELECT * FROM GRAPH_TABLE (BANK
    MATCH (a)-[r]->(b)-[s]->(c)-[t]->(a) WHERE a.id < b.id AND a.id < c.id
    COLUMNS (a.owner AS A, b.owner AS B, c.owner AS C));
SELECT COUNT(*) AS TRIANGLES FROM GRAPH_TABLE (BANK
    MATCH (a)-[r]->(b)-[s]->(c)-[t]->(a)
    COLUMNS (a.id AS A) OPTIONS (NO_WCOJ));
```

```
 A                    | B                    | C
----------------------+----------------------+----------------------
 Scott                | Mike                 | Jay
(1 row)
            TRIANGLES
----------------------
                    3
(1 row)
```

A quantifier of at most three iterations is unrolled into a join for
each count of them, and a path pattern written as an earlier one is
searched once. GRAPH_START, GRAPH_DIRECTION, USE_ADJACENCY and
PATH_ALGO direct a search, so a pattern written with one is searched:

```sql
SELECT * FROM GRAPH_TABLE (BANK
    MATCH (a WHERE a.owner = 'Scott')-[]->{1,2}(b)
    COLUMNS (b.owner AS REACHED));
SELECT COUNT(*) AS PATHS FROM GRAPH_TABLE (BANK
    MATCH (a WHERE a.owner = 'Scott')-[]->{1,2}(b)
    COLUMNS (b.owner AS REACHED) OPTIONS (PATH_ALGO (DFS)));
```

```
 REACHED
----------------------
 Mike
 Aretha
 Jay
(3 rows)
                PATHS
----------------------
                    3
(1 row)
```

A search reads the edges of 1,024 states of its frontier at a time,
each edge table's lists in one pass of its index. Of ANY SHORTEST or
ANY from many starts, it searches from up to 64 starts at once (MS-BFS)
when that many starts would together reach about every vertex, judged by
the tables' sizes and the average degree of the edges the pattern's
labels match, once the graph's statistics count them, and from each
start alone otherwise. It runs on the workers EXECUTOR.GRAPH_THREADS allows once its
estimated states reach GRAPH.PARALLEL_MIN_STATES: in morsels of starts,
or level by level from one start. PATH_ALGO picks the algorithm within
the family the selector calls for, BFS or MSBFS of a shortest or ANY
selector, DFS of every path, DIJKSTRA of CHEAPEST, and a hint that
cannot answer the pattern is refused.

A label index ([CREATE LABEL INDEX](statements/create-label-index.md))
answers a pattern's label tests and finds its starts without reading
rows. A reachability index
([CREATE REACHABILITY INDEX](statements/create-reachability-index.md))
drops the starts of a quantified pattern that reach none of its ends.

A graph's statistics ([GRAPH_STATISTICS](07-information-schema.md#graph-statistics))
come from its tables' and give each edge table's degree. A join plan
estimates the rows of each Expand by that average degree. A quantified
pattern with both ends filtered is searched from the end of fewer
candidates times fan-out, and its paths are turned back, but only where
the answer is the same either way. GRAPH_START or GRAPH_DIRECTION
(e, REVERSE) names that end instead.

## EXPLAIN

EXPLAIN shows each path pattern's automaton, the search chosen, and
whether each edge table is followed through an adjacency index or by a
scan.
