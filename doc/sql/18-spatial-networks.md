# Spatial Networks

[Contents](README.md)

A network is a graph of nodes and directed links. Each node may have a
point and each link a curve from its start node's point to its end
node's, or none of them may, and then the network is logical. The
routines of SQL/MM Spatial's topology-networks make a network, add,
move and remove its nodes and links, split a link in two, heal two links
into one, and check what the rows hold. Two table functions find the
shortest paths through any table of curves, a network's links among
them. The routines are built in, as the geometry types are, so every
database has them. Read [Spatial Data](10-spatial.md) first for the
types.

The examples on this page are run in order, by the user ANN. Table IDS
takes the identifiers the functions return, as a function that changes
data is called where a statement may change data:

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE IDS (ID INTEGER);
COMMIT;
```

```
OK
OK
OK
```

## Making a network

`CALL ST_INITTOPONET(name)` makes a network. The name is a character
string of at most 128 characters, and the network is a schema of that
name, owned by the user who calls it. The schema holds two views, which
any query reads.

| View | Columns |
|---|---|
| ST_NODE | NODE_ID INTEGER, GEOMETRY ST_POINT |
| ST_LINK | LINK_ID INTEGER, START_NODE INTEGER, END_NODE INTEGER, GEOMETRY ST_CURVE |

Each view is over a base table of the same columns, ST_NET_NODE and
ST_NET_LINK, whose keys are the identifiers and whose START_NODE and
END_NODE reference ST_NET_NODE. The routines write the tables as the
schema's owner, so a user who may call a routine changes the network
through it and not otherwise. The name is matched as given, with its
case, so a query quotes it:

```sql
CALL ST_INITTOPONET('Roads');
SELECT TABLE_NAME, TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES
    WHERE TABLE_SCHEMA = 'Roads' ORDER BY TABLE_NAME;
CALL ST_INITTOPONET('Roads');
```

```
OK
 TABLE_NAME                                                   | TABLE_TYPE
--------------------------------------------------------------+--------------------------------------------------------------
 ST_LINK                                                      | VIEW
 ST_NET_LINK                                                  | BASE TABLE
 ST_NET_NODE                                                  | BASE TABLE
 ST_NODE                                                      | VIEW
(4 rows)
turborel: SQLCODE -301 (SQLSTATE 2FF56): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.7 Description 2b] schema "Roads" exists
```

## Nodes and links

| Routine | What it does |
|---|---|
| `ST_ADDISONETNODE(net, point)` | Adds an isolated node, of the point or of none, and returns its identifier |
| `ST_MOVEISONETNODE(net, node, point)` | Gives an isolated node another point, or none |
| `ST_REMISONETNODE(net, node)` | Removes an isolated node |
| `ST_ADDLINK(net, start, end, curve)` | Adds a link from node `start` to node `end`, of the curve or of none, and returns its identifier |
| `ST_CHANGELINKGEOM(net, link, curve)` | Gives a link another curve, or none |
| `ST_REMOVELINK(net, link)` | Removes a link, its nodes kept |

A node is isolated when no link starts or ends at it. A link's curve
starts at its start node's point and ends at its end node's, which
ST_ADDLINK and ST_CHANGELINKGEOM check of each node that has a point.
New identifiers are one more than the greatest in use. The functions
return INTEGER and are called where a statement changes data, the
procedures by CALL:

```sql
INSERT INTO IDS VALUES (ST_ADDISONETNODE('Roads', ST_POINT(0, 0)));
INSERT INTO IDS VALUES (ST_ADDISONETNODE('Roads', ST_POINT(10, 0)));
INSERT INTO IDS VALUES (ST_ADDISONETNODE('Roads', ST_POINT(10, 10)));
INSERT INTO IDS VALUES
    (ST_ADDLINK('Roads', 1, 2, TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 10 0)') AS ST_CURVE))),
    (ST_ADDLINK('Roads', 2, 3, TREAT(ST_GEOMFROMTEXT('LINESTRING (10 0, 10 10)') AS ST_CURVE)));
SELECT ID FROM IDS ORDER BY ID;
SELECT LINK_ID, START_NODE, END_NODE, ST_ASTEXT(GEOMETRY) AS GEOMETRY FROM "Roads".ST_LINK ORDER BY LINK_ID;
```

```
OK
OK
OK
OK
           ID
--------------
            1
            1
            2
            2
            3
(5 rows)
      LINK_ID |   START_NODE |     END_NODE | GEOMETRY
--------------+--------------+--------------+--------------------------------------------------------------
            1 |            1 |            2 | LINESTRING (0 0, 10 0)
            2 |            2 |            3 | LINESTRING (10 0, 10 10)
(2 rows)
```

A node a link ends at is not moved or removed, and a curve that misses
its node is refused:

```sql
CALL ST_MOVEISONETNODE('Roads', 2, ST_POINT(10, 1));
CALL ST_CHANGELINKGEOM('Roads', 1, TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 5 -2, 10 1)') AS ST_CURVE));
CALL ST_CHANGELINKGEOM('Roads', 1, TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 5 -2, 10 0)') AS ST_CURVE));
SELECT ST_ASTEXT(GEOMETRY) AS GEOMETRY FROM "Roads".ST_LINK WHERE LINK_ID = 1;
```

```
turborel: SQLCODE -301 (SQLSTATE 2FF50): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.2 Description 2e i] a link ends at node 2
turborel: SQLCODE -301 (SQLSTATE 2FF33): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.5 Description 2e i 2B] the curve's end point is not its end node's point
OK
 GEOMETRY
