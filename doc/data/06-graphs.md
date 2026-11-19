# Graphs

[Specialized Data](README.md)

A bank keeps its accounts and the transfers between them in two tables,
and wants to follow money as it moves: who sent to whom, along which
paths, and by the shortest or cheapest. This guide presents the tables
as a property graph, matches patterns of accounts and transfers with
GRAPH_TABLE, finds paths of several transfers, and indexes the
transfers so the search follows them quickly. The
[reference](../sql/20-property-graphs.md) gives every rule.

## The accounts and transfers

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE ACCOUNT (ID VARCHAR(4) PRIMARY KEY, OWNER VARCHAR(20), BLOCKED BOOLEAN NOT NULL);
CREATE TABLE TRANSFER (ID VARCHAR(4) PRIMARY KEY,
    SRC VARCHAR(4) NOT NULL REFERENCES ACCOUNT, DST VARCHAR(4) NOT NULL REFERENCES ACCOUNT,
    AMOUNT INTEGER NOT NULL);
INSERT INTO ACCOUNT VALUES ('a1', 'Scott', FALSE), ('a2', 'Aretha', FALSE),
    ('a3', 'Mike', FALSE), ('a4', 'Jay', TRUE), ('a5', 'Dave', FALSE);
INSERT INTO TRANSFER VALUES ('t1', 'a1', 'a3', 8), ('t2', 'a3', 'a2', 10),
    ('t3', 'a2', 'a4', 10), ('t4', 'a4', 'a5', 10), ('t5', 'a5', 'a3', 10),
    ('t6', 'a3', 'a5', 6);
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

## The graph

The accounts are the vertices and the transfers the edges, each from
the account its SRC names to the one its DST names. The keys are the
tables' primary keys, and the ends their foreign keys:

```sql
CREATE PROPERTY GRAPH BANK
    VERTEX TABLES (ACCOUNT PROPERTIES (ID, OWNER, BLOCKED))
    EDGE TABLES (TRANSFER
        SOURCE KEY (SRC) REFERENCES ACCOUNT (ID)
        DESTINATION KEY (DST) REFERENCES ACCOUNT (ID)
        PROPERTIES (ID, AMOUNT));
COMMIT;
SELECT INDEX_NAME FROM INFORMATION_SCHEMA.INDEXES WHERE METHOD_NAME = 'adjacency';
```

```
OK
OK
 INDEX_NAME
--------------------------------------------------------------
 BANK_TRANSFER_ADJACENCY
(1 row)
```

The graph is ENFORCED, its keys and ends declared constraints, so as it
committed TRANSFER was given an adjacency index *(extension)*, and a search follows a
transfer from either account without a join.

## Patterns

A pattern names accounts in parentheses and transfers in brackets
between them. Who sent money to a blocked account, and how much:

```sql
SELECT * FROM GRAPH_TABLE (BANK
    MATCH (a)-[t]->(b WHERE b.blocked)
    COLUMNS (a.owner AS SENDER, t.amount AS AMOUNT, b.owner AS BLOCKED_OWNER));
```

```
 SENDER               |       AMOUNT | BLOCKED_OWNER
----------------------+--------------+----------------------
 Aretha               |           10 | Jay
(1 row)
```

## Paths

A quantifier follows several transfers. ANY SHORTEST keeps a path of
fewest transfers from each start to each end, and a set function of the
transfer variable reads the path's transfers in order:

```sql
SELECT * FROM GRAPH_TABLE (BANK
    MATCH ANY SHORTEST (a WHERE a.owner = 'Scott')-[t]->+(b WHERE b.owner = 'Dave')
    COLUMNS (ARRAY_AGG(t.id) AS PATH, SUM(t.amount) AS MOVED));
```

```
 PATH                                                         |                MOVED
--------------------------------------------------------------+----------------------
 ARRAY['t1', 't6']                                            |                   14
(1 row)
```

TRAIL keeps every path that repeats no transfer, here each round trip
from Mike's account back to it, by its transfers' count:

```sql
SELECT * FROM GRAPH_TABLE (BANK
    MATCH TRAIL (a WHERE a.owner = 'Mike')-[t]->+(a)
    COLUMNS (COUNT(t.*) AS N, ARRAY_AGG(t.id) AS LOOP))
  ORDER BY N;
```

```
                    N | LOOP
----------------------+--------------------------------------------------------------
                    2 | ARRAY['t6', 't5']
                    4 | ARRAY['t2', 't3', 't4', 't5']
(2 rows)
```

ANY CHEAPEST *(extension)* keeps the path of least cost, here of the smallest
transfers' sum:

```sql
SELECT * FROM GRAPH_TABLE (BANK
    MATCH ANY CHEAPEST COST t.amount (a WHERE a.owner = 'Mike')-[t]->+(b WHERE b.owner = 'Dave')
    COLUMNS (ARRAY_AGG(t.id) AS PATH, SUM(t.amount) AS COST));
```

```
 PATH                                                         |                 COST
--------------------------------------------------------------+----------------------
 ARRAY['t6']                                                  |                    6
(1 row)
```

## Each account

A GRAPH_TABLE reads the tables before it, so each account may have its
own pattern. The accounts each sends to directly, and those that send
to none, of a LEFT JOIN:

```sql
SELECT A.OWNER, G.TO_OWNER
  FROM ACCOUNT A LEFT JOIN GRAPH_TABLE (BANK
    MATCH (x WHERE x.id = A.ID)-[t]->(y)
    COLUMNS (y.owner AS TO_OWNER)) AS G ON TRUE
  ORDER BY A.OWNER, G.TO_OWNER;
```

```
 OWNER                | TO_OWNER
----------------------+----------------------
 Aretha               | Jay
 Dave                 | Mike
 Jay                  | Dave
 Mike                 | Aretha
 Mike                 | Dave
 Scott                | Mike
(6 rows)
```
