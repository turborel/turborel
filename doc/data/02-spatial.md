# Spatial Data

[Specialized Data](README.md)

A bicycle delivery service keeps its orders' addresses as points, its
couriers' routes as lines and its service areas as polygons. Its
dispatchers need the orders on a map, the areas that serve each order,
the routes that roadworks close and the depots nearest a courier. This
guide asks those questions in SQL, with the GEOMETRY type, its functions
and the R-tree indexes that find shapes without reading every row. The
[reference](../sql/10-spatial.md) gives every rule.

GEOMETRY is in `turboreld`, which registers it before it opens its files.
A program embedding the library registers it itself
([The Embedded Library](../client/03-embedding.md#spatial-data-extension)).

## The deliveries

The places are fictional, in a local grid whose X runs east and Y north,
one unit a metre. The grid has no assigned reference identifier, so its
values have SRID 0. The metre is the service's own convention, as SRID 0
names no unit. Each column is of `GEOMETRY(0)`, which holds only values
of SRID 0, so it can have an R-tree index. An index is made outside a
transaction, so the rows are committed first:

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE DELIVERIES (
    ID INTEGER PRIMARY KEY,
    CUSTOMER VARCHAR(20) NOT NULL,
    STATUS VARCHAR(10) NOT NULL,
    LOCATION GEOMETRY(0)
);
CREATE TABLE ZONES (
    ID INTEGER PRIMARY KEY,
    NAME VARCHAR(10) NOT NULL,
    PRIORITY INTEGER NOT NULL,
    BOUNDARY GEOMETRY(0) NOT NULL
);
CREATE TABLE DEPOTS (ID INTEGER PRIMARY KEY, NAME VARCHAR(20) NOT NULL, LOCATION GEOMETRY(0) NOT NULL);
CREATE TABLE ROUTES (ID INTEGER PRIMARY KEY, NAME VARCHAR(20) NOT NULL, PATH GEOMETRY(0) NOT NULL);
INSERT INTO DELIVERIES VALUES
    (1, 'Museum shop', 'ready', ST_POINT(100, 100)),
    (2, 'University library', 'ready', ST_POINT(500, 500)),
    (3, 'Harbour cafe', 'ready', ST_POINT(900, 100)),
    (4, 'Market stall', 'ready', ST_POINT(700, 300)),
    (5, 'Hill florist', 'ready', ST_POINT(1400, 200)),
    (6, 'New customer', 'ready', NULL),
    (7, 'Warehouse', 'delivered', ST_POINT(200, 200)),
    (8, 'Station kiosk', 'ready', ST_POINT(1000, 100));
INSERT INTO ZONES VALUES
    (1, 'Central', 20, ST_GEOMFROMTEXT(
        'POLYGON ((0 0, 1000 0, 1000 1000, 0 1000, 0 0), (400 400, 400 600, 600 600, 600 400, 400 400))')),
    (2, 'East', 10, ST_MAKEENVELOPE(1000, 0, 2000, 1000)),
    (3, 'North', 30, ST_MAKEENVELOPE(0, 1000, 1000, 2000));
INSERT INTO DEPOTS VALUES
    (1, 'Canal depot', ST_POINT(100, 700)),
    (2, 'Market depot', ST_POINT(1000, 300)),
    (3, 'South depot', ST_POINT(400, -300)),
    (4, 'North depot', ST_POINT(400, 1100));
INSERT INTO ROUTES VALUES
    (1, 'Canal route', ST_GEOMFROMTEXT('LINESTRING (0 200, 1000 200)')),
    (2, 'North route', ST_GEOMFROMTEXT('LINESTRING (0 800, 1000 800)')),
    (3, 'East route', ST_GEOMFROMTEXT('LINESTRING (1200 0, 1200 1000)'));
COMMIT;
CREATE INDEX DELIVERIES_LOCATION ON DELIVERIES (LOCATION) USING RTREE;
CREATE INDEX ZONES_BOUNDARY ON ZONES (BOUNDARY) USING RTREE;
CREATE INDEX DEPOTS_LOCATION ON DEPOTS (LOCATION) USING RTREE;
CREATE INDEX ROUTES_PATH ON ROUTES (PATH) USING RTREE;
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
OK
OK
OK
OK
```

The indexes are kept up to date as rows change. Each order's address,
in well-known text:

```sql
SELECT CUSTOMER, STATUS, ST_ASTEXT(LOCATION) AS WKT FROM DELIVERIES ORDER BY ID;
```

```
 CUSTOMER             | STATUS     | WKT
----------------------+------------+--------------------------------------------------------------
 Museum shop          | ready      | POINT (100 100)
 University library   | ready      | POINT (500 500)
 Harbour cafe         | ready      | POINT (900 100)
 Market stall         | ready      | POINT (700 300)
 Hill florist         | ready      | POINT (1400 200)
 New customer         | ready      | NULL
 Warehouse            | delivered  | POINT (200 200)
 Station kiosk        | ready      | POINT (1e3 100)
(8 rows)
```

Central has a hole, a courtyard closed to bicycles, and the University
library is in it. The Station kiosk is on the edge Central shares with
East. The new customer has given no address yet, and the Warehouse's
order is delivered. North has no orders.

## Coordinates and reference systems

A pair of coordinates has no unit of its own. `(4.9, 52.37)` may be a
longitude and latitude in degrees, and `(100, 200)` metres on a map or
millimetres on a machine's bed. Values compared with each other must be
of one coordinate system, and an SRID labels which.

Turborel stores the SRID as given. It does not look it up, check the
coordinates' range, swap axes or transform coordinates, and SRID 0 does
not mean WGS 84. Giving a value SRID 4326 does not correct axes the
wrong way round, and ST_SETSRID does not reproject. Of longitude and
latitude, a distance is in degrees, intersection is tested in the plane,
and a window across the antimeridian is two windows.

Two values of different SRIDs are never compared. A function given both
raises an error rather than give no match.

## Importing and exporting

A feed may give coordinates as two numbers, which ST_POINT makes a point
of, with no WKT text to build. Its rows are kept apart until checked:

```sql
CREATE TABLE FEED (ID INTEGER, CUSTOMER VARCHAR(20), EASTING DOUBLE PRECISION, NORTHING DOUBLE PRECISION);
INSERT INTO FEED VALUES (9, 'Garden centre', 300, 900);
CREATE TABLE INCOMING (ID INTEGER PRIMARY KEY, CUSTOMER VARCHAR(20) NOT NULL, LOCATION GEOMETRY(0));
INSERT INTO INCOMING SELECT ID, CUSTOMER, ST_POINT(EASTING, NORTHING) FROM FEED;
INSERT INTO INCOMING VALUES
    (10, 'Night market', ST_GEOMFROMTEXT('POINT EMPTY')),
    (11, 'New office', NULL),
    (12, 'Rail office', ST_GEOMFROMTEXT('LINESTRING (100 100, 200 100)'));
COMMIT;
```

```
OK
OK
OK
OK
OK
OK
```

An address may be missing, empty or of the wrong kind. A `GEOMETRY(0)`
column checks the SRID, of empty values too, and not the kind, so the
import checks that:

```sql
SELECT CUSTOMER,
       CASE
           WHEN LOCATION IS NULL THEN 'missing'
           WHEN ST_ISEMPTY(LOCATION) = 1 THEN 'empty'
           WHEN ST_GEOMETRYTYPE(LOCATION) <> 'ST_Point' THEN 'not a point'
           ELSE 'ready'
       END AS CHECKED
FROM INCOMING ORDER BY ID;
```

```
 CUSTOMER             | CHECKED
----------------------+-------------
 Garden centre        | ready
 Night market         | empty
 New office           | missing
 Rail office          | not a point
(4 rows)
```

Only usable addresses are copied to DELIVERIES. A NULL fails the test of
the kind, and an empty point the test of emptiness. Neither should be
made `(0, 0)`, a real place at the grid's origin. The copy is rolled
back here, so the later examples have the data as it was:

```sql
INSERT INTO DELIVERIES
    SELECT ID, CUSTOMER, 'ready', LOCATION FROM INCOMING
    WHERE ST_GEOMETRYTYPE(LOCATION) = 'ST_Point' AND ST_ISEMPTY(LOCATION) = 0;
SELECT ID, CUSTOMER, ST_ASTEXT(LOCATION) AS WKT FROM DELIVERIES WHERE ID >= 9;
ROLLBACK;
```

```
OK
           ID | CUSTOMER             | WKT
--------------+----------------------+--------------------------------------------------------------
            9 | Garden centre        | POINT (300 900)
(1 row)
OK
```

A map's markers take X and Y apart, of DOUBLE PRECISION, so `1E2` is
100. ST_X and ST_Y give NULL of POINT EMPTY and refuse another kind:

```sql
SELECT CUSTOMER, ST_X(LOCATION) AS X, ST_Y(LOCATION) AS Y FROM DELIVERIES
    WHERE STATUS = 'ready' AND ST_GEOMETRYTYPE(LOCATION) = 'ST_Point' ORDER BY ID;
```

```
 CUSTOMER             |                        X |                        Y
----------------------+--------------------------+--------------------------
 Museum shop          |                      1E2 |                      1E2
 University library   |                      5E2 |                      5E2
 Harbour cafe         |                      9E2 |                      1E2
 Market stall         |                      7E2 |                      3E2
 Hill florist         |                    1.4E3 |                      2E2
 Station kiosk        |                      1E3 |                      1E2
(6 rows)
```

Neither WKT nor WKB carries the SRID, so an export keeps it beside
them, and an import gives it to ST_GEOMFROMTEXT or ST_GEOMFROMWKB. WKT
may spell a number differently, as `1e3` for 1000:

```sql
SELECT NAME, ST_ASTEXT(PATH) AS WKT, ST_SRID(PATH) AS SRID FROM ROUTES ORDER BY ID;
SELECT ST_ASTEXT(ST_GEOMFROMWKB(ST_ASBINARY(PATH), ST_SRID(PATH))) AS RESTORED FROM ROUTES WHERE ID = 1;
```

```
 NAME                 | WKT                                                          |         SRID
----------------------+--------------------------------------------------------------+--------------
 Canal route          | LINESTRING (0 200, 1e3 200)                                  |            0
 North route          | LINESTRING (0 800, 1e3 800)                                  |            0
 East route           | LINESTRING (1200 0, 1200 1e3)                                |            0
(3 rows)
 RESTORED
--------------------------------------------------------------
 LINESTRING (0 200, 1e3 200)
(1 row)
```

A map fits itself to a route by its bounds. A level route's least and
greatest Y are equal, so a window made of them is widened first, as
ST_MAKEENVELOPE refuses a rectangle of no area:

```sql
SELECT ST_MINX(PATH) AS WEST, ST_MINY(PATH) AS SOUTH, ST_MAXX(PATH) AS EAST, ST_MAXY(PATH) AS NORTH
    FROM ROUTES WHERE ID = 1;
```

```
                     WEST |                    SOUTH |                     EAST |                    NORTH
--------------------------+--------------------------+--------------------------+--------------------------
                      0E0 |                      2E2 |                      1E3 |                      2E2
(1 row)
```

## Searching a map window

A dispatcher's map shows the ready orders in its window, made by
ST_MAKEENVELOPE of its west, south, east and north bounds. The Station
kiosk on the window's edge is in it, and the delivered Warehouse is not:

```sql
SELECT ID, CUSTOMER FROM DELIVERIES
    WHERE STATUS = 'ready' AND ST_INTERSECTS(LOCATION, ST_MAKEENVELOPE(0, 0, 1000, 400)) = 1
    ORDER BY ID;
```

```
           ID | CUSTOMER
--------------+----------------------
            1 | Museum shop
            3 | Harbour cafe
            4 | Market stall
            8 | Station kiosk
(4 rows)
```

The index on LOCATION finds the candidates whose rectangles meet the
window, and each is checked against its exact shape, so a point in a
polygon's rectangle but in its hole is not found. A program binds the
window as a parameter, of the column's SRID. The planner uses the index
where it costs less than reading the table, which these few rows may
not, and EXPLAIN shows which it chose.

An address corrected is found at once, within the transaction that
corrects it. It is rolled back here:

```sql
UPDATE DELIVERIES SET LOCATION = ST_POINT(300, 100) WHERE ID = 6;
SELECT ID, CUSTOMER FROM DELIVERIES
    WHERE STATUS = 'ready' AND ST_INTERSECTS(LOCATION, ST_MAKEENVELOPE(0, 0, 1000, 400)) = 1
    ORDER BY ID;
ROLLBACK;
```

```
OK
           ID | CUSTOMER
--------------+----------------------
            1 | Museum shop
            3 | Harbour cafe
            4 | Market stall
            6 | New customer
            8 | Station kiosk
(5 rows)
OK
```

A table that keeps X and Y as numbers has an R-tree of them, in (X, Y)
order, without GEOMETRY. Its columns are NOT NULL, as the index omits a
row of a NULL coordinate, and an unknown position is never `(0, 0)`:

```sql
CREATE TABLE COURIERS (NAME VARCHAR(10) PRIMARY KEY, X DOUBLE PRECISION NOT NULL, Y DOUBLE PRECISION NOT NULL);
INSERT INTO COURIERS VALUES ('Amina', 100, 100), ('Bo', 750, 250), ('Chao', 1500, 200);
COMMIT;
CREATE INDEX COURIERS_XY ON COURIERS (X, Y) USING RTREE;
SELECT NAME FROM COURIERS WHERE X BETWEEN 0 AND 1000 AND Y BETWEEN 0 AND 400 ORDER BY NAME;
```

```
OK
OK
OK
OK
 NAME
------------
 Amina
 Bo
(2 rows)
```

Rectangles, as a map's tiles, have an R-tree of four columns, in
(min X, min Y, max X, max Y) order. A tile meets a window if each
overlaps the other on both axes, and a courier is in a tile if within
its bounds, edges included:

```sql
CREATE TABLE TILES (
    ID INTEGER PRIMARY KEY, NAME VARCHAR(10) NOT NULL,
    MIN_X DOUBLE PRECISION NOT NULL, MIN_Y DOUBLE PRECISION NOT NULL,
    MAX_X DOUBLE PRECISION NOT NULL, MAX_Y DOUBLE PRECISION NOT NULL
);
INSERT INTO TILES VALUES (1, 'West', 0, 0, 1000, 400), (2, 'East', 1000, 0, 2000, 400), (3, 'North', 0, 400, 1000, 1200);
COMMIT;
CREATE INDEX TILES_BOUNDS ON TILES (MIN_X, MIN_Y, MAX_X, MAX_Y) USING RTREE;
SELECT NAME FROM TILES WHERE MIN_X <= 1100 AND MAX_X >= 900 AND MIN_Y <= 300 AND MAX_Y >= 100 ORDER BY ID;
SELECT C.NAME, T.NAME AS TILE FROM COURIERS C JOIN TILES T
    ON C.X BETWEEN T.MIN_X AND T.MAX_X AND C.Y BETWEEN T.MIN_Y AND T.MAX_Y
    ORDER BY C.NAME, T.ID;
```

```
OK
OK
OK
OK
 NAME
------------
 West
 East
(2 rows)
 NAME       | TILE
------------+------------
 Amina      | West
 Bo         | West
 Chao       | East
(3 rows)
```

A numeric R-tree's bounds may have no width, unlike ST_MAKEENVELOPE's.
It does not find the nearest rows by an expression of X and Y, which a
query sorts. The nearest rows are found by ST_DISTANCE of a GEOMETRY
column, as under [Nearby depots](#nearby-depots).

## Service areas and routes

ST_INTERSECTS gives 1 where two shapes share a point, a boundary's
included, so a join finds each ready order's service areas. The library
in Central's hole has none, the new customer's NULL address none, and
the Station kiosk two:

```sql
SELECT D.CUSTOMER, Z.NAME AS SERVED_BY FROM DELIVERIES D JOIN ZONES Z
    ON ST_INTERSECTS(D.LOCATION, Z.BOUNDARY) = 1
    WHERE D.STATUS = 'ready'
    ORDER BY D.ID, Z.ID;
```

```
 CUSTOMER             | SERVED_BY
----------------------+------------
 Museum shop          | Central
 Harbour cafe         | Central
 Market stall         | Central
 Hill florist         | East
 Station kiosk        | Central
 Station kiosk        | East
(6 rows)
```

Of a point, intersection is being inside or on the boundary. A route
that crosses an area intersects it too, and ST_CONTAINS and ST_WITHIN
are not implemented. The planner may probe either table's index with
the other's shapes, as both columns are of SRID 0.

A report of each area's orders keeps the areas with none. The status is
tested in ON, so the unmatched North is not lost. COUNT of a column
counts North 0 where COUNT(*) would count its one row, and warns with
SQLSTATE 01003 that it left a null out:

```sql
SELECT Z.NAME, COUNT(D.ID) AS ORDERS FROM ZONES Z LEFT JOIN DELIVERIES D
    ON ST_INTERSECTS(D.LOCATION, Z.BOUNDARY) = 1 AND D.STATUS = 'ready'
    GROUP BY Z.ID, Z.NAME ORDER BY NAME;
```

```
 NAME       |               ORDERS
------------+----------------------
 Central    |                    4
 East       |                    2
 North      |                    0
(3 rows, with a warning: SQLSTATE 01003)
```

An order in two areas is served by one, the one of least PRIORITY, the
ID breaking a tie. The rule is written in the query, as the order an
index finds rows in is no rule:

```sql
WITH CHOICES AS (
    SELECT D.ID, D.CUSTOMER, Z.NAME AS SERVED_BY,
           ROW_NUMBER() OVER (PARTITION BY D.ID ORDER BY Z.PRIORITY, Z.ID) AS CHOICE
    FROM DELIVERIES D JOIN ZONES Z ON ST_INTERSECTS(D.LOCATION, Z.BOUNDARY) = 1
    WHERE D.STATUS = 'ready'
)
SELECT CUSTOMER, SERVED_BY FROM CHOICES WHERE CHOICE = 1 ORDER BY ID;
```

```
 CUSTOMER             | SERVED_BY
----------------------+------------
 Museum shop          | Central
 Harbour cafe         | Central
 Market stall         | Central
 Hill florist         | East
 Station kiosk        | East
(5 rows)
```

An order no area serves is one for which no area intersects it, which
NOT EXISTS asks. ST_DISJOINT would ask of one area at a time, and an
order in Central is disjoint from North. The test of emptiness also
leaves out NULL, so the orders with no address are listed apart:

```sql
SELECT D.CUSTOMER FROM DELIVERIES D
    WHERE D.STATUS = 'ready' AND ST_ISEMPTY(D.LOCATION) = 0
      AND NOT EXISTS (SELECT 1 FROM ZONES Z WHERE ST_INTERSECTS(D.LOCATION, Z.BOUNDARY) = 1)
    ORDER BY D.ID;
SELECT CUSTOMER FROM DELIVERIES
    WHERE STATUS = 'ready' AND (LOCATION IS NULL OR ST_ISEMPTY(LOCATION) = 1) ORDER BY ID;
```

```
 CUSTOMER
----------------------
 University library
(1 row)
 CUSTOMER
----------------------
 New customer
(1 row)
```

Roadworks close the rectangle from `(300, 100)` to `(500, 300)`. The
Canal route crosses it, though neither of its ends is in it, as the test
is of the whole line. ST_DISJOINT lists the routes still open, and reads
the table, as no index serves it:

```sql
SELECT NAME FROM ROUTES WHERE ST_INTERSECTS(PATH, ST_MAKEENVELOPE(300, 100, 500, 300)) = 1 ORDER BY ID;
SELECT NAME FROM ROUTES WHERE ST_DISJOINT(PATH, ST_MAKEENVELOPE(300, 100, 500, 300)) = 1 ORDER BY ID;
```

```
 NAME
----------------------
 Canal route
(1 row)
 NAME
----------------------
 North route
 East route
(2 rows)
```

An empty path is disjoint from everything, so a list of open routes
tests `ST_ISEMPTY(PATH) = 0` where an import may have left one. The
tests are exact, of the coordinates as stored, and right only of
polygons of valid topology, which an import does not check.

## Nearby depots

A courier waits at `(400, 300)`. ST_DISTANCE gives the straight-line
distance in the grid's metres, not the way by road. The depots within
600 metres, the edge included, are found by a WHERE that reads every
row, as no index serves a distance filter. The CAST rounds the report
and not the test:

```sql
SELECT NAME, CAST(ST_DISTANCE(LOCATION, ST_POINT(400, 300)) AS DECIMAL(8, 1)) AS METRES FROM DEPOTS
    WHERE ST_DISTANCE(LOCATION, ST_POINT(400, 300)) <= 600
    ORDER BY METRES, ID;
```

```
 NAME                 |     METRES
----------------------+------------
 Canal depot          |      500.0
 Market depot         |      600.0
 South depot          |      600.0
(3 rows)
```

The two nearest depots are found by ordering by the distance and
fetching the first rows, the ID choosing between the two at 600 metres.
This form can use the index, which reads the nearest rows first and
measures each exactly, so a polygon's hole or a long route is ranked by
its shape and not its rectangle:

```sql
SELECT NAME, ST_DISTANCE(LOCATION, ST_POINT(400, 300)) AS METRES FROM DEPOTS
    ORDER BY ST_DISTANCE(LOCATION, ST_POINT(400, 300)) ASC NULLS LAST, ID
    FETCH FIRST 2 ROWS ONLY;
```

```
 NAME                 |                   METRES
----------------------+--------------------------
 Canal depot          |                      5E2
 Market depot         |                      6E2
(2 rows)
```

The index is used where the order is ascending with NULLS LAST, the
point a constant or a parameter, and FETCH FIRST a number known when the
query is planned. OFFSET may be given. A descending order, NULLS FIRST,
WITH TIES, PERCENT, an empty or NULL point, a point of another SRID, or a
filter that may raise an error, reads the table and sorts it, with the
same rows.

The ready orders within 100 metres of the Canal route are measured from
the whole line, not its start. Of the new customer, with no address, the
distance is NULL:

```sql
SELECT D.CUSTOMER, ST_DISTANCE(D.LOCATION, R.PATH) AS METRES FROM DELIVERIES D JOIN ROUTES R ON R.ID = 1
    WHERE D.STATUS = 'ready' AND ST_DISTANCE(D.LOCATION, R.PATH) <= 100
    ORDER BY D.ID;
SELECT CUSTOMER, ST_DISTANCE(LOCATION, ST_POINT(400, 300)) AS METRES FROM DELIVERIES WHERE ID = 6;
```

```
 CUSTOMER             |                   METRES
----------------------+--------------------------
 Museum shop          |                      1E2
 Harbour cafe         |                      1E2
 Market stall         |                      1E2
 Station kiosk        |                      1E2
(4 rows)
 CUSTOMER             |                   METRES
----------------------+--------------------------
 New customer         |                     NULL
(1 row)
```

A distance is a DOUBLE PRECISION, so a comparison at a radius's edge
reflects its rounding, and ST_INTERSECTS, not a distance of 0, is the
test of contact. The [reference](../sql/10-spatial.md) gives each
function, its errors and the index's rules.
