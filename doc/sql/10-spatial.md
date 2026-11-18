# Spatial Data *(extension)*

[Contents](README.md)

A GEOMETRY value is a point, a line, a polygon or a collection of them
in the XY plane, with an SRID naming its spatial reference system. Its
functions read and write the well-known text (WKT) and binary (WKB)
forms, observe a value's coordinates and bounds, test whether two values
intersect and measure the planar distance between them. An index of the
method `rtree` on a GEOMETRY column finds the rows an intersection
filter or join matches, and the rows nearest a point, without reading
the table.

The server, `turboreld`, registers the type and its functions before it
opens its files. A program embedding the library registers them itself
([The Embedded Library](../client/03-embedding.md#spatial-data-extension)).
For an introduction with complete examples, read the
[Spatial Guide](../data/02-spatial.md).

The examples on this page are run in order, by the user ANN, on a table
of places on a field, in a local grid whose unit is the metre:

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE PLACES (ID INTEGER PRIMARY KEY, NAME VARCHAR(20), SHAPE GEOMETRY(0));
INSERT INTO PLACES VALUES
    (1, 'Well', ST_POINT(2, 2)),
    (2, 'Road', ST_GEOMFROMTEXT('LINESTRING (-5 -5, 15 -5)')),
    (3, 'Field', ST_GEOMFROMTEXT('POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (4 4, 6 4, 6 6, 4 6, 4 4))')),
    (4, 'Pond', ST_POINT(5, 5)),
    (5, 'Gate', CAST('POINT (10 5)' AS GEOMETRY(0))),
    (6, 'Lost', ST_GEOMFROMTEXT('POINT EMPTY')),
    (7, 'Unmapped', NULL);
COMMIT;
```

```
OK
OK
OK
OK
```

## The GEOMETRY type

`GEOMETRY` holds a value of any SRID, a signed 32-bit integer.
`GEOMETRY(srid)` holds only values of that SRID, empty ones included,
and its parameter is an integer from 0 to 2147483647. The SRID is a
label stored as given. It is not looked up, 0 does not mean WGS 84, and
no coordinates are transformed or axes exchanged.

A value is of one of seven kinds, POINT, LINESTRING, POLYGON,
MULTIPOINT, MULTILINESTRING, MULTIPOLYGON and GEOMETRYCOLLECTION. They
are kinds of one SQL type, not structured subtypes, and a column cannot
be limited to one kind. ST_GEOMETRYTYPE names a value's kind in the
spelling of SQL/MM Spatial, case-sensitive:

| Kind | ST_GEOMETRYTYPE |
|---|---|
| POINT | ST_Point |
| LINESTRING | ST_LineString |
| POLYGON | ST_Polygon |
| MULTIPOINT | ST_MultiPoint |
| MULTILINESTRING | ST_MultiLineString |
| MULTIPOLYGON | ST_MultiPolygon |
| GEOMETRYCOLLECTION | ST_GeomCollection |

```sql
SELECT ID, ST_ASTEXT(SHAPE) AS WKT, ST_GEOMETRYTYPE(SHAPE) AS KIND, ST_ISEMPTY(SHAPE) AS IS_EMPTY FROM PLACES ORDER BY ID;
```

```
           ID | WKT                                                          | KIND                             |     IS_EMPTY
--------------+--------------------------------------------------------------+----------------------------------+--------------
            1 | POINT (2 2)                                                  | ST_Point                         |            0
            2 | LINESTRING (-5 -5, 15 -5)                                    | ST_LineString                    |            0
            3 | POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (4 4, 6 4, 6 6, 4 6, 4 4)) | ST_Polygon                       |            0
            4 | POINT (5 5)                                                  | ST_Point                         |            0
            5 | POINT (10 5)                                                 | ST_Point                         |            0
            6 | POINT EMPTY                                                  | ST_Point                         |            1
            7 | NULL                                                         | NULL                             |         NULL
(7 rows)
```

An empty value, as `POINT EMPTY`, is a known geometry with no
coordinates, and keeps its kind and SRID. NULL is no value at all.

A value of another SRID is refused by a `GEOMETRY(srid)` column, as is a
parameter out of range:

```sql
INSERT INTO PLACES VALUES (8, 'Elsewhere', ST_POINT(1, 1, 4326));
CREATE TABLE MAPS (ID INTEGER, AREA GEOMETRY(2147483648));
ROLLBACK;
```

```
turborel: SQLCODE -301 (SQLSTATE 22000): invalid geometry: SRID 4326 does not match the column's required SRID 0
turborel: SQLCODE -103 (SQLSTATE 2A000): GEOMETRY accepts at most one parameter: an SRID from 0 to 2147483647 (at character 36)
OK
```

`CAST(text AS GEOMETRY)` reads WKT with SRID 0, and
`CAST(text AS GEOMETRY(srid))` with that SRID. A cast of a geometry to
`GEOMETRY(srid)` checks its SRID and changes nothing. A character string
is not stored in a GEOMETRY column by assignment, so WKT is read with a
constructor or CAST.

`=`, ORDER BY, GROUP BY, DISTINCT and UNIQUE compare the stored
representation, SRID, kind and coordinates in their order. Two values
are equal only if they are written alike, so this is not the spatial
equality of ST_Equals, and a line reversed is a different value. Byte
order and the sign of a zero do not count. The order of values is that
of their bytes, not a spatial one.

```sql
VALUES CASE WHEN ST_GEOMFROMTEXT('LINESTRING (0 0, 1 1)') = ST_GEOMFROMTEXT('LINESTRING (1 1, 0 0)')
    THEN 'equal' ELSE 'different' END;
```

```

-----------
 different
(1 row)
```

## Routines

| Routine | Result |
|---|---|
| `ST_POINT(x, y [, srid])` | A point of two finite numbers, SRID 0 by default |
| `ST_MAKEENVELOPE(min_x, min_y, max_x, max_y [, srid])` | A rectangular polygon of finite bounds, each minimum less than its maximum, SRID 0 by default |
| `ST_GEOMFROMTEXT(text [, srid])` | The geometry of a WKT text, SRID 0 by default |
| `ST_GEOMFROMWKB(binary [, srid])` | The geometry of a WKB string, SRID 0 by default |
| `ST_ASTEXT(geometry)` | Its WKT, without its SRID |
| `ST_ASBINARY(geometry)` | Its little-endian WKB, without its SRID |
| `ST_SRID(geometry)` | Its SRID |
| `ST_SETSRID(geometry, srid)` | The same coordinates with another SRID |
| `ST_X(point)`, `ST_Y(point)` | Its X or Y, DOUBLE PRECISION; NULL of POINT EMPTY |
| `ST_MINX(geometry)`, `ST_MAXX(geometry)` | Its least or greatest X, DOUBLE PRECISION; NULL of an empty value |
| `ST_MINY(geometry)`, `ST_MAXY(geometry)` | Its least or greatest Y, DOUBLE PRECISION; NULL of an empty value |
| `ST_ISEMPTY(geometry)` | INTEGER 1 if it has no coordinates, else 0 |
| `ST_GEOMETRYTYPE(geometry)` | Its kind's name, as `ST_Point` |
| `ST_INTERSECTS(a, b)` | INTEGER 1 if `a` and `b` share a point, else 0 |
| `ST_DISJOINT(a, b)` | INTEGER 1 if `a` and `b` share no point, else 0 |
| `ST_DISTANCE(a, b)` | The least planar distance between them, DOUBLE PRECISION; NULL if either is empty |

The routines are scalar functions, called where SQL.FUNCTION_CALLS is
ON, its default. Each gives NULL if an argument is NULL.

ST_X and ST_Y take a point only. ST_SETSRID relabels a value and is
right only where its coordinates are already of that reference system.
ST_MAKEENVELOPE makes a polygon of four numbers, and is not SQL/MM's
ST_Envelope, which derives a value's envelope. Reversed bounds, or a
zero-width rectangle, are refused, and ST_POINT makes a single point:

```sql
SELECT ID, ST_X(SHAPE) AS X, ST_Y(SHAPE) AS Y FROM PLACES WHERE ID IN (1, 5, 6, 7) ORDER BY ID;
SELECT ST_MINX(SHAPE) AS MIN_X, ST_MINY(SHAPE) AS MIN_Y, ST_MAXX(SHAPE) AS MAX_X, ST_MAXY(SHAPE) AS MAX_Y
    FROM PLACES WHERE ID = 3;
SELECT ST_X(SHAPE) FROM PLACES WHERE ID = 2;
VALUES ST_ASTEXT(ST_MAKEENVELOPE(0, 0, 0, 5));
```

```
           ID |                        X |                        Y
--------------+--------------------------+--------------------------
            1 |                      2E0 |                      2E0
            5 |                      1E1 |                      5E0
            6 |                     NULL |                     NULL
            7 |                     NULL |                     NULL
(4 rows)
                    MIN_X |                    MIN_Y |                    MAX_X |                    MAX_Y
--------------------------+--------------------------+--------------------------+--------------------------
                      0E0 |                      0E0 |                      1E1 |                      1E1
(1 row)
turborel: SQLCODE -301 (SQLSTATE 22000): invalid geometry: expected a POINT geometry
turborel: SQLCODE -301 (SQLSTATE 22000): invalid geometry: a rectangle needs a minimum strictly below its maximum on both axes
```

## Text and binary forms

WKT is read in any case, with decimal and exponent coordinates. A
MULTIPOINT's members are in parentheses, `MULTIPOINT ((1 2), (3 4))`.
WKB is read in either byte order, mixed orders within a collection too,
and written little-endian. POINT EMPTY is written in WKB as a pair of
NaN coordinates, the convention of
[GEOS](https://libgeos.org/specifications/wkb/), and no other
coordinate may be infinite or NaN.

Neither form carries the SRID, so a value is exported with ST_SRID
beside it and read back with that SRID given to the constructor:

```sql
SELECT ST_ASBINARY(SHAPE) AS WKB, ST_SRID(SHAPE) AS SRID FROM PLACES WHERE ID = 1;
SELECT ST_ASTEXT(ST_GEOMFROMWKB(ST_ASBINARY(SHAPE), ST_SRID(SHAPE))) AS ROUND_TRIP FROM PLACES WHERE ID = 2;
VALUES ST_ASTEXT(CAST('multipoint ((1 2), (3.5e1 4))' AS GEOMETRY));
VALUES ST_SRID(ST_SETSRID(ST_POINT(1, 2), 4326));
```

```
 WKB                                                          |         SRID
--------------------------------------------------------------+--------------
 010100000000000000000000400000000000000040                   |            0
(1 row)
 ROUND_TRIP
--------------------------------------------------------------
 LINESTRING (-5 -5, 15 -5)
(1 row)

--------------------------------------------------------------
 MULTIPOINT ((1 2), (35 4))
(1 row)

--------------
         4326
(1 row)
```

A reader refuses input cut short or followed by more, a line of one
point, a polygon ring of fewer than four coordinates or not closed, and
a WKB collection holding a member of the wrong kind. Z and M
coordinates, curves and the SRID and dimension flags of EWKB are not
supported. A reader checks a value's structure, not its topology, so a
ring may cross itself and a hole lie outside its shell:

```sql
VALUES ST_ASTEXT(ST_GEOMFROMTEXT('LINESTRING (1 1)'));
VALUES ST_ASTEXT(ST_GEOMFROMTEXT('POINT Z (1 2 3)'));
VALUES ST_ASTEXT(ST_GEOMFROMTEXT('POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))'));
```

```
turborel: SQLCODE -301 (SQLSTATE 22000): invalid geometry: a nonempty line needs at least two points
turborel: SQLCODE -900 (SQLSTATE 0A000): geometry does not support Z or M coordinates

--------------------------------------------------------------
 POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))
