# Topology-Geometry

[Contents](README.md)

A topology describes a planar graph of nodes, edges and faces. Each edge
runs from a start node to an end node along a curve. Faces are the areas
the edges bound, and the universal face, face 0, is everything outside
them. The graph stays consistent: no two nodes coincide, no edge crosses
another or passes through a node, and each edge knows its faces and the
next edge round each of them. The routines of SQL/MM Spatial §11 build a
topology and edit it, keeping it so.

The examples on this page are run in order, by the user ANN. ANN has a
table R that takes the values of the functions that change a topology:

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE R (N INTEGER);
```

```
OK
OK
```

## A topology's schema

`CALL ST_INITTOPOGEO(name)` makes a topology. It creates a schema of
that name, spelled exactly as given, which the caller owns. The schema
holds three views, ST_NODE, ST_EDGE and ST_FACE, and under them three
base tables, ST_TOPO_NODE, ST_TOPO_EDGE and ST_TOPO_FACE, which hold the
rows. ST_FACE starts with the universal face. Like every definition, it
is undone if the transaction rolls back:

```sql
CALL ST_INITTOPOGEO('Field');
SELECT FACE_ID, MBR FROM "Field".ST_FACE;
```

```
OK
      FACE_ID | MBR
--------------+--------------------------------------------------------------
            0 | NULL
(1 row)
```

| View | Columns |
|---|---|
| ST_NODE | NODE_ID INTEGER, GEOMETRY ST_POINT, CONTAINING_FACE INTEGER (the face of an isolated node, else NULL) |
| ST_EDGE | EDGE_ID, START_NODE, END_NODE, NEXT_LEFT_EDGE, NEXT_RIGHT_EDGE, LEFT_FACE, RIGHT_FACE INTEGER, GEOMETRY ST_CURVE |
| ST_FACE | FACE_ID INTEGER, MBR ST_POLYGON (the face's bounding rectangle, NULL for face 0) |

NEXT_LEFT_EDGE is the next edge counterclockwise round the face on the
edge's left, and NEXT_RIGHT_EDGE the same round the face on its right.
Either is negative when that edge is walked from its end to its start.

A user who may SELECT from the three views may edit the topology with
its routines. The routines read the views as that user and write the
base tables as the schema's owner.

## Routines

| Routine | Kind | What it does |
|---|---|---|
| `ST_ADDISONODE(t, face, point)` | function | Adds an isolated node in the face it is within. A NULL face takes that face. Returns the node's id |
| `ST_MOVEISONODE(t, node, point)` | procedure | Moves an isolated node |
| `ST_REMISONODE(t, node)` | procedure | Removes an isolated node |
| `ST_ADDISOEDGE(t, start, end, curve)` | function | Adds an edge between two isolated nodes of one face. Returns its id |
| `ST_CHANGEEDGEGEOM(t, edge, curve)` | procedure | Gives an edge another curve between its nodes |
| `ST_REMISOEDGE(t, edge)` | procedure | Removes an edge no other edge touches, isolating its nodes |
| `ST_NEWEDGESSPLIT(t, edge, point)` | function | Replaces an edge with two new ones that meet at a new node. Returns the node |
| `ST_MODEDGESPLIT(t, edge, point)` | function | Splits an edge at a new node, keeping it as the first part. Returns the node |
| `ST_NEWEDGEHEAL(t, edge1, edge2)` | function | Replaces two edges meeting at a node no other edge touches with one new edge. Returns it |
| `ST_MODEDGEHEAL(t, edge1, edge2)` | procedure | Heals two edges into the first |
| `ST_ADDEDGENEWFACES(t, start, end, curve)` | function | Adds an edge. A face it splits is replaced by two new faces. Returns the edge |
| `ST_ADDEDGEMODFACE(t, start, end, curve)` | function | Adds an edge. A face it splits keeps its id on one side and a new face is made on the other. Returns the edge |
| `ST_REMEDGENEWFACE(t, edge)` | function | Removes an edge. The two faces it separated become a new face, whose id is returned, NULL if none is made |
| `ST_REMEDGEMODFACE(t, edge)` | procedure | Removes an edge. The face on its left absorbs the one on its right |
| `ST_GETFACEEDGES(t, face)` | table function | The signed edges round a face, counterclockwise, as rows (SEQUENCE, EDGE) |
| `ST_GETFACEGEOMETRY(t, face)` | function | The polygon of a face |
| `ST_INITTOPOGEO(t)` | procedure | Makes a topology |
| `ST_CREATETOPOGEO(t, collection)` | procedure | Fills an empty topology from a geometry collection |
| `ST_VALIDATETOPOGEO(t)` | table function | The inconsistencies of a topology, as rows (ERROR, PRIMITIVE1, PRIMITIVE2) |

The procedures are called by CALL. The functions that change a topology
may modify SQL-data, so they are called where such a function may be:
in a row of an INSERT's VALUES, a SET clause, or a CALL's argument. The
table functions are called as `TABLE(f(args))` in a FROM clause.
ST_GETFACEGEOMETRY is called in any value expression. The name of the
topology given to these three is a literal or a parameter, as the views
they read are found when the statement is prepared.

## Building a topology

Four isolated nodes, then four edges round them, close a ring. The ring
makes face 1. R holds the four nodes' identifiers, then the four edges':

```sql
INSERT INTO R VALUES (ST_ADDISONODE('Field', CAST(NULL AS INTEGER), ST_POINT(0, 0)));
INSERT INTO R VALUES (ST_ADDISONODE('Field', CAST(NULL AS INTEGER), ST_POINT(10, 0)));
INSERT INTO R VALUES (ST_ADDISONODE('Field', CAST(NULL AS INTEGER), ST_POINT(10, 10)));
INSERT INTO R VALUES (ST_ADDISONODE('Field', CAST(NULL AS INTEGER), ST_POINT(0, 10)));
INSERT INTO R VALUES (ST_ADDEDGEMODFACE('Field', 1, 2, TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 10 0)') AS ST_CURVE)));
INSERT INTO R VALUES (ST_ADDEDGEMODFACE('Field', 2, 3, TREAT(ST_GEOMFROMTEXT('LINESTRING (10 0, 10 10)') AS ST_CURVE)));
INSERT INTO R VALUES (ST_ADDEDGEMODFACE('Field', 3, 4, TREAT(ST_GEOMFROMTEXT('LINESTRING (10 10, 0 10)') AS ST_CURVE)));
INSERT INTO R VALUES (ST_ADDEDGEMODFACE('Field', 4, 1, TREAT(ST_GEOMFROMTEXT('LINESTRING (0 10, 0 0)') AS ST_CURVE)));
SELECT N FROM R;
SELECT EDGE_ID, START_NODE, END_NODE, NEXT_LEFT_EDGE, NEXT_RIGHT_EDGE, LEFT_FACE, RIGHT_FACE
    FROM "Field".ST_EDGE ORDER BY EDGE_ID;
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
            N
--------------
            1
            2
            3
            4
            1
            2
            3
            4
(8 rows)
      EDGE_ID |   START_NODE |     END_NODE | NEXT_LEFT_EDGE | NEXT_RIGHT_EDGE |    LEFT_FACE |   RIGHT_FACE
--------------+--------------+--------------+----------------+-----------------+--------------+--------------
            1 |            1 |            2 |              2 |              -4 |            1 |            0
            2 |            2 |            3 |              3 |              -1 |            1 |            0
            3 |            3 |            4 |              4 |              -2 |            1 |            0
            4 |            4 |            1 |              1 |              -3 |            1 |            0
(4 rows)
```

An edge across the face splits it. ST_ADDEDGENEWFACES replaces face 1
with two new faces:

```sql
INSERT INTO R VALUES (ST_ADDEDGENEWFACES('Field', 1, 3, TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 10 10)') AS ST_CURVE)));
SELECT FACE_ID, ST_ASTEXT(MBR) AS MBR FROM "Field".ST_FACE ORDER BY FACE_ID;
SELECT SEQUENCE, EDGE FROM TABLE(ST_GETFACEEDGES('Field', 2)) AS E;
VALUES ST_ASTEXT(ST_GETFACEGEOMETRY('Field', 3));
```

```
OK
      FACE_ID | MBR
--------------+--------------------------------------------------------------
            0 | NULL
            2 | POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))
            3 | POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))
