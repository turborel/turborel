# Spatial Data *(extension)*

[Contents](README.md)

A geometry value is a point, a curve, a surface, a solid or a
collection of them, with an SRID naming its spatial reference system.
Its type is ST_GEOMETRY or one of its 26 subtypes, the type family of
SQL/MM Spatial. Its
functions read and write the well-known text (WKT) and binary (WKB)
forms, observe a value's coordinates and bounds, test whether two values
intersect and measure the planar distance between them. An index of the
method `rtree` on a GEOMETRY column finds the rows an intersection
filter or join matches, and the rows nearest a point, without reading
the table.

The types and their functions are built in, so every database has
them and nothing is registered before its files are opened. For an
introduction with complete examples, read the
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

## The geometry types

`ST_GEOMETRY` holds a value of any type of the family and any SRID, a
signed 32-bit integer. `GEOMETRY` is another name of it. Each type
takes an SRID as its parameter, as `ST_POINT(4326)`, and then holds
only values of that SRID, empty ones included. The parameter is an
integer from 0 to 2147483647. The SRID is a label stored as given. It
is not looked up, 0 does not mean WGS 84, and no coordinates are
transformed or axes exchanged.

The types form a hierarchy. A value of a type is a value of each type
above it, and the types in italics hold no value of their own:

| Type | Subtypes |
|---|---|
| *ST_GEOMETRY* | ST_POINT, *ST_CURVE*, *ST_SURFACE*, ST_SOLID, ST_GEOMCOLLECTION |
| *ST_CURVE* | ST_LINESTRING, ST_CIRCULARSTRING, ST_GEODESICSTRING, ST_ELLIPTICALCURVE, ST_NURBSCURVE, ST_CLOTHOID, ST_SPIRALCURVE, ST_COMPOUNDCURVE |
| ST_CIRCULARSTRING | ST_CIRCLE |
| *ST_SURFACE* | ST_CURVEPOLYGON, ST_POLYHDRLSURFACE, ST_COMPOUNDSURFACE |
| ST_CURVEPOLYGON | ST_POLYGON |
| ST_POLYGON | ST_TRIANGLE |
| ST_POLYHDRLSURFACE | ST_TIN |
| ST_SOLID | ST_BREPSOLID |
| ST_GEOMCOLLECTION | ST_MULTIPOINT, ST_MULTICURVE, ST_MULTISURFACE |
| ST_MULTICURVE | ST_MULTILINESTRING |
| ST_MULTISURFACE | ST_MULTIPOLYGON |

A column, variable or parameter of a type takes a value of that type or
of a type below it. A value of a type above it is stored only through
`TREAT(value AS type)`, which raises SQLSTATE 0D000 of a value of
another type. `value IS OF (type, ...)` tests a value's type, and
`IS OF (ONLY type)` that type alone. `CAST(value AS type)` converts
within the hierarchy as SQL/MM Spatial's Table 4 says: a point becomes
a multipoint of one point, a collection of one element becomes that
element, an empty value becomes the empty value of the target, and
anything else raises SQLSTATE 2FF08. `NEW type(...)` is refused of a
type in italics.