(1 row)
```

The functions take and give text and binary strings of at most 32,767
characters or octets. Larger values are read by the library's codecs,
within the limits of [Appendix D](../appendix/d-limits.md).

## Relationships

ST_INTERSECTS and ST_DISJOINT test the closed geometries, boundaries
included and polygon holes excluded. Of the field, the well and the gate
on its edge intersect it, and the pond in its hole and the road below
it do not. An empty value intersects nothing:

```sql
SELECT P.NAME, ST_INTERSECTS(P.SHAPE, F.SHAPE) AS MEETS, ST_DISJOINT(P.SHAPE, F.SHAPE) AS APART
    FROM PLACES P, PLACES F WHERE F.ID = 3 ORDER BY P.ID;
```

```
 NAME                 |        MEETS |        APART
----------------------+--------------+--------------
 Well                 |            1 |            0
 Road                 |            0 |            1
 Field                |            1 |            0
 Pond                 |            0 |            1
 Gate                 |            1 |            0
 Lost                 |            0 |            1
 Unmapped             |         NULL |         NULL
(7 rows)
```

Both values must have the same SRID, even when empty. A query writes
`ST_INTERSECTS(a, b) = 1` in a WHERE or ON condition. The tests decide
by the exact orientation of the binary64 coordinates as stored, without
a tolerance, in the plane, even where the SRID names a geographic
reference system. They are right only of polygons of valid topology,
which they do not check:

```sql
VALUES ST_INTERSECTS(ST_POINT(5, 5), ST_POINT(5, 5, 4326));
```

```
turborel: SQLCODE -301 (SQLSTATE 22000): invalid geometry: spatial operands must have the same SRID
```

## Distance

ST_DISTANCE gives the least distance between the points of two values,
in their coordinates' unit, 0 if they intersect. Of longitude and
latitude it gives degrees, not metres along the Earth. Its result is
approximate, and a positive distance too small for DOUBLE PRECISION may
round to 0, so ST_INTERSECTS is the test of contact. Both values must
have the same SRID, and polygons valid topology:

```sql
SELECT P.NAME, ST_DISTANCE(P.SHAPE, F.SHAPE) AS FROM_FIELD
    FROM PLACES P, PLACES F WHERE F.ID = 3 ORDER BY P.ID;
