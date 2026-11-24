# CREATE REACHABILITY INDEX

[Statements](README.md)

## NAME

CREATE REACHABILITY INDEX: keep which vertices a property graph's edge table leads to *(extension)*

## SYNOPSIS

```
<create reachability index statement> ::=
    CREATE REACHABILITY INDEX [ <index name> ] ON GRAPH <graph name>
    EDGE TABLE <element table alias>
```

## DESCRIPTION

CREATE REACHABILITY INDEX makes an index of the `reachability` method on
the table of an edge table of a property graph, named by its alias in
the graph. Its key is the source key's columns, then the destination
key's. It keeps, of each vertex, labels that say which vertices the
table's edges lead to from it in any number of steps
([CREATE INDEX](create-index.md)). A GRAPH_TABLE search reads it to
drop the starts that cannot reach the end of a match, so it never walks
from them.

A path pattern is pruned so when it has a quantifier, every edge of a
match is of the indexed table, followed one way, between vertices of
one vertex table that both ends of the edge table reference by the same
columns, and its last node pattern has a WHERE of its own variable alone.
Such a WHERE is tested of each vertex of that table before the search,
and a start of the table is kept if it is one of the vertices kept or
reaches one. The search then finds the matches it finds without the
index. EXPLAIN shows `starts pruned by reachability index NAME` for each
pattern pruned.

An index written without a name is named after the graph and the alias,
`GRAPH_ALIAS_REACHABILITY`. It is the [CREATE INDEX](create-index.md) of
the `reachability` method with REACHABILITY.SOURCE_COLUMNS set to the
source key's column count.

## RULES

1. The session has no transaction open: COMMIT or ROLLBACK first.
2. The graph exists, and the alias is that of one of its edge tables
   (GRF-0012).
3. The edge table is a base table, and its user owns the table's schema.
4. The source key's columns and the destination key's are as many and of
   the same types, in order, and none has a collation.
5. No index has the name.

## PRIVILEGES

The table's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 42000 | -102 | EDGE TABLE does not follow the graph's name |
| 42000 | -103 | A rule above is broken |
| 42000 | -104 | Its user does not own the table's schema |
| 0A000 | -900 | The ends' columns differ in number or type, or have a collation |

## EXAMPLES

```sql
CREATE TABLE STOP (NAME VARCHAR(10) PRIMARY KEY, FARE_ZONE INTEGER NOT NULL);
CREATE TABLE LEG (ID INTEGER PRIMARY KEY, A VARCHAR(10) NOT NULL REFERENCES STOP,
    B VARCHAR(10) NOT NULL REFERENCES STOP);
INSERT INTO STOP VALUES ('Ash', 1), ('Birch', 1), ('Cedar', 2), ('Dune', 3), ('Elm', 3);
INSERT INTO LEG VALUES (1, 'Ash', 'Birch'), (2, 'Birch', 'Cedar'), (3, 'Dune', 'Ash'), (4, 'Cedar', 'Elm');
CREATE PROPERTY GRAPH TRANSIT VERTEX TABLES (STOP) EDGE TABLES (LEG
    SOURCE KEY (A) REFERENCES STOP (NAME) DESTINATION KEY (B) REFERENCES STOP (NAME));
COMMIT;
CREATE REACHABILITY INDEX ON GRAPH TRANSIT EDGE TABLE LEG;
SELECT S, T, N FROM GRAPH_TABLE (TRANSIT
    MATCH ANY SHORTEST (s)-[l:Leg]->+(t WHERE t.FARE_ZONE = 2)
    COLUMNS (s.NAME AS S, t.NAME AS T, COUNT(l.*) AS N)) ORDER BY S;
COMMIT;
```

```
OK
OK
OK
OK
OK
OK
OK
 S          | T          |                    N
------------+------------+----------------------
 Ash        | Cedar      |                    2
 Birch      | Cedar      |                    1
 Dune       | Cedar      |                    3
(3 rows)
OK
```

The one stop of zone 2 is Cedar. The search is not run from Elm, which
the index says reaches no stop of zone 2; its EXPLAIN shows
`path 1: starts pruned by reachability index TRANSIT_LEG_REACHABILITY`.
Cedar is kept as a start, being one of the ends, and finds no path of
one leg or more back to itself.

## STANDARD

The property graph implementation standard has no such statement: it
is an extension (RULES.md D150).

## SEE ALSO

[CREATE ADJACENCY INDEX](create-adjacency-index.md),
[CREATE INDEX](create-index.md),
[CREATE LABEL INDEX](create-label-index.md),
[CREATE PROPERTY GRAPH](create-property-graph.md),
[DROP INDEX](drop-index.md)