--------------------------------------------------------------
 LINESTRING (0 0, 5 -2, 10 0)
(1 row)
```

## Splits and heals

| Routine | What it does |
|---|---|
| `ST_NEWLOGLINKSPLIT(net, link)` | Replaces a logical link by two new ones through a new node, and returns the node |
| `ST_MODLOGLINKSPLIT(net, link)` | Ends a logical link at a new node and adds a link from it, and returns the node |
| `ST_NEWGEOLINKSPLIT(net, link, point)` | Replaces a spatial link by two new ones through a new node at the point, and returns the node |
| `ST_MODGEOLINKSPLIT(net, link, point)` | Ends a spatial link at a new node at the point and adds a link from it, and returns the node |
| `ST_NEWLINKHEAL(net, link, other)` | Replaces two links that share a node by one new link, removes the node, and returns the link |
| `ST_MODLINKHEAL(net, link, other)` | Extends the first link over the second, which it removes with the node they share |

A split's point lies on the link's curve, not at its ends, and becomes
a vertex of both parts. A heal is refused if the node the links share
is the end of another link. The healed link runs in the first link's
direction, and its curve joins the two curves, the second reversed when
it runs the other way:

```sql
DELETE FROM IDS;
INSERT INTO IDS VALUES (ST_NEWGEOLINKSPLIT('Roads', 2, ST_POINT(10, 4)));
SELECT ID FROM IDS;
SELECT LINK_ID, START_NODE, END_NODE, ST_ASTEXT(GEOMETRY) AS GEOMETRY FROM "Roads".ST_LINK ORDER BY LINK_ID;
CALL ST_MODLINKHEAL('Roads', 3, 4);
SELECT LINK_ID, START_NODE, END_NODE, ST_ASTEXT(GEOMETRY) AS GEOMETRY FROM "Roads".ST_LINK ORDER BY LINK_ID;
INSERT INTO IDS VALUES (ST_NEWGEOLINKSPLIT('Roads', 1, ST_POINT(0, 0)));
```

```
OK
OK
           ID
--------------
            4
(1 row)
      LINK_ID |   START_NODE |     END_NODE | GEOMETRY
--------------+--------------+--------------+--------------------------------------------------------------
            1 |            1 |            2 | LINESTRING (0 0, 5 -2, 10 0)
            3 |            2 |            4 | LINESTRING (10 0, 10 4)
            4 |            4 |            3 | LINESTRING (10 4, 10 10)
(3 rows)
OK
      LINK_ID |   START_NODE |     END_NODE | GEOMETRY
--------------+--------------+--------------+--------------------------------------------------------------
            1 |            1 |            2 | LINESTRING (0 0, 5 -2, 10 0)
            3 |            2 |            3 | LINESTRING (10 0, 10 4, 10 10)
(2 rows)
turborel: SQLCODE -301 (SQLSTATE 2FF55): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.10 Description 2f iii] the point is not within link 1
```

## Networks from other data

| Routine | What it does |
|---|---|
| `ST_SPATNETFROMGEOM(net, collection)` | Fills an empty network from a geometry collection |
| `ST_LOGINETFROMTGEO(net, topology)` | Fills an empty network from a topology's nodes and edges, without geometry |
| `ST_SPATNETFROMTGEO(net, topology)` | Fills an empty network from a topology's nodes and edges, with their geometry |

ST_SPATNETFROMGEOM makes each point of the collection a node, and each
curve a link from a node at its start to a node at its end. Curves that
end at one point share a node, and curves that cross do not meet. Nodes
and links are numbered from 1 in the order the collection holds them.
A topology is a schema of views ST_NODE and ST_EDGE, as SQL/MM Spatial's
topology-geometry defines them, and its edges become links of the same
identifiers:

```sql
CALL ST_INITTOPONET('Paths');
CALL ST_SPATNETFROMGEOM('Paths', TREAT(ST_GEOMFROMTEXT(
    'GEOMETRYCOLLECTION (LINESTRING (0 0, 3 4), LINESTRING (3 4, 3 10), LINESTRING (0 0, 0 10, 3 10), POINT (8 8))')
    AS ST_GEOMCOLLECTION));