(3 rows)
     SEQUENCE |         EDGE
--------------+--------------
            1 |            3
            2 |            4
            3 |            5
(3 rows)

--------------------------------------------------------------
 POLYGON ((0 0, 10 0, 10 10, 0 0))
(1 row)
```

## Editing a topology

A split adds a node on an edge, and a heal takes it away again. Removing
the diagonal edge heals the two faces into one:

```sql
INSERT INTO R VALUES (ST_MODEDGESPLIT('Field', 1, ST_POINT(4, 0)));
SELECT EDGE_ID, START_NODE, END_NODE, ST_ASTEXT(GEOMETRY) AS GEOMETRY FROM "Field".ST_EDGE WHERE EDGE_ID IN (1, 6);
CALL ST_MODEDGEHEAL('Field', 1, 6);
CALL ST_REMEDGEMODFACE('Field', 5);
SELECT FACE_ID FROM "Field".ST_FACE ORDER BY FACE_ID;
```

```
OK
      EDGE_ID |   START_NODE |     END_NODE | GEOMETRY
--------------+--------------+--------------+--------------------------------------------------------------
            1 |            1 |            5 | LINESTRING (0 0, 4 0)
            6 |            5 |            2 | LINESTRING (4 0, 10 0)
(2 rows)
OK
OK
      FACE_ID
--------------
            0
            2