ST_GEOMETRYTYPE names a value's most specific type in the spelling of
SQL/MM Spatial, case-sensitive, as `ST_Point`. The XY values of seven
types, ST_POINT, ST_LINESTRING, ST_POLYGON, ST_MULTIPOINT,
ST_MULTILINESTRING, ST_MULTIPOLYGON and ST_GEOMCOLLECTION, are related
to each other. A value of every instantiable type, with Z or M
coordinates too, is read and written as text, WKB and GML, and the
curves and surfaces have the methods of [Curves](#curves) and
[Surfaces](#surfaces).

```sql
CREATE TABLE MARKS (ID INTEGER, SPOT ST_POINT(0), ROUTE ST_CURVE(0));
INSERT INTO MARKS VALUES (1, ST_POINT(1, 1), TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 3 4)') AS ST_CURVE));
INSERT INTO MARKS (ID, SPOT) VALUES (2, ST_GEOMFROMTEXT('POINT (2 2)'));
INSERT INTO MARKS (ID, SPOT) VALUES (2, TREAT(ST_GEOMFROMTEXT('POINT (2 2)') AS ST_POINT));
SELECT ID, ST_GEOMETRYTYPE(ROUTE) AS ROUTE_TYPE, ROUTE IS OF (ST_LINESTRING) AS IS_LINE FROM MARKS ORDER BY ID;
VALUES ST_GEOMETRYTYPE(CAST(ST_POINT(1, 2) AS ST_MULTIPOINT));
VALUES CAST(ST_POINT(1, 2) AS ST_LINESTRING);
ROLLBACK;
```

```
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): cannot assign a ST_GEOMETRY value to column SPOT (ST_POINT(0)) [ISO/IEC 9075:1992 §13.8 SR6; ISO/IEC 9075:1992 §9.2 SR2] (at character 40)
OK
           ID | ROUTE_TYPE                       | IS_LINE
--------------+----------------------------------+---------
            1 | ST_LineString                    | TRUE
            2 | NULL                             | NULL
(2 rows)

----------------------------------
 ST_MultiPoint
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2FF08): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.5 Table 4] a value of ST_Point is not cast to ST_LineString
OK
```

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

The ordering of the types is EQUALS ONLY BY RELATIVE, with
ST_ORDERINGEQUALS. `=` and `<>` compare two values by it: they are
equal if they have one SRID and are written alike, or are both empty.
This is not the spatial equality of ST_Equals, and a line reversed is a
different value. Byte order and the sign of a zero do not count. ORDER
BY, GROUP BY, DISTINCT, `<` and the other orderings are refused of a
geometry value, and so are a PRIMARY KEY, UNIQUE or FOREIGN KEY of a
geometry column, as its values have no key of their own.

```sql
VALUES CASE WHEN ST_GEOMFROMTEXT('LINESTRING (0 0, 1 1)') = ST_GEOMFROMTEXT('LINESTRING (1 1, 0 0)')
    THEN 'equal' ELSE 'different' END;
SELECT DISTINCT SHAPE FROM PLACES;
```

```

-----------
 different
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): DISTINCT compares values of ST_GEOMETRY, which has an ordering of EQUALS ONLY (ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.65, ST_ORDERINGEQUALS) [ISO/IEC 9075-2:2023 §9.12 SR3]
```

## Routines

| Routine | Result |
|---|---|
| `ST_POINT(x, y [, srid])` | A point of two finite numbers, SRID 0 by default |
| `ST_MAKEENVELOPE(min_x, min_y, max_x, max_y [, srid])` | A rectangular polygon of finite bounds, each minimum less than its maximum, SRID 0 by default |
| `ST_GEOMFROMTEXT(text [, srid])` | The geometry of a WKT text, a character string or CLOB, SRID 0 by default |
| `ST_GEOMFROMWKB(binary [, srid])` | The geometry of a WKB string, a binary string or BLOB, of any type and coordinate dimension, SRID 0 by default |
| `ST_ASTEXT(geometry)` | Its WKT, without its SRID, a CHARACTER LARGE OBJECT |
| `ST_ASBINARY(geometry)` | Its little-endian WKB, without its SRID, a BINARY LARGE OBJECT |
| `ST_SRID(geometry)` | Its SRID |
| `ST_SETSRID(geometry, srid)` | The same coordinates with another SRID |
| `ST_X(point)`, `ST_Y(point)` | Its X or Y, DOUBLE PRECISION; NULL of POINT EMPTY |
| `ST_MINX(geometry)`, `ST_MAXX(geometry)` | Its least or greatest X, DOUBLE PRECISION; NULL of an empty value |
| `ST_MINY(geometry)`, `ST_MAXY(geometry)` | Its least or greatest Y, DOUBLE PRECISION; NULL of an empty value |
| `ST_ISEMPTY(geometry)` | INTEGER 1 if it has no coordinates, else 0 |
| `ST_GEOMETRYTYPE(geometry)` | Its most specific type's name, as `ST_Point` |
| `ST_ORDERINGEQUALS(a, b)` | INTEGER 0 if `a` and `b` are equal by the ordering of the types, else 1 |
| `ST_INTERSECTS(a, b)` | INTEGER 1 if `a` and `b` share a point, else 0 |
| `ST_DISJOINT(a, b)` | INTEGER 1 if `a` and `b` share no point, else 0 |
| `ST_DISTANCE(a, b)` | The least planar distance between them, DOUBLE PRECISION; NULL if either is empty |
| `ST_NUMGEOMETRIES(collection)` | The count of its elements, INTEGER |
| `ST_GEOMETRYN(collection, n)` | Its `n`th element, from 1; NULL and the warning 01F01 if there is none |
| `ST_GEOMETRIES(collection)` | Its elements, an ARRAY of the collection's element type |

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

## Methods

Each routine of a value is also a method of its type, invoked as
`value.ST_X()`, and a type has the methods of its supertypes. A method
is found by the declared type of the value. ST_X of a column declared
ST_GEOMETRY is refused until TREAT makes it an ST_POINT. A method that a
subtype overrides runs as the value's most specific type chooses:
ST_GEOMETRIES of an ST_MULTIPOINT returns an ST_POINT ARRAY, whose
elements have ST_X. `(value AS type).method()` runs the method of a
supertype instead, here ST_GEOMCOLLECTION's, whose ARRAY is of
ST_GEOMETRY, so its elements have no ST_X. `value.SPECIFICTYPE` names
the value's most specific type:

```sql
CREATE TABLE GROUPS (ID INTEGER, PTS ST_MULTIPOINT);
INSERT INTO GROUPS VALUES (1, TREAT(ST_GEOMFROMTEXT('MULTIPOINT ((1 2), (3 4))') AS ST_MULTIPOINT));
SELECT PTS.ST_NUMGEOMETRIES() AS N, PTS.ST_GEOMETRIES()[2].ST_X() AS X, PTS.SPECIFICTYPE AS T FROM GROUPS;
SELECT (PTS AS ST_GEOMCOLLECTION).ST_GEOMETRIES()[1].ST_X() FROM GROUPS;
SELECT (PTS AS ST_GEOMCOLLECTION).ST_GEOMETRIES()[1].ST_GEOMETRYTYPE() AS T FROM GROUPS;
ROLLBACK;
```

```
OK
OK
            N |                        X | T
--------------+--------------------------+--------------------------------------------------------------
            2 |                      3E0 | "TURBOREL"."INFORMATION_SCHEMA"."ST_MULTIPOINT"
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): ST_Geometry and its supertypes have no method ST_X of 0 arguments [ISO/IEC 9075-2:2023 §9.18 SR10b, §9.19 SR3] (at character 52)
 T
----------------------------------
 ST_Point
(1 row)
OK
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
a WKB collection holding a member of the wrong kind. Text and WKB carry
Z and M coordinates and every one of the 23 instantiable types. The SRID
and dimension flags of EWKB are not supported. A reader checks a
value's structure, not its topology, so a ring may cross itself and a
hole lie outside its shell:

```sql
VALUES ST_ASTEXT(ST_GEOMFROMTEXT('LINESTRING (1 1)'));
VALUES ST_ASTEXT(ST_GEOMFROMTEXT('POINT Z (1 2 3)'));
VALUES ST_ASTEXT(ST_GEOMFROMTEXT('POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))'));
```

```
turborel: SQLCODE -301 (SQLSTATE 2FF02): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.2] a curve of points that is not empty has at least two

--------------------------------------------------------------
 POINT Z (1 2 3)
(1 row)

--------------------------------------------------------------
 POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))
