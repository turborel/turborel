# Graph Compatibility Syntax *(extension)*

[Contents](README.md)

Queries of property graphs may be written as SQL Server, Oracle, PGQL and
openCypher write them. Each is read as a query of GRAPH_TABLE
([Property Graphs](16-property-graphs.md)), one for each MATCH clause of
Cypher and one otherwise, so it matches as GRAPH_TABLE does and gives
the same rows. The examples here read schema ANN's node
table PERSON and edge table FRIEND_OF, and the graph CHAIN of the same
friendships kept in plain tables.

```sql
CREATE TABLE PERSON (ID INTEGER, NAME VARCHAR(10)) AS NODE;
CREATE TABLE FRIEND_OF (SINCE INTEGER) AS EDGE;
INSERT INTO PERSON VALUES (1, 'Alice'), (2, 'Bob'), (3, 'Carol'), (4, 'Dave'), (5, 'Erin');
INSERT INTO FRIEND_OF ($FROM_ID, $TO_ID, SINCE)
    SELECT P.$NODE_ID, Q.$NODE_ID, 2020 FROM PERSON P, PERSON Q
    WHERE (P.ID, Q.ID) IN (VALUES (1, 2), (2, 3), (3, 4), (1, 5), (5, 4));
CREATE TABLE MEMBER (ID INTEGER PRIMARY KEY, NAME VARCHAR(10));
CREATE TABLE FOLLOWS (ID INTEGER PRIMARY KEY,
    SRC INTEGER REFERENCES MEMBER, DST INTEGER REFERENCES MEMBER);
INSERT INTO MEMBER SELECT ID, NAME FROM PERSON;
INSERT INTO FOLLOWS VALUES (12, 1, 2), (23, 2, 3), (34, 3, 4), (15, 1, 5), (54, 5, 4);
CREATE PROPERTY GRAPH CHAIN
    VERTEX TABLES (MEMBER LABEL MEMBER PROPERTIES (ID, NAME))
    EDGE TABLES (FOLLOWS
        SOURCE KEY (SRC) REFERENCES MEMBER (ID)
        DESTINATION KEY (DST) REFERENCES MEMBER (ID)
        LABEL FOLLOWS PROPERTIES (ID));
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
OK
OK
OK
```

## MATCH in a WHERE clause

```
<graph match predicate> ::= MATCH ( <graph search item> [ { AND <graph search item> }... ] )
<graph search item> ::= <simple match pattern>
    | SHORTEST_PATH ( <arbitrary length pattern> [ { AND <arbitrary length pattern> }... ] )
    | LAST_NODE ( <node alias> ) = LAST_NODE ( <node alias> )
<simple match pattern> ::= <match node> { <match edge> <match node> | <match group> }...
<match group> ::= ( <match edge> <node alias> ) <graph pattern quantifier>
<arbitrary length pattern> ::= <match node> <match group>
    | ( <node alias> <match edge> ) <graph pattern quantifier> <match node>
<match node> ::= <node alias> | LAST_NODE ( <node alias> )
<match edge> ::= - ( <edge alias> ) -> | <- ( <edge alias> ) -
<path table> ::= <table name> FOR PATH [ [ AS ] <correlation name> ]
<graph path aggregate> ::= <graph path function> ( <value expression> [ , <separator> ] )
    WITHIN GROUP ( GRAPH PATH )
<graph path function> ::= STRING_AGG | LAST_VALUE | COUNT | SUM | AVG | MIN | MAX
```

MATCH in a WHERE clause, as SQL Server writes it, matches a pattern
over the tables of the FROM clause. Each alias names a table of the FROM
clause, and the rows of the query are those of the tables that a match
joins. The tables may be node and edge tables, whose graph is their
schema's SYS_GRAPH, or the tables of one property graph:

```sql
SELECT p2.name FROM person p1, friend_of f, person p2
  WHERE MATCH(p1-(f)->p2) AND p1.name = 'Alice';
```

```
 NAME
------------
 Bob
 Erin
(2 rows)
```

SHORTEST_PATH matches one shortest path from each start to each end it
reaches, ANY SHORTEST in GRAPH_TABLE. The tables a group repeats are
written FOR PATH, and an aggregate WITHIN GROUP (GRAPH PATH) reads their
elements in path order. STRING_AGG joins them with its separator, and
LAST_VALUE gives the last:

