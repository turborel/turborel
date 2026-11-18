# CREATE GRAPH

[Statements](README.md)

## NAME

CREATE GRAPH: create a GQL graph, typed by a graph type or open

## SYNOPSIS

```
<create graph statement> ::=
    CREATE GRAPH [ IF NOT EXISTS ] <graph name>
      [ { TYPED | :: } <graph type name> | ANY [ PROPERTY GRAPH ] ]
```

## DESCRIPTION

CREATE GRAPH creates a GQL graph and the tables that hold it, and the
property graph of its name over them, which GQL and GRAPH_TABLE read.

A typed graph has a base table for each node type and each edge type
of its graph type, named after the graph and the type, as `NET$PERSON`.
Each table has a key `$ID` that the graph's sequence `NET$SEQ` numbers,
a column for each property, and, of an edge type, columns `$SRC` and
`$DST` that reference its endpoints' tables. A label or a property the
type has not is refused.

An open graph, of ANY or of no type written, takes any labels and
properties. Its vertices are rows of `G$NODES` and its edges of
`G$EDGES`, the labels as text and the properties as a JSON object.

CREATE GRAPH runs within the session's transaction, and ROLLBACK undoes
it with its tables.

## RULES

1. No GQL graph of the name exists, but with IF NOT EXISTS, and no
   table or graph has the name of one it makes.
2. A typed graph's graph type exists.
3. The transaction is not READ ONLY.

## PRIVILEGES

The graph's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken; GRF-0012 of no such graph type ([Appendix A](../../appendix/a-sqlstate.md)) |
| 2A000 | -104 | Its user does not own the schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

Of the graph type SOCIAL of [CREATE GRAPH TYPE](create-graph-type.md):

```sql
CREATE GRAPH NET TYPED SOCIAL;
CREATE GRAPH FREE ANY;
SELECT GRAPH_NAME, GRAPH_TYPE_NAME FROM INFORMATION_SCHEMA.GQL_GRAPHS ORDER BY 1;
SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME LIKE 'NET$%' ORDER BY 1;
```

```
OK
OK
 GRAPH_NAME                                                   | GRAPH_TYPE_NAME
--------------------------------------------------------------+--------------------------------------------------------------
 FREE                                                         | NULL
 NET                                                          | SOCIAL
(2 rows)
 TABLE_NAME
--------------------------------------------------------------
 NET$CITY
 NET$KNOWS
 NET$LIVES_IN
 NET$PERSON
(4 rows)
```

## STANDARD

ISO/IEC 39075:2024 (GQL) defines CREATE GRAPH. turborel holds a GQL
graph in tables it generates, Mode C of its property graph
implementation standard (§4.3, RULES.md D110).

## SEE ALSO

[CREATE GRAPH TYPE](create-graph-type.md), [DROP GRAPH](drop-graph.md),
[EXECUTE GQL](execute-gql.md), [CREATE PROPERTY GRAPH](create-property-graph.md)