(1 row)
```

The functions take and give text and binary strings of at most 32,767
characters or octets. Larger values are read by the library's codecs,
within the limits of [Appendix D](../appendix/d-limits.md).

Each instantiable type has its own constructors of each form, which
return the value as that type and refuse a value of another with the
form's SQLSTATE, 2FF22 for text, 2FF23 for WKB and 2FF24 for GML.
ST_WKTTOSQL and ST_WKBTOSQL read any type of SRID 0:

| Type | Text | WKB | GML |
|---|---|---|---|
| ST_Point | `ST_POINTFROMTEXT` | `ST_POINTFROMWKB` | `ST_POINTFROMGML` |
| ST_LineString | `ST_LINEFROMTEXT` | `ST_LINEFROMWKB` | `ST_LINEFROMGML` |
| ST_CircularString | `ST_CIRCULARFROMTXT` | `ST_CIRCULARFROMWKB` | `ST_CIRCULARFROMGML` |
| ST_Circle | `ST_CIRCLEFROMTXT` | `ST_CIRCLEFROMWKB` | `ST_CIRCLEFROMGML` |
| ST_GeodesicString | `ST_GEODESICFROMTXT` | `ST_GEODESICFROMWKB` | `ST_GEODESICFROMGML` |
| ST_EllipticalCurve | `ST_ELLIPTICFROMTXT` | `ST_ELLIPTICFROMWKB` | `ST_ELLIPTICFROMGML` |
| ST_NURBSCurve | `ST_NURBSFROMTXT` | `ST_NURBSFROMWKB` | `ST_NURBSFROMGML` |
| ST_Clothoid | `ST_CLOTHOIDFROMTXT` | `ST_CLOTHOIDFROMWKB` | `ST_CLOTHOIDFROMGML` |
| ST_SpiralCurve | `ST_SPIRALFROMTXT` | `ST_SPIRALFROMWKB` | `ST_SPIRALFROMGML` |
| ST_CompoundCurve | `ST_COMPOUNDFROMTXT` | `ST_COMPOUNDFROMWKB` | `ST_COMPOUNDFROMGML` |
| ST_CurvePolygon | `ST_CPOLYFROMTEXT` | `ST_CPOLYFROMWKB` | `ST_CPOLYFROMGML` |
| ST_Polygon | `ST_POLYFROMTEXT` | `ST_POLYFROMWKB` | `ST_POLYFROMGML` |
| ST_Triangle | `ST_TRIFROMTEXT` | `ST_TRIFROMWKB` | `ST_TRIFROMGML` |
| ST_PolyhdrlSurface | `ST_PHSFROMTEXT` | `ST_PHSFROMWKB` | `ST_PHSFROMGML` |
| ST_TIN | `ST_TINFROMTEXT` | `ST_TINFROMWKB` | `ST_TINFROMGML` |
| ST_CompoundSurface | `ST_COMPSURFFROMTXT` | `ST_COMPSURFFROMWKB` | `ST_COMPSURFFROMGML` |
| ST_BRepSolid | `ST_BREPFROMTEXT` | `ST_BREPFROMWKB` | `ST_BREPFROMGML` |
| ST_GeomCollection | `ST_GEOMCOLLFROMTXT` | `ST_GEOMCOLLFROMWKB` | `ST_GEOMCOLLFROMGML` |
| ST_MultiPoint | `ST_MPOINTFROMTEXT` | `ST_MPOINTFROMWKB` | `ST_MPOINTFROMGML` |
| ST_MultiCurve | `ST_MCURVEFROMTEXT` | `ST_MCURVEFROMWKB` | `ST_MCURVEFROMGML` |
| ST_MultiLineString | `ST_MLINEFROMTEXT` | `ST_MLINEFROMWKB` | `ST_MLINEFROMGML` |
| ST_MultiSurface | `ST_MSURFACEFROMTXT` | `ST_MSURFACEFROMWKB` | `ST_MSURFACEFROMGML` |
| ST_MultiPolygon | `ST_MPOLYFROMTEXT` | `ST_MPOLYFROMWKB` | `ST_MPOLYFROMGML` |

```sql
VALUES ST_ASTEXT(ST_CIRCULARFROMTXT('CIRCULARSTRING Z (0 0 1, 1 1 2, 2 0 3)', 4326));
VALUES ST_GEOMETRYTYPE(ST_WKBTOSQL(ST_ASBINARY(ST_TRIFROMTEXT('TRIANGLE (0 0, 1 0, 0 1)'))));
VALUES ST_POINTFROMTEXT('LINESTRING (0 0, 1 1)');
```

```

