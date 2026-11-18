# EXECUTE GQL

[Statements](README.md)

## NAME

EXECUTE GQL: run a GQL request on a graph

## SYNOPSIS

```
<execute gql statement> ::=
    EXECUTE GQL [ ON [ PROPERTY ] GRAPH <graph name> ] <character string literal>

<gql table> ::=
    GQL_TABLE ( <graph name> , <character string literal> )
      [ AS ] <correlation name> [ ( <column name> [ , ... ] ) ]

<GQL request> ::= <session statement> | <transaction statement> | <procedure>

<procedure> ::= <statement> [ { NEXT <statement> }... ]

<statement> ::= <linear statement>
    [ { { UNION | EXCEPT | INTERSECT } [ ALL | DISTINCT ] | OTHERWISE } <linear statement> ]...

<linear statement> ::= [ USE <graph name> ] <clause>... [ <result> ]

<clause> ::=
    [ OPTIONAL ] MATCH <graph pattern>
  | FILTER <search condition> | WHERE <search condition>
  | LET <variable> = <value> [ , ... ]
  | FOR <variable> IN <list> [ WITH ORDINALITY <variable> | WITH OFFSET <variable> ]
  | ORDER BY <sort key> [ , ... ] | OFFSET <count> | SKIP <count> | LIMIT <count>
  | [ OPTIONAL ] CALL [ ( <variable> [ , ... ] ) ] { <procedure> }
  | [ OPTIONAL ] CALL <procedure name> ( <value> [ , ... ] ) [ YIELD <column> [ AS <variable> ] [ , ... ] ]
  | INSERT <path pattern> [ , ... ]
  | SET <set item> [ , ... ] | REMOVE <remove item> [ , ... ]
  | [ DETACH | NODETACH ] DELETE <variable> [ , ... ]

<result> ::= FINISH
  | RETURN [ DISTINCT ] { * | <value> [ AS <name> ] [ , ... ] }
      [ GROUP BY <name> [ , ... ] ] [ ORDER BY ... ] [ OFFSET | SKIP <count> ] [ LIMIT <count> ]
```

## DESCRIPTION

EXECUTE GQL runs a request of ISO/IEC 39075 (GQL) on the graph it
names, or on the session's GQL graph, and returns the rows of its
RETURN. GQL_TABLE is a table of a query, read in a FROM clause as any
table is. The graph is a property graph over tables, one of node and
edge tables (SYS_GRAPH), or a GQL graph of [CREATE GRAPH](create-graph.md).

A procedure reads a binding table, starting from one row of no column.
MATCH joins it with the rows its pattern matches, on the variables
both have, and OPTIONAL MATCH keeps each row that matches nothing, its
new variables null. FILTER and WHERE keep the rows a condition is true
of, LET adds a value, and FOR adds a row for each element of a list.
CALL joins the rows of an inline procedure, which sees the variables
its scope names, or of a table function. RETURN projects the table,
grouping by its items that call no aggregate if one does. NEXT feeds a
statement's rows to the next. A set operator combines statements by
their columns' names, and OTHERWISE gives the right's rows only if the
left has none.

The patterns are those of GRAPH_TABLE, with labels, property maps,
quantifiers, the restrictors TRAIL, ACYCLIC and SIMPLE, and the
selectors ANY, ALL SHORTEST and SHORTEST k. A property is written
`x.name`, a label test `x:L` or `x IS LABELED L`, and ELEMENT_ID,
LABELS, PATH_LENGTH and COUNT of a group variable read the bindings.

INSERT writes the vertices and edges of its patterns, a vertex the
table binds already written no more. In a graph over tables an
element's labels choose the one table it is written to, and its
properties are that table's columns, its key among them. SET writes a
property, all of them with `x = {...}`, or a label of an open graph;
REMOVE takes one away. DELETE deletes an element, and refuses a vertex
with edges unless DETACH deletes them too. A procedure that writes
runs under a savepoint, so it writes all or nothing, and its writes are
the transaction's, which ROLLBACK undoes.

A session whose setting SESSION.LANGUAGE is GQL reads each statement
its client sends as a GQL request, but SET, on the graph SESSION SET
GRAPH names. SESSION SET SCHEMA, SESSION SET TIME ZONE, SESSION RESET
and SESSION CLOSE set and reset the session, and START TRANSACTION,
COMMIT and ROLLBACK are SQL's.

## RULES