```sql
SELECT p1.name,
       STRING_AGG(p2.name, '->') WITHIN GROUP (GRAPH PATH) AS route,
       LAST_VALUE(p2.name) WITHIN GROUP (GRAPH PATH) AS last_name,
       COUNT(p2.id) WITHIN GROUP (GRAPH PATH) AS hops
  FROM person p1, friend_of FOR PATH f, person FOR PATH p2
  WHERE MATCH(SHORTEST_PATH(p1(-(f)->p2)+)) AND p1.name = 'Alice'
  ORDER BY route;
```

```
 NAME       | ROUTE                                                        | LAST_NAME  |                 HOPS
------------+--------------------------------------------------------------+------------+----------------------
 Alice      | Bob                                                          | Bob        |                    1
 Alice      | Bob->Carol                                                   | Carol      |                    2
 Alice      | Erin                                                         | Erin       |                    1
 Alice      | Erin->Dave                                                   | Dave       |                    2
(4 rows)
```

LAST_NODE (x) is the last node of the shortest path whose group ends
at x. Another pattern may start there, and LAST_NODE (x) = LAST_NODE (y)
makes two paths end at one node:

```sql
SELECT a.name, b.name, LAST_VALUE(x.name) WITHIN GROUP (GRAPH PATH) AS meet
  FROM person a, person b, friend_of FOR PATH f, person FOR PATH x,
       friend_of FOR PATH g, person FOR PATH y
  WHERE MATCH(SHORTEST_PATH(a(-(f)->x)+) AND SHORTEST_PATH(b(-(g)->y)+)
              AND LAST_NODE(x) = LAST_NODE(y))
    AND a.name = 'Bob' AND b.name = 'Erin';
```

```
 NAME       | NAME       | MEET
------------+------------+------------
 Bob        | Erin       | Dave
(1 row)
```

Unlike SQL Server, MATCH may stand under OR and NOT, an edge alias may
be written twice, and a group may be quantified outside SHORTEST_PATH.
A MATCH under OR or NOT has no FOR PATH table or GRAPH PATH aggregate.
A MATCH anywhere but a WHERE clause is refused with SQLCODE -103 (SQLSTATE 2A000),
and so is one whose tables are of no one graph.

## GRAPH_TABLE as Oracle writes it

VERTEX_ID and EDGE_ID give an element's identity as a JSON value, and
VERTEX_EQUAL and EDGE_EQUAL compare two vertices or two edges, as SAME
does. A property of the JSON type is read by the dot notation of the
JSON type ([The JSON Type](12-json-type.md)). Inside GRAPH_TABLE, `x.$NODE_ID`
and `x.$EDGE_ID` are an element's identity too, its table's
pseudo-column of a node or edge table's element.

A comment that begins `/*+` just after GRAPH_TABLE's parenthesis holds
hints, the ones its OPTIONS may write too. GRAPH_START (x) names the
variable a search begins at, and GRAPH_DIRECTION (e, REVERSE) an edge
variable walked from its destination. NO_WCOJ asks for no worst-case
optimal join, USE_ADJACENCY (t) names an edge table's adjacency index,
and PATH_ALGO (BFS | BIBFS | DFS | DIJKSTRA | MSBFS) a path search. A
hint changes no row, and a word of the comment that names no hint is
passed over:

```sql
SELECT * FROM GRAPH_TABLE (/*+ GRAPH_START(a) PATH_ALGO(BIBFS) */ CHAIN
    MATCH ANY SHORTEST p = (a WHERE a.name = 'Alice')-[e]->+(b WHERE b.name = 'Dave')
    COLUMNS (JSON_SERIALIZE(VERTEX_ID(a)) AS START_ID, PATH_LENGTH(p) AS HOPS));
```

```
 START_ID                                                     |         HOPS
--------------------------------------------------------------+--------------
 {"table":"MEMBER","key":[1]}                                 |            2
(1 row)
```

## PGQL

```
<pgql query> ::= [ <path macro>... ] SELECT [ DISTINCT | ALL ] <select list>
    FROM <pgql match> [ { , <pgql match> }... ]
    [ WHERE <search condition> ] [ <group by clause> ] [ <having clause> ]
    [ ORDER BY <sort specification list> ] [ LIMIT <row count> ] [ OFFSET <row count> ]
<pgql match> ::= MATCH <path pattern> [ { , <path pattern> }... ] [ ON <graph name> ]
<path macro> ::= PATH <macro name> AS <path pattern> [ WHERE <search condition> ]
<reachability> ::= -/ <reach> /-> | <-/ <reach> /- | -/ <reach> /-
<reach> ::= : <label expression> [ <quantifier> ]
```