--------------------------------------------------------------
 CIRCULARSTRING Z (0 0 1, 1 1 2, 2 0 3)
(1 row)

----------------------------------
 ST_Triangle
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2FF22): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.62] ST_POINTFROMTEXT reads a ST_LineString, no ST_Point
```

## GML

ST_ASGML writes a value's GML 3.2, and ST_GMLTOSQL, ST_GEOMFROMGML and
each type's FromGML read it. A value's SRID is written as the srsName
of its reference system, the organization and its identifier there, or
else the system's definition. A reader finds the SRID of an srsName
among the systems of the Spatial Information Schema. ST_GEOMFROMGML
and the typed readers take an SRID too, which replaces it. GML has no M
coordinate, so a value's measures are left out:

```sql
VALUES ST_ASGML(ST_POINTFROMTEXT('POINT (1 2)', 4326));
VALUES ST_SRID(ST_GMLTOSQL('<gml:Point srsName="EPSG:4326"><gml:pos>1 2</gml:pos></gml:Point>'));
VALUES ST_ASTEXT(ST_LINEFROMGML(ST_ASGML(ST_LINEFROMTEXT('LINESTRING M (0 0 5, 3 4 6)'))));
```

```

--------------------------------------------------------------
 <gml:Point xmlns:gml="http://www.opengis.net/gml/3.2" srsName="EPSG:4326"><gml:pos>1 2</gml:pos></gml:Point>
(1 row)

--------------
         4326
(1 row)

--------------------------------------------------------------
 LINESTRING (0 0, 3 4)