(2 rows)
```

ST_VALIDATETOPOGEO lists what is inconsistent, and here finds nothing.
COMMIT keeps the topology:

```sql
SELECT * FROM TABLE(ST_VALIDATETOPOGEO('Field')) AS V;
COMMIT;
```

```
 ERROR                          |   PRIMITIVE1 |   PRIMITIVE2
--------------------------------+--------------+--------------
(0 rows)
OK
```

## Building from geometry

ST_CREATETOPOGEO fills a topology that has only its universal face. It
nodes the collection's points, curves and polygon rings, splitting curves
where they cross or touch, so two crossing lines meet at a new node. A
polygon's ring bounds a face, and a point becomes an isolated node:

```sql
CALL ST_INITTOPOGEO('Roads');
CALL ST_CREATETOPOGEO('Roads', TREAT(ST_GEOMFROMTEXT(
    'GEOMETRYCOLLECTION (LINESTRING (0 0, 10 10), LINESTRING (0 10, 10 0), POLYGON ((20 0, 30 0, 30 10, 20 10, 20 0)), POINT (25 5))')
    AS ST_GEOMCOLLECTION));
SELECT NODE_ID, ST_ASTEXT(GEOMETRY) AS GEOMETRY, CONTAINING_FACE FROM "Roads".ST_NODE ORDER BY NODE_ID;
SELECT COUNT(*) AS EDGES FROM "Roads".ST_EDGE;
```

```
OK
OK
      NODE_ID | GEOMETRY                                                     | CONTAINING_FACE
--------------+--------------------------------------------------------------+-----------------
            1 | POINT (0 0)                                                  |            NULL
            2 | POINT (0 10)                                                 |            NULL
            3 | POINT (5 5)                                                  |            NULL
            4 | POINT (10 0)                                                 |            NULL
            5 | POINT (10 10)                                                |            NULL
            6 | POINT (20 0)                                                 |            NULL
            7 | POINT (25 5)                                                 |               1
(7 rows)
                EDGES
----------------------
                    5
(1 row)
```

## Errors

Each routine checks its arguments in the order the standard gives and
raises the SQLSTATE of its first failure. The topology is left as it
was:

```sql
INSERT INTO R VALUES (ST_ADDISONODE('Field', CAST(NULL AS INTEGER), ST_POINT(0, 0)));
CALL ST_REMISOEDGE('Field', 99);
CALL ST_INITTOPOGEO('Field');
SELECT * FROM TABLE(ST_VALIDATETOPOGEO('Nowhere')) AS V;
```

```
turborel: SQLCODE -301 (SQLSTATE 2FF28): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.1 Description 2e i] node 1 is there
turborel: SQLCODE -301 (SQLSTATE 2FF43): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.7 Description 2e] no edge 99
turborel: SQLCODE -301 (SQLSTATE 2FF56): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.17 Description 2b] schema "Field" exists
turborel: SQLCODE -301 (SQLSTATE 2FF62): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3 Description 2b] table Nowhere.ST_NODE is not defined (at character 14)
```

| SQLSTATE | When |
|---|---|
| 2FF03 | An argument is NULL |
| 2FF62 | The name is empty, longer than 128 characters, or names no topology |
| 2FF63 | The user may not SELECT from the topology's ST_NODE |
| 2FF56 | ST_INITTOPOGEO names a schema that exists |
| 2FF47, 2FF48 | ST_CREATETOPOGEO names no schema, or a schema without one of the views |
| 2FF42, 2FF61 | ST_CREATETOPOGEO's topology is not empty, or its one face is not face 0 |
| 2FF43, 2FF44, 2FF46 | An edge, face or node does not exist |
| 2FF27, 2FF28 | An edge or node would coincide with another |
| 2FF29 | A curve is not simple |
| 2FF30, 2FF34, 2FF35, 2FF36 | A curve or point would cross an edge or node, or meet an edge |
| 2FF33, 2FF57 | A curve does not end or start at its node |
| 2FF37, 2FF51 | A curve or point is not within the face |
| 2FF39 | ST_ADDISOEDGE's nodes are in different faces |
| 2FF40, 2FF52 | Two edges to heal share no node, or another edge ends at it |
| 2FF50 | A node or edge to remove or move is not isolated |
| 2FF54 | The point to split at is not on the edge |
| 2FF60 | ST_GETFACEGEOMETRY is given face 0 |
| 2FF32 | ST_VALIDATETOPOGEO's topology is empty |
| 2FF10, 2FF25 | Geometries of different SRIDs or coordinate dimensions are mixed |

## Standard

The views, the routines and their exceptions are those of ISO/IEC
13249-3:2016, SQL/MM Spatial, §11 and §22. The base tables under the
views are Turborel's own, as §11.2 gives its tables as a model only. The
readings taken of the text are listed in
[CONFORMANCE.md](../../CONFORMANCE.md).

## See also

[Spatial Data](10-spatial.md), [CALL](statements/call.md)