VALUES ST_DISTANCE(ST_POINT(-1E308, 0), ST_POINT(1E308, 0));
```

```
 NAME                 |               FROM_FIELD
----------------------+--------------------------
 Well                 |                      0E0
 Road                 |                      5E0
 Field                |                      0E0
 Pond                 |                      1E0
 Gate                 |                      0E0
 Lost                 |                     NULL
 Unmapped             |                     NULL
(7 rows)
turborel: SQLCODE -301 (SQLSTATE 22003): geometry distance exceeds the finite binary64 range
```

## The rtree index method

```
CREATE INDEX <index name> ON <table name> ( <column name> ) USING RTREE
```

An `rtree` index of one `GEOMETRY(srid)` column holds each value's
envelope, its bounding rectangle, and omits NULLs. A column of plain
GEOMETRY, which may mix SRIDs, cannot have one. The index is kept up to
date as rows change, and it is made outside a transaction, so the one
the examples began is ended first:

```sql
COMMIT;
CREATE INDEX PLACES_SHAPE ON PLACES (SHAPE) USING RTREE;
CREATE TABLE SKETCHES (ID INTEGER PRIMARY KEY, SHAPE GEOMETRY);
COMMIT;
CREATE INDEX SKETCHES_SHAPE ON SKETCHES (SHAPE) USING RTREE;
```

```
OK
OK
OK
OK
turborel: SQLCODE -900 (SQLSTATE 0A000): the rtree index method needs GEOMETRY(srid) for a geometry key
```

The planner uses the index where it costs less than reading the table,
which these few rows may not, in three forms.

- A filter `ST_INTERSECTS(column, value) = 1`, or with the column second,
  where the value is a constant or a parameter.
- An inner join on `ST_INTERSECTS(a.column, b.column) = 1`, where both
  columns are of the same SRID. Each row of one side probes the other's
  index, and for a broad join an adaptive scan may read it once instead.
- `ORDER BY ST_DISTANCE(column, value) ASC NULLS LAST` with FETCH FIRST,
  OFFSET too, where the value is a constant or a parameter, which reads
  the nearest rows first.

```sql
SELECT NAME FROM PLACES WHERE ST_INTERSECTS(SHAPE, ST_MAKEENVELOPE(0, 0, 3, 3)) = 1 ORDER BY ID;
SELECT NAME, ST_DISTANCE(SHAPE, ST_POINT(12, 5)) AS DISTANCE FROM PLACES
    ORDER BY ST_DISTANCE(SHAPE, ST_POINT(12, 5)) ASC NULLS LAST, ID FETCH FIRST 2 ROWS ONLY;