(1 row)
```

## Curves

Every curve has a length, end points and a linear form. Its arcs,
splines, clothoids and spirals are measured exactly or by quadrature,
as [CONFORMANCE.md](../../CONFORMANCE.md#geometry-representations)
describes. A method of a position, as ST_POINTN, gives NULL and the
warning 01F01 where there is none.

| Method | Of | Result |
|---|---|---|
| `ST_LENGTH()` | ST_Curve | Its length, DOUBLE PRECISION |
| `ST_STARTPOINT()`, `ST_ENDPOINT()` | ST_Curve | Its first or last point |
| `ST_ISCLOSED()`, `ST_ISRING()` | ST_Curve | INTEGER 1 if its ends meet, and if it is also simple |
| `ST_CURVETOLINE()` | ST_Curve | Its linear form, an ST_LineString |
| `ST_POINTATDISTANCE(d)` | ST_Curve | The point `d` along it; 2FF79 beyond its end |
| `ST_DISTANCETOPOINT(p)` | ST_Curve | How far along it `p` lies; 2FF80 off it |
| `ST_PERPPOINTS(p)` | ST_Curve | The perpendicular projections of `p` on it, a point, a curve or a collection |
| `ST_POINTS()`, `ST_NUMPOINTS()`, `ST_POINTN(n)` | ST_LineString, ST_CircularString, ST_GeodesicString, ST_Circle | Its points, their count, the `n`th |
| `ST_NUMSEGMENTS()`, `ST_SEGMENTN(n)`, `ST_MIDPOINTREP()` | ST_CircularString | Its arcs' count, the `n`th arc, its points with each arc's middle |
| `ST_CENTER()`, `ST_RADIUS()`, `ST_BULGE()` | ST_CircularString of one arc, ST_Circle | Its center, radius or signed bulge; 2FF76 of more arcs |
| `ST_STARTM()`, `ST_ENDM()` | ST_EllipticalCurve, ST_NURBSCurve, ST_Clothoid, ST_SpiralCurve | Its first or last measure |
| `ST_DEGREE()` | ST_NURBSCurve | Its degree |
| `ST_SCALEFACTOR()`, `ST_STARTDISTANCE()`, `ST_ENDDISTANCE()` | ST_Clothoid | Its parameters |
| `ST_STARTCURVATURE()`, `ST_ENDCURVATURE()`, `ST_SPIRALTYPE()` | ST_SpiralCurve | Its parameters |
| `ST_CURVES()`, `ST_NUMCURVES()`, `ST_CURVEN(n)` | ST_CompoundCurve | Its parts, their count, the `n`th |

```sql
CREATE TABLE ROADS (ID INTEGER, PATH ST_CURVE);
INSERT INTO ROADS VALUES (1, ST_COMPOUNDFROMTXT('COMPOUNDCURVE (LINESTRING (0 0, 1 0), CIRCULARSTRING (1 0, 2 1, 3 0))'));
SELECT CAST(PATH.ST_LENGTH() AS DECIMAL(9, 4)) AS LEN, PATH.ST_ISCLOSED() AS CLOSED,
    ST_ASTEXT(PATH.ST_ENDPOINT()) AS LAST_POINT FROM ROADS;
SELECT ST_ASTEXT(TREAT(PATH AS ST_COMPOUNDCURVE).ST_CURVEN(2)) AS ARC FROM ROADS;
SELECT TREAT(PATH AS ST_COMPOUNDCURVE).ST_CURVEN(3) AS NONE FROM ROADS;
ROLLBACK;
```

```
OK
OK
         LEN |       CLOSED | LAST_POINT
-------------+--------------+--------------------------------------------------------------
      4.1416 |            0 | POINT (3 0)
(1 row)
 ARC
--------------------------------------------------------------
 CIRCULARSTRING (1 0, 2 1, 3 0)
(1 row)
 NONE
--------------------------------------------------------------
 NULL
