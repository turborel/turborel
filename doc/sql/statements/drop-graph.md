# DROP GRAPH

[Statements](README.md)

## NAME

DROP GRAPH: drop a GQL graph and its tables

## SYNOPSIS

```
<drop graph statement> ::=
    DROP GRAPH [ IF EXISTS ] <graph name> [ <drop behavior> ]

<drop behavior> ::= CASCADE | RESTRICT
```

## DESCRIPTION

DROP GRAPH drops a GQL graph made by [CREATE GRAPH](create-graph.md),
with the property graph over its tables, the tables and its sequence.
IF EXISTS drops nothing, and raises nothing, if there is no GQL graph of
the name. DROP PROPERTY GRAPH refuses a GQL graph, whose tables would be
left behind.

DROP GRAPH runs within the session's transaction, and ROLLBACK undoes
it.

## RULES

1. The GQL graph exists, but with IF EXISTS.
2. With RESTRICT, no view and no routine reads it.
3. The transaction is not READ ONLY.

## PRIVILEGES

The graph's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the graph's schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

Of the graph FREE of [CREATE GRAPH](create-graph.md):

```sql
DROP PROPERTY GRAPH FREE;
DROP GRAPH FREE;
DROP GRAPH IF EXISTS FREE;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.FREE is a GQL graph, which DROP GRAPH drops with its tables (at character 20)
OK
OK
```

## STANDARD

ISO/IEC 39075:2024 (GQL) defines DROP GRAPH. turborel drops the tables
CREATE GRAPH made with it (RULES.md D-GQL).

## SEE ALSO

[CREATE GRAPH](create-graph.md), [DROP GRAPH TYPE](drop-graph-type.md)