```

```
 NAME
----------------------
 Well
 Field
(2 rows)
 NAME                 |                 DISTANCE
----------------------+--------------------------
 Field                |                      2E0
 Gate                 |                      2E0
(2 rows)
```

An envelope finds candidates, and each is checked against the visible
row's exact geometry and the whole condition, so the rows are those a
scan gives, with the same errors. A query value of another SRID still
raises its error. ST_DISJOINT, distance comparisons in WHERE, outer
joins, and nearest queries descending, NULLS FIRST, WITH TIES or PERCENT
read the table. A filter that may raise an error keeps the index from
skipping rows. [CREATE INDEX](statements/create-index.md) gives the
index's syntax, and its numeric forms of X and Y columns.

## Errors

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 22000 | -301 | WKT or WKB is malformed, a coordinate is not finite, ST_MAKEENVELOPE's bounds are reversed or of no area, ST_X or ST_Y is given another kind than a point, or two values' SRIDs differ |
| 22000 | -301 | A value's SRID is not its column's or CAST's |
| 22003 | -301 | A distance is greater than DOUBLE PRECISION holds |
| 2A000 | -103 | GEOMETRY's parameter is not one integer from 0 to 2147483647, or a function's arguments are not of its types |
| 0A000 | -900 | Z or M coordinates, curves, EWKB flags, more than the limits of [Appendix D](../appendix/d-limits.md), or an `rtree` index of a column of plain GEOMETRY |
| TB916 | -916 | A stored value or index key is damaged, or of an unknown version |

## Standard

GEOMETRY, its functions and the `rtree` index method are extensions.
The kinds and their WKT and WKB forms are those of OGC 06-103r4,
Simple Feature Access Part 1, §§7–8, and the names ST_GEOMETRYTYPE
gives are SQL/MM Spatial's. Turborel claims conformance to neither.
GEOMETRY is a registered type, not a user-defined type, so it declares
no methods and its kinds are not subtypes. The choices made are listed
in [CONFORMANCE.md](../../CONFORMANCE.md#geometry-representations).

## See also

[CREATE INDEX](statements/create-index.md),
[Spatial Guide](../data/02-spatial.md),
[Appendix D: Limits](../appendix/d-limits.md),
[The Embedded Library](../client/03-embedding.md#spatial-data-extension)