(1 row, with a warning: SQLSTATE 01F01)
OK
```

## Surfaces

Every surface and multisurface has an area, a perimeter, a centroid and
a point on it, measured in the coordinate plane. A polygon has its
rings, a polyhedral surface and a TIN their patches, and a compound
surface its parts.

| Method | Of | Result |
|---|---|---|
| `ST_AREA()`, `ST_PERIMETER()` | ST_Surface, ST_MultiSurface | DOUBLE PRECISION |
| `ST_CENTROID()`, `ST_POINTONSURFACE()` | ST_Surface, ST_MultiSurface | An ST_Point |
| `ST_EXTERIORRING()`, `ST_INTERIORRINGS()`, `ST_NUMINTERIORRING()`, `ST_INTERIORRINGN(n)` | ST_CurvePolygon, ST_Polygon, ST_Triangle | Its rings, as curves, or line strings of a polygon |
| `ST_CURVEPOLYTOPOLY()` | ST_CurvePolygon | Its linear form, an ST_Polygon |
| `ST_PATCHES()`, `ST_NUMPATCHES()`, `ST_PATCHN(n)` | ST_PolyhdrlSurface, ST_TIN | Its patches, polygons or triangles |
| `ST_SURFACES()`, `ST_NUMSURFACES()`, `ST_SURFACEN(n)` | ST_CompoundSurface | Its parts |

```sql
VALUES CAST(ST_CPOLYFROMTEXT('CURVEPOLYGON (CIRCULARSTRING (0 0, 2 2, 4 0, 2 -2, 0 0))').ST_AREA() AS DECIMAL(9, 4));
VALUES ST_POLYFROMTEXT('POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 1 2, 2 2, 2 1, 1 1))').ST_PERIMETER();
VALUES ST_ASTEXT(ST_POLYFROMTEXT('POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 1 2, 2 2, 2 1, 1 1))').ST_INTERIORRINGN(1));
VALUES ST_ASTEXT(ST_MPOLYFROMTEXT('MULTIPOLYGON (((0 0, 1 0, 1 1, 0 1, 0 0)), ((2 0, 3 0, 3 1, 2 1, 2 0)))').ST_CENTROID());
```

```

-------------
     12.5664
(1 row)

--------------------------
                      2E1
(1 row)

--------------------------------------------------------------
 LINESTRING (1 1, 1 2, 2 2, 2 1, 1 1)
(1 row)

--------------------------------------------------------------
 POINT (1.5 0.5)
(1 row)
```

## TINs

A TIN is built from its elements, of the type ST_TINELEMENT, as their
constrained Delaunay triangulation. Random points and spots are its
vertices, breaklines and boundaries its edges, and voids and holes are
cut out of it. A maximum side length above 0 removes the triangles of
longer outer sides. ST_TIN takes the triangles too, and keeps them as
they are.

| Routine | Result |
|---|---|
| `ST_TINELEMENT(type, [id,] tag, geometry)` | An element, its type one of `random points`, `group spot`, `boundary`, `breakline`, `soft break`, `control contour`, `break void`, `drape void`, `void`, `hole` and `stop line`; 2FF74 of a geometry the type does not take |
| `ST_ELEMENTTYPE(e)`, `ST_ELEMENTID(e)`, `ST_ELEMENTTAG(e)`, `ST_ELEMENTGEOMETRY(e)` | Its parts, and of a second argument the element with that part replaced |
| `ST_TIN(elements, max_side [, srid])` | The triangulation of an ARRAY of elements |
| `ST_TIN(triangles, elements, max_side [, srid])` | The TIN of an ARRAY of triangles and one of elements |
| `tin.ST_TINELEMENTS()`, `tin.ST_MAXSIDELENGTH()` | Its elements, an ARRAY, and its maximum side length |

```sql
VALUES ST_TIN(ARRAY[ST_TINELEMENT('random points', 'spots',
    ST_MPOINTFROMTEXT('MULTIPOINT Z ((0 0 0), (4 0 1), (4 4 2), (0 4 3), (2 2 5))'))], 0E0).ST_NUMPATCHES();
VALUES ST_ELEMENTTAG(ST_TIN(ARRAY[ST_TINELEMENT('random points', 'spots',
    ST_MPOINTFROMTEXT('MULTIPOINT Z ((0 0 0), (4 0 1), (4 4 2))'))], 0E0).ST_TINELEMENTS()[1]);
VALUES ST_TINELEMENT('breakline', 'edge', ST_LINEFROMTEXT('LINESTRING (0 0, 1 1)'));
```

```

--------------
            4
(1 row)