A PGQL query is a query wherever SQL's is. Its patterns are GRAPH_TABLE's,
and its WHERE clause tests each match. The select list and the clauses
after WHERE read the variables, so an aggregate of a group variable is
of each path, and any other aggregates the query's rows. A query of no
ON reads SYS_GRAPH:

```sql
SELECT b.name, COUNT(*) AS followers
  FROM MATCH (a:Member)-[:Follows]->(b) ON chain
  GROUP BY b.name ORDER BY followers DESC, b.name LIMIT 2;
```

```
 NAME       |            FOLLOWERS
------------+----------------------
 Dave       |                    2
 Bob        |                    1
(2 rows)
```

A reachability, written between two nodes of a path pattern, asks only
whether a path of its label's edges joins them. It binds no variable,
and two nodes it joins match once, however many paths there are. Its
quantifier counts the edges, and with none written there is one:

```sql
SELECT b.name FROM MATCH (a:Member)-/:Follows+/->(b) ON chain
  WHERE a.name = 'Bob' ORDER BY b.name;
```

```
 NAME
------------
 Carol
 Dave
(2 rows)
```

The label of a reachability may name a path macro the query defines
first. The macro's pattern is then what repeats, each repetition ending
where the next begins, and its WHERE clause tests each. The macro's
variables are its own and are renamed at each use, so they never meet
the query's. Here each step is two hops that do not pass through Erin:

```sql
PATH hop AS (x)-[:Follows]->(m)-[:Follows]->(y) WHERE m.name <> 'Erin'
SELECT b.name FROM MATCH (a)-/:hop+/->(b) ON chain
  WHERE a.name = 'Alice' ORDER BY b.name;
```

```
 NAME
------------
 Carol
(1 row)
```

## openCypher

```
<cypher query> ::= [ USE <graph name> ] <cypher match>...
    RETURN [ DISTINCT ] <return item> [ { , <return item> }... ]
    [ ORDER BY <sort specification list> ] [ SKIP <row count> ] [ LIMIT <row count> ]
<cypher match> ::= [ OPTIONAL ] MATCH <cypher pattern> [ { , <cypher pattern> }... ]
    [ WHERE <search condition> ]
<cypher pattern> ::= [ <path variable> = ] <cypher path>
    | [ <path variable> = ] { shortestPath | allShortestPaths } ( <cypher path> )
```

A MATCH clause matches no relationship twice. A relationship's length
`*n..m` is a quantifier. `:A:B` is both labels and `:A|B` either. A
return item with an aggregate groups the rows by the others. Strings
are SQL's, in single quotes. A query of no USE reads SYS_GRAPH:

```sql
MATCH (a:PERSON {name: 'Alice'})-[*2]->(c) RETURN DISTINCT c.name ORDER BY c.name;
```

```
 NAME
------------
 Carol
 Dave
(2 rows)
```

```sql
USE chain MATCH p = shortestPath((a)-[*]->(d))
  WHERE a.name = 'Bob' AND d.name = 'Dave'
  RETURN length(p) AS hops;
```

```
         HOPS
--------------
            2
(1 row)
```

A relationship with nothing to say in brackets may be written short, as
`-->`, `<--`, or `--` and `<-->` for either direction. Since `--` begins
a comment in SQL, the short form is read only with no space around it,
between one node's `)` and the next node's `(`:

```sql
MATCH (a:PERSON {name: 'Dave'})<--(b) RETURN b.name ORDER BY b.name;
```

```
 NAME
------------
 Carol
 Erin
(2 rows)
```

A query may have several MATCH clauses. Each is matched on its own, and
a variable that a later clause shares with an earlier one is the same
node or relationship in both:

```sql
USE chain MATCH (a)-->(b) MATCH (b)-->(c)
  RETURN a.name AS origin, c.name AS reached ORDER BY origin, reached;
```

```
 ORIGIN     | REACHED
------------+------------
 Alice      | Carol
 Alice      | Dave
 Bob        | Dave
(3 rows)
```

An OPTIONAL MATCH keeps each earlier row it cannot extend, its own
variables null. Its WHERE clause decides what it matches, so a row that
fails it is kept too:

```sql
USE chain MATCH (a)
  OPTIONAL MATCH (a)-->(b) WHERE b.name > 'C'
  RETURN a.name AS who, b.name AS friend ORDER BY who, friend;
```

```
 WHO        | FRIEND
------------+------------
 Alice      | Erin
 Bob        | Carol
 Carol      | Dave
 Dave       | NULL
 Erin       | Dave
(5 rows)
```
