# DROP GRAPH TYPE

[Statements](README.md)

## NAME

DROP GRAPH TYPE: drop a graph type

## SYNOPSIS

```
<drop graph type statement> ::=
    DROP GRAPH TYPE [ IF EXISTS ] <graph type name> [ <drop behavior> ]

<drop behavior> ::= CASCADE | RESTRICT
```

## DESCRIPTION

DROP GRAPH TYPE drops a graph type. RESTRICT, the default, refuses the
drop if a GQL graph is typed by it, and CASCADE drops those graphs too.
IF EXISTS drops nothing, and raises nothing, if there is no graph type
of the name.

## RULES

1. The graph type exists, but with IF EXISTS.
2. With RESTRICT, no GQL graph is typed by it.
3. The transaction is not READ ONLY.

## PRIVILEGES

The graph type's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the graph type's schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

Of the graph type SOCIAL and its graph NET of [CREATE GRAPH](create-graph.md):

```sql
DROP GRAPH TYPE SOCIAL;
DROP GRAPH TYPE SOCIAL CASCADE;
SELECT COUNT(*) FROM INFORMATION_SCHEMA.GQL_GRAPHS;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): graph ANN.NET is of graph type ANN.SOCIAL, so it is dropped only with CASCADE (at character 16)
OK

----------------------
                    0
(1 row)
```

## STANDARD

ISO/IEC 39075:2024 (GQL) defines DROP GRAPH TYPE (RULES.md D-GQL).

## SEE ALSO

[CREATE GRAPH TYPE](create-graph-type.md), [DROP GRAPH](drop-graph.md)