--------------------------------------------------------------
 spots
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2FF74): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.1.2 Description 2a] a breakline element is of an ST_LineString with z
```

## Measures

A value with M coordinates locates its measures. ST_LOCATEBETWEEN(a, b)
gives the parts whose M lies from `a` to `b`, interpolated along each
curve, without Z. ST_LOCATEALONG(m) is ST_LOCATEBETWEEN(m, m). The parts
are a multipoint, a multicurve or a collection of both, and the empty
point where there are none. A surface's measures are those of its rings:

```sql
VALUES ST_ASTEXT(ST_LINEFROMTEXT('LINESTRING M (0 0 0, 10 0 10, 10 10 20)').ST_LOCATEBETWEEN(5E0, 15E0));
VALUES ST_ASTEXT(ST_LINEFROMTEXT('LINESTRING M (0 0 0, 10 0 10, 10 10 20)').ST_LOCATEALONG(12E0));
VALUES ST_LINEFROMTEXT('LINESTRING M (0 0 0, 10 0 10)').ST_LOCATEBETWEEN(2E0, 1E0);
```

```

--------------------------------------------------------------
 MULTICURVE M ((5 0 5, 10 0 10, 10 5 15))
(1 row)

--------------------------------------------------------------
 MULTIPOINT M ((10 2 12))
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2FF02): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.15] the end measure is less than the start measure
```

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

## The Spatial Information Schema

The schema ST_INFORMTN_SCHEMA holds SQL/MM Spatial's views, and SELECT
on each is everyone's.

| View | Rows |
|---|---|
| ST_GEOMETRY_COLUMNS | Each column of a geometry type the user may see, with the name and identifier of the system its type names, as `ST_POINT(4326)` names WGS 84. Both are NULL of a type that names no system, or one not supplied |
| ST_SPATIAL_REFERENCE_SYSTEMS | The spatial reference systems supplied, each with its name, identifier, organization and well-known text |
| ST_UNITS_OF_MEASURE | The units of length and angle, each with its factor to the metre or the radian |
| ST_SIZINGS | Each implementation-defined limit, 0 if there is none and NULL if what it limits is not supported |
| GEOMETRY_COLUMNS, SPATIAL_REF_SYS, ST_UNITS | The first three under short names |

The systems supplied are WGS 84 (4326), its geocentric form (4978),
NAD83 (4269), ETRS89 (4258), WGS 84 / Pseudo-Mercator (3857) and the
120 WGS 84 UTM zones (32601 to 32660 and 32701 to 32760), by the EPSG
identifiers. The tables the views read are in ST_DEFINITION_SCHEMA,
which no statement reads directly.

A system of its own is defined by `CALL ST_DEFINESRS(name, id,
organization, organization_id, definition, description)` and dropped by
`CALL ST_DROPSRS(name)` *(extension)*. A column is registered with a
system by `CALL ST_REGISTERGEOMETRYCOLUMN(schema, table, column, name)`
and unregistered by `CALL ST_UNREGISTERGEOMETRYCOLUMN(schema, table,
column)` *(extension)*. Each is a change to the catalog, made in the
transaction and undone by ROLLBACK. Each keeps the constraints of the
tables: a name and an identifier unique, an organization and its
identifier both given or both NULL, a column registered with a system
that is there and of its SRID if its type names one, a system no column
is registered with dropped. A column's owner registers it. A
registration goes with its column or table.

```sql
CREATE TABLE TRACKS (ID INTEGER, PATH ST_LINESTRING(4326), PLAN ST_GEOMETRY);
SELECT TABLE_NAME, COLUMN_NAME, SRS_NAME, SRS_ID FROM ST_INFORMTN_SCHEMA.ST_GEOMETRY_COLUMNS WHERE TABLE_NAME = 'TRACKS' ORDER BY COLUMN_NAME;
SELECT SRID, AUTH_NAME, AUTH_ID FROM ST_INFORMTN_SCHEMA.SPATIAL_REF_SYS WHERE SRS_NAME = 'WGS 84 / UTM zone 33N';
SELECT UNIT_NAME, CONVERSION_FACTOR FROM ST_INFORMTN_SCHEMA.ST_UNITS_OF_MEASURE WHERE UNIT_TYPE = 'LINEAR' ORDER BY CONVERSION_FACTOR;
ROLLBACK;
```

```
OK
 TABLE_NAME                                                   | COLUMN_NAME                                                  | SRS_NAME                                                     |       SRS_ID
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------
 TRACKS                                                       | PATH                                                         | WGS 84                                                       |         4326
 TRACKS                                                       | PLAN                                                         | NULL                                                         |         NULL
(2 rows)
         SRID | AUTH_NAME                                                    |      AUTH_ID
--------------+--------------------------------------------------------------+--------------
        32633 | EPSG                                                         |        32633
(1 row)
 UNIT_NAME                                                    |        CONVERSION_FACTOR