SELECT NODE_ID, ST_ASTEXT(GEOMETRY) AS GEOMETRY FROM "Paths".ST_NODE ORDER BY NODE_ID;
SELECT LINK_ID, START_NODE, END_NODE FROM "Paths".ST_LINK ORDER BY LINK_ID;
CALL ST_SPATNETFROMGEOM('Paths', TREAT(ST_GEOMFROMTEXT('GEOMETRYCOLLECTION (POINT (1 1))') AS ST_GEOMCOLLECTION));
```

```
OK
OK
      NODE_ID | GEOMETRY
--------------+--------------------------------------------------------------
            1 | POINT (0 0)
            2 | POINT (3 4)
            3 | POINT (3 10)
            4 | POINT (8 8)
(4 rows)
      LINK_ID |   START_NODE |     END_NODE
--------------+--------------+--------------
            1 |            1 |            2
            2 |            2 |            3
            3 |            1 |            3
(3 rows)
turborel: SQLCODE -301 (SQLSTATE 2FF42): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.16 Description 2h i] the network's ST_NODE has rows
```

## Checking a network

`ST_VALIDLOGICALNET(net)` and `ST_VALIDSPATIALNET(net)` are table
functions of the problems they find, a row each, of the columns ERROR
VARCHAR(30), PRIMITIVE1 INTEGER and PRIMITIVE2 INTEGER. A logical
network's links have no curve, and ST_VALIDLOGICALNET returns a row of
each node or link that has a geometry. ST_VALIDSPATIALNET returns a row
of each node or link without one, of each link whose curve misses its
start or end node's point, and one if the geometries are of more than
one SRID. A network of no node and no link is refused by both:

```sql
SELECT * FROM TABLE(ST_VALIDSPATIALNET('Paths')) AS P;
SELECT * FROM TABLE(ST_VALIDLOGICALNET('Paths')) AS P ORDER BY ERROR, PRIMITIVE1;
CALL ST_INITTOPONET('Empty');
SELECT * FROM TABLE(ST_VALIDSPATIALNET('Empty')) AS P;
```

```
 ERROR                          |   PRIMITIVE1 |   PRIMITIVE2
--------------------------------+--------------+--------------
(0 rows)
 ERROR                          |   PRIMITIVE1 |   PRIMITIVE2
--------------------------------+--------------+--------------
 link has geometry              |            1 |         NULL
 link has geometry              |            2 |         NULL
 link has geometry              |            3 |         NULL
 node has geometry              |            1 |         NULL
 node has geometry              |            2 |         NULL
 node has geometry              |            3 |         NULL
 node has geometry              |            4 |         NULL
(7 rows)
OK
turborel: SQLCODE -301 (SQLSTATE 2FF31): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.18 Description 2d i] the network has neither node nor link
```

## Shortest paths

`ST_SHORTESTUNDPATH(table, id, geometry [, weight], from, to)` finds the
shortest paths from point `from` to point `to` through the curves of a
table, each walked either way. `ST_SHORTESTDIRPATH(table, id, geometry,
start [, weight], from, to)` walks each curve only from the point in its
column `start`, which is one of the curve's ends. Each argument before
the points is a character string naming a table or view, or one of its
columns. The paths meet at the curves' end points, and each curve's
weight is its column `weight`, or its length if no weight is named.

Each returns a row of each path of the least total weight, its column
SHORTEST_PATH an INTEGER ARRAY of the identifiers of the curves walked
in order, and TOTAL_WEIGHT the sum of their weights, DOUBLE PRECISION.
Paths of equal weight are each returned, and no row is returned if no
path leads from one point to the other. Rows of a null identifier,
geometry or weight are passed over, and so is a row whose geometry is
not a curve:

```sql
SELECT * FROM TABLE(ST_SHORTESTUNDPATH('"Paths".ST_LINK', 'LINK_ID', 'GEOMETRY',
    ST_POINT(0, 0), ST_POINT(3, 10))) AS P;
CREATE TABLE TRAILS (ID INTEGER, PATH ST_CURVE, MINUTES DOUBLE PRECISION, FROM_PT ST_POINT);
INSERT INTO TRAILS VALUES
    (1, TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 5 0)') AS ST_CURVE), 10, ST_POINT(0, 0)),
    (2, TREAT(ST_GEOMFROMTEXT('LINESTRING (5 0, 5 5)') AS ST_CURVE), 10, ST_POINT(5, 0)),
    (3, TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 0 5)') AS ST_CURVE), 5, ST_POINT(0, 0)),
    (4, TREAT(ST_GEOMFROMTEXT('LINESTRING (0 5, 5 5)') AS ST_CURVE), 15, ST_POINT(5, 5));