1. The graph exists, and its user may query it.
2. Each label, property and variable is the graph's or in scope
   (GRF-0012).
3. A write is one the graph's tables take (GRF-0051, GRF-0053).
4. EXECUTE GQL is not prepared, and a GQL_TABLE's request is a query.

## PRIVILEGES

SELECT on the graph, and on each table it reads; INSERT, UPDATE and
DELETE on the tables a procedure writes.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -102 | The text is no GQL request |
| 2A000 | -103 | A rule above is broken; the GRF codes are in [Appendix A](../../appendix/a-sqlstate.md) |
| 42000 | -104 | Its user may not query the graph |
| 23000 | -201 | A vertex with edges is deleted (GRF-0052), or a typed graph refuses a write (GRF-0053) |
| 0A000 | -900 | CHEAPEST, or a path function GRAPH_TABLE alone reads |

## EXAMPLES

Of the graph NET of [CREATE GRAPH](create-graph.md):

```sql
EXECUTE GQL ON GRAPH NET 'INSERT (:PERSON {name: ''Ann'', age: 30})-[:KNOWS {since: 2020}]->(:PERSON {name: ''Bob'', age: 40})';
EXECUTE GQL ON GRAPH NET 'MATCH (p:PERSON {name: ''Bob''}) INSERT (p)-[:LIVES_IN]->(:CITY {name: ''Rome''})';
EXECUTE GQL ON GRAPH NET 'MATCH (a:PERSON)-[k:KNOWS]->(b) OPTIONAL MATCH (b)-[:LIVES_IN]->(c) RETURN a.name AS a, k.since AS since, c.name AS city';
EXECUTE GQL ON GRAPH NET 'MATCH (p:PERSON) LET older = p.age + 1 FILTER older > 35 RETURN p.name AS name, older';
EXECUTE GQL ON GRAPH NET 'MATCH (p:PERSON) RETURN COUNT(*) AS n, SUM(p.age) AS ages';
```

```
OK
OK
 A                                                            |        SINCE | CITY
--------------------------------------------------------------+--------------+--------------------------------------------------------------
 Ann                                                          |         2020 | Rome
(1 row)
 NAME                                                         |                OLDER
--------------------------------------------------------------+----------------------
 Bob                                                          |                   41
(1 row)
                    N |                 AGES
----------------------+----------------------
                    2 |                   70
(1 row)
```

A write, and what it refuses:

```sql
EXECUTE GQL ON GRAPH NET 'MATCH (p:PERSON {name: ''Ann''}) SET p.age = 31 RETURN p.name AS name, p.age AS age';
EXECUTE GQL ON GRAPH NET 'MATCH (p:PERSON {name: ''Bob''}) DELETE p';
EXECUTE GQL ON GRAPH NET 'INSERT (:ROBOT {name: ''R2''})';
```

```
 NAME                                                         |          AGE
--------------------------------------------------------------+--------------
 Ann                                                          |           31
(1 row)
turborel: SQLCODE -201 (SQLSTATE 23000): GRF-0052 vertex NET$PERSON|2 has edges, so DELETE does not delete it, but DETACH DELETE deletes them too
turborel: SQLCODE -201 (SQLSTATE 23000): GRF-0053 the labels ROBOT name no one element table, against the graph type of ANN.NET
```

GQL_TABLE in a query, and a session in GQL:

```sql
SELECT G.NAME FROM GQL_TABLE(NET, 'MATCH (p:PERSON) RETURN p.name AS name') AS G ORDER BY 1;
SET SESSION.LANGUAGE = 'GQL';
SESSION SET GRAPH NET;
MATCH (p:PERSON)-[:KNOWS]->{1,2}(q) RETURN DISTINCT q.name AS name;
SET SESSION.LANGUAGE = 'SQL';
```

```
 NAME
--------------------------------------------------------------
 Ann
 Bob
(2 rows)
OK
OK
 NAME
--------------------------------------------------------------
 Bob
(1 row)
OK
```

## STANDARD

ISO/IEC 39075:2024 (GQL) defines the requests. EXECUTE GQL, GQL_TABLE
and the setting SESSION.LANGUAGE are turborel's, of its property graph
implementation standard (§9). A query is translated to SQL clause by
clause, as Francis et al. show it can be, and a procedure that writes
runs its clauses in order (RULES.md D-GQL).

## SEE ALSO

[CREATE GRAPH](create-graph.md), [CREATE PROPERTY GRAPH](create-property-graph.md),
[SET](set-setting.md)
