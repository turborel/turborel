# DROP PROPERTY GRAPH

[Statements](README.md)

## NAME

DROP PROPERTY GRAPH: drop a property graph

## SYNOPSIS

```
<drop property graph statement> ::=
    DROP PROPERTY GRAPH [ IF EXISTS ] <graph name> [ <drop behavior> ]

<drop behavior> ::= CASCADE | RESTRICT
```

## DESCRIPTION

DROP PROPERTY GRAPH drops a graph and the privileges on it; no row of
its tables changes. RESTRICT, the default, refuses the drop if a view
or routine reads the graph, and CASCADE drops those too. IF EXISTS
drops nothing, and raises nothing, if there is no graph of the name.

A graph goes too with a table it reads, when DROP TABLE drops it with
CASCADE, and with its schema; RESTRICT refuses either.

DROP PROPERTY GRAPH runs within the session's transaction: ROLLBACK
undoes it.

## RULES

1. The graph exists, but with IF EXISTS.
2. Its user owns the graph's schema.
3. With RESTRICT, no view and no routine reads it.
4. The transaction is not READ ONLY.

## PRIVILEGES

The graph's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken; GRF-0001 of no such graph ([Appendix A](../../appendix/a-sqlstate.md)) |
| 2A000 | -104 | Its user does not own the graph's schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

Of the graph FRIENDS of [CREATE PROPERTY GRAPH](create-property-graph.md):

```sql
DROP TABLE KNOWS RESTRICT;
DROP PROPERTY GRAPH FRIENDS;
DROP PROPERTY GRAPH FRIENDS;
DROP PROPERTY GRAPH IF EXISTS FRIENDS;
SELECT COUNT(*) FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.KNOWS is read by property graph ANN.FRIENDS, so it is dropped only with CASCADE [ISO/IEC 9075:1992 §11.18 SR4] (at character 11)
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): GRF-0001 there is no property graph ANN.FRIENDS (at character 20)
OK

----------------------
                    0
(1 row)
```

## STANDARD

ISO/IEC 9075-16:2023 (SQL/PGQ) defines DROP PROPERTY GRAPH. turborel
follows its implementation standard's Mode A (§4.1), whose IF EXISTS
is an extension, and takes RESTRICT where no `<drop behavior>` is
written (RULES.md D-GRAPH-CATALOG).

## SEE ALSO

[CREATE PROPERTY GRAPH](create-property-graph.md),
[ALTER PROPERTY GRAPH](alter-property-graph.md), [DROP TABLE](drop-table.md)
