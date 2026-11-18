# CREATE ADJACENCY INDEX

[Statements](README.md)

## NAME

CREATE ADJACENCY INDEX: index the edges of a property graph's edge table *(extension)*

## SYNOPSIS

```
<create adjacency index statement> ::=
    CREATE ADJACENCY INDEX [ <index name> ] ON GRAPH <graph name>
    EDGE TABLE <element table alias>
    [ DIRECTION { FORWARD | REVERSE | BOTH } ]
    [ INCLUDE ( <column name> [ { , <column name> }... ] ) ]
```

## DESCRIPTION

CREATE ADJACENCY INDEX makes an index of the `adjacency` method on the
table of an edge table of a property graph, named by its alias in the
graph. Its key is the source key's columns, then the destination key's,
then the columns INCLUDE names, so a search follows an edge from either
end without a join ([Property Graphs](../16-property-graphs.md)). It is
the [CREATE INDEX](create-index.md) of that method with the option
ADJACENCY.SOURCE_COLUMNS set to the source key's column count.

DIRECTION FORWARD keeps the edges by their sources alone, REVERSE by
their destinations alone, and BOTH, the default, both ways. An index
written without a name is named after the graph and the alias,
`GRAPH_ALIAS_ADJACENCY`.

As the transaction that creates an ENFORCED graph commits, each of its
base edge tables that has no adjacency index of its ends is given one,
of DIRECTION BOTH, named so.

## RULES

1. The session has no transaction open: COMMIT or ROLLBACK first.
2. The graph exists, and the alias is that of one of its edge tables
   (GRF-0012).
3. The edge table is a base table, and its user owns the table's schema.
4. Each column INCLUDE names is the table's and NOT NULL.
5. No index has the name.

## PRIVILEGES

The table's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 2A000 | -102 | The DIRECTION is none of FORWARD, REVERSE and BOTH |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the table's schema |
| 0A000 | -900 | A column INCLUDE names may be null |

## EXAMPLES

```sql
CREATE TABLE CITY (NAME VARCHAR(20) PRIMARY KEY);
CREATE TABLE ROAD (ID INTEGER PRIMARY KEY, A VARCHAR(20) NOT NULL, B VARCHAR(20) NOT NULL, KM INTEGER NOT NULL);
CREATE PROPERTY GRAPH ROADS VERTEX TABLES (CITY) EDGE TABLES (ROAD
    SOURCE KEY (A) REFERENCES CITY (NAME) DESTINATION KEY (B) REFERENCES CITY (NAME))
    OPTIONS (TRUSTED MODE);
COMMIT;
CREATE ADJACENCY INDEX ON GRAPH ROADS EDGE TABLE ROAD INCLUDE (KM);
SELECT INDEX_NAME, COLUMN_NAMES FROM INFORMATION_SCHEMA.INDEXES WHERE METHOD_NAME = 'adjacency';
COMMIT;
```

```
OK
OK
OK
OK
OK
 INDEX_NAME                                                   | COLUMN_NAMES
--------------------------------------------------------------+--------------------------------------------------------------
 ROADS_ROAD_ADJACENCY                                         | A, B, KM
(1 row)
OK
```

## STANDARD

The property graph implementation standard's §13.2 describes the
statement. turborel's form of it is an extension (RULES.md D107).

## SEE ALSO

[CREATE INDEX](create-index.md),
[CREATE PROPERTY GRAPH](create-property-graph.md),
[DROP INDEX](drop-index.md)