--------------------------------------------------------------+--------------------------
 FOOT                                                         |                 3.048E-1
 US SURVEY FOOT                                               |     3.048006096012192E-1
 METRE                                                        |                      1E0
 KILOMETRE                                                    |                      1E3
 STATUTE MILE                                                 |               1.609344E3
 NAUTICAL MILE                                                |                  1.852E3
(6 rows)
OK
```

A system a column is registered with is not dropped:

```sql
CREATE TABLE PLOTS (ID INTEGER, AREA ST_POLYGON);
CALL ST_DEFINESRS('FIELD GRID', 900001, 'ANN', 1, 'LOCAL_CS["field",LOCAL_DATUM["site",0],UNIT["metre",1]]', 'the field');
CALL ST_REGISTERGEOMETRYCOLUMN('ANN', 'PLOTS', 'AREA', 'FIELD GRID');
SELECT COLUMN_NAME, SRS_NAME, SRS_ID FROM ST_INFORMTN_SCHEMA.ST_GEOMETRY_COLUMNS WHERE TABLE_NAME = 'PLOTS';
CALL ST_DROPSRS('FIELD GRID');
ROLLBACK;
```

```
OK
OK
OK
 COLUMN_NAME                                                  | SRS_NAME                                                     |       SRS_ID
--------------------------------------------------------------+--------------------------------------------------------------+--------------
 AREA                                                         | FIELD GRID                                                   |       900001
(1 row)
turborel: SQLCODE -201 (SQLSTATE 23000): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20] ST_GEOMETRY_COLUMNS: SRS_SUPPORTED: column AREA of ANN.PLOTS is registered with FIELD GRID
OK
```

## Errors

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 22000 | -301 | WKT or WKB is malformed, a coordinate is not finite, ST_MAKEENVELOPE's bounds are reversed or of no area, ST_X or ST_Y is given another kind than a point, or two values' SRIDs differ |
| 22000 | -301 | A value's SRID is not its column's or CAST's |
| 22003 | -301 | A distance is greater than DOUBLE PRECISION holds |
| 2A000 | -103 | A type's parameter is not one integer from 0 to 2147483647, a function's arguments are not of its types, a value of a supertype is assigned without TREAT, NEW names a type that holds no value of its own, geometry values are ordered, grouped or made distinct, a key names a geometry column, or a method is not of the value's declared type |
| 0D000 | -301 | TREAT is given a value of another type |
| 2FF08 | -301 | CAST is given a value Table 4 does not convert, or a value is assigned to a type it is not of |
| 2FF22 | -301 | A text is malformed, or a typed constructor reads a value of another type |
| 2FF23 | -301 | WKB of a type is malformed, or a typed constructor reads a value of another type |
| 2FF24 | -301 | GML is malformed, or a typed constructor reads a value of another type |
| 2FF02 | -301 | A method is given a value or an argument it does not take, as a reversed measure range |
| 2FF74 | -301 | A TIN element's geometry is not of the type its element type takes |
| 2FF76, 2FF79, 2FF80 | -301 | ST_CENTER of more than one arc, ST_POINTATDISTANCE beyond a curve's end, ST_DISTANCETOPOINT of a point off it |
| 01F01 | warning | A method of a position is given one outside the value |
| 0A000 | -900 | Relationships of a value of a type beyond the seven, EWKB flags, more than the limits of [Appendix D](../appendix/d-limits.md), or an `rtree` index of a column of plain GEOMETRY |
| 23000 | -201 | ST_DEFINESRS, ST_DROPSRS or ST_REGISTERGEOMETRYCOLUMN would break a constraint of the Spatial Information Schema's tables |
| TB916 | -916 | A stored value or index key is damaged, or of an unknown version |

## Standard

The types, their hierarchy, their ordering, their WKB forms and the
Spatial Information Schema are those of ISO/IEC 13249-3:2016, SQL/MM
Spatial, §4.2, §5.1.65, §5.1.68, §19 and §20. The text forms are those
of OGC 06-103r4, Simple Feature Access Part 1, §§7–8. The functions
named here are called as functions, or as methods, as
`ST_POINT(1, 2).ST_X()`. Turborel claims conformance to neither yet, as
most of SQL/MM Spatial's routines are not there. The `rtree` index
method is an extension. The choices made are listed in
[CONFORMANCE.md](../../CONFORMANCE.md#geometry-representations).

## See also

[CREATE INDEX](statements/create-index.md),
[Spatial Guide](../data/02-spatial.md),
[Appendix D: Limits](../appendix/d-limits.md)