SELECT * FROM TABLE(ST_SHORTESTUNDPATH('TRAILS', 'ID', 'PATH', 'MINUTES',
    ST_POINT(0, 0), ST_POINT(5, 5))) AS P ORDER BY SHORTEST_PATH[1];
SELECT * FROM TABLE(ST_SHORTESTDIRPATH('TRAILS', 'ID', 'PATH', 'FROM_PT', 'MINUTES',
    ST_POINT(0, 0), ST_POINT(5, 5))) AS P;
SELECT * FROM TABLE(ST_SHORTESTDIRPATH('TRAILS', 'ID', 'PATH', 'FROM_PT', 'MINUTES',
    ST_POINT(5, 5), ST_POINT(0, 0))) AS P;
SELECT * FROM TABLE(ST_SHORTESTUNDPATH('TRAILS', 'ID', 'PATH', 'MINUTES',
    ST_POINT(1, 1), ST_POINT(5, 5))) AS P;
```

```
 SHORTEST_PATH                                                |             TOTAL_WEIGHT
--------------------------------------------------------------+--------------------------
 ARRAY[1, 2]                                                  |                    1.1E1
(1 row)
OK
OK
 SHORTEST_PATH                                                |             TOTAL_WEIGHT
--------------------------------------------------------------+--------------------------
 ARRAY[1, 2]                                                  |                      2E1
 ARRAY[3, 4]                                                  |                      2E1
(2 rows)
 SHORTEST_PATH                                                |             TOTAL_WEIGHT
--------------------------------------------------------------+--------------------------
 ARRAY[1, 2]                                                  |                      2E1
(1 row)
 SHORTEST_PATH                                                |             TOTAL_WEIGHT
--------------------------------------------------------------+--------------------------
(0 rows)
turborel: SQLCODE -301 (SQLSTATE 2FF02): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §13.1.1 Description 2e] the point is no end of a path's curve
```

The table and its columns are named when the query is prepared, so
they are character string literals. The search is Dijkstra's, of a
binary heap, and each path's curves are distinct. More than 65535 paths
of the least weight are refused, not cut short.

## Errors

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2FF02 | -301 | A shortest path's table or column is not there or not readable, a weight is negative or not finite, a point is no curve's end, a start point is neither end of its curve, or ST_SPATNETFROMGEOM is given a surface or a solid |
| 2FF03 | -301 | An argument a routine needs is null |
| 2FF05 | -301 | Two rows of a shortest path's table have one identifier |
| 2FF10 | -301 | A geometry is of another SRID than the network's |
| 2FF25 | -301 | Two links to be healed have curves of different coordinate dimensions |
| 2FF31 | -301 | A network to be checked has no node and no link |
| 2FF33 | -301 | A link's curve does not end at its end node's point |
| 2FF38 | -301 | A link to be split by a point has no curve |
| 2FF41 | -301 | Two links to be healed share no node, or are one link |
| 2FF42 | -301 | A network to be filled is not empty |
| 2FF45 | -301 | A link is not there |
| 2FF46 | -301 | A node a new link names is not there |
| 2FF47 | -301 | A topology or network to be read is not there |
| 2FF48 | -301 | A topology or network lacks one of its views |
| 2FF49 | -301 | A link to be split logically has a curve |
| 2FF50 | -301 | A node to be moved or removed is not isolated |
| 2FF53 | -301 | The node two links share is the end of another link |
| 2FF55 | -301 | A split's point is not on the link, or is at its end |
| 2FF56 | -301 | ST_INITTOPONET names a schema that is there |
| 2FF57 | -301 | A link's curve does not start at its start node's point |
| 2FF58 | -301 | A link's node has no point, or the link is not there, as its curve is changed |
| 2FF62 | -301 | A topology's name is not a name |
| 2FF63 | -301 | A topology is not readable by the user |
| 2FF64 | -301 | A network's name is not a name, or names no network |
| 2FF65 | -301 | A network is not readable by the user |

## Standard

The routines, the views and their exceptions are those of ISO/IEC
13249-3:2016, SQL/MM Spatial, §12 and §13. The base tables under the
views, and a network as a schema, are this implementation's model of
§12.2, which says the tables are a model only. The choices made are
listed in [CONFORMANCE.md](../../CONFORMANCE.md).

## See also

[Spatial Data](10-spatial.md),
[Appendix A: SQLSTATE values](../appendix/a-sqlstate.md)
