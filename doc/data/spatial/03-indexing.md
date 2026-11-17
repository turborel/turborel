# Query Spatial Data with an R-tree

[Spatial Data](README.md) · [Previous: store and exchange geometries](02-interchange.md) ·
[Next: spatial relationships](04-relationships.md)

Load the [dispatch dataset](00-sample-data.md) first. Its geometry indexes
let the planner find candidate deliveries, service areas, depots and
routes without testing every shape. Queries still name the relationship
they need; the index does not replace the geometry test.

## Show ready deliveries in a map window

A dispatch map asks for orders in its current viewport, excluding orders
already delivered. Construct the viewport directly from its bounds:

```sql
SELECT id, customer
FROM deliveries
WHERE status = 'ready'
  AND ST_INTERSECTS(location, ST_MAKEENVELOPE(0, 0, 1000, 400)) = 1
ORDER BY id;
```

```text
1,Museum shop
3,Harbour cafe
4,Market stall
8,Station kiosk
```

The four numbers are west, south, east and north in the data's coordinate
system. Station kiosk lies on the eastern edge and is included. Warehouse
is in the window but has already been delivered. A missing or empty
location cannot intersect the window.

In a map application, bind the changing bounds or the constructed geometry
as parameters. For a fixed-SRID column, the query geometry must carry the
same SRID; pass it as the fifth argument to ST_MAKEENVELOPE when it is not
0. Each minimum must be strictly below its maximum. Use ST_POINT for a
single-coordinate query.

The setup created `deliveries_location` on `deliveries(location)` using
`rtree`. You do not need four additional envelope columns or a separate
bounding-box condition. For polygons, the index's rectangle is only a
candidate bound: the exact predicate excludes holes and other nonmatching
parts. [Service-area joins](04-relationships.md#join-by-intersection)
show why this matters.

## Keep the map current after an address correction

Updating a location updates its index in the same transaction. Here the
new customer supplies an address inside the viewport:

```sql
COMMIT;
UPDATE deliveries SET location = ST_POINT(300, 100) WHERE id = 6;
SELECT id, customer
FROM deliveries
WHERE status = 'ready'
  AND ST_INTERSECTS(location, ST_MAKEENVELOPE(0, 0, 1000, 400)) = 1
ORDER BY id;
ROLLBACK;
```

```text
1,Museum shop
3,Harbour cafe
4,Market stall
6,New customer
8,Station kiosk
```

The example rolls back the correction to preserve the sample data.
Commit a real correction when it is accepted. Other sessions follow the
database's transaction visibility rules; an index candidate is checked
against the geometry version visible to the reading transaction.

## Understand when the index helps

The planner recognizes `ST_INTERSECTS(column, value) = 1`, with a constant
or parameter in either argument position. The indexed column must be
`GEOMETRY(srid)`, not unconstrained GEOMETRY. A different query SRID raises
an error rather than quietly returning no matches.

Use EXPLAIN to inspect a query's chosen access path. On these small tables,
a scan can be cheaper than an index lookup. A city-wide viewport can also
favor a scan even when a street-sized viewport benefits from the R-tree.
Estimates count small candidate sets and sample larger ones; they affect
the choice of plan, never the exact result.

For an unordered intersection join, `AdaptiveSpatialJoin` can begin with
index probes and switch to one scan when repeated broad lookups would cost
more. It buffers the remaining outer rows only within the work-area and
database memory budgets, and checks the full geometry relationship for
every candidate. Missing estimates, insufficient memory or a required
input order retain the ordinary index join.

Geometry outer joins, ST_DISJOINT filters and distance comparisons in
WHERE do not currently use geometry probes. Combining a window with a
distance filter does not promise an indexed radius search: other
expressions that can raise errors can prevent pruning. See
[distance queries](05-distance.md) for the supported nearest-neighbour form.

## Find couriers stored as numeric coordinates

An existing telemetry table may already store X and Y separately. It can
use a numeric R-tree without converting its data to GEOMETRY:

```sql
COMMIT;
CREATE TABLE courier_positions (
    courier VARCHAR(30) PRIMARY KEY,
    x DOUBLE PRECISION NOT NULL,
    y DOUBLE PRECISION NOT NULL
);
INSERT INTO courier_positions VALUES
    ('Amina', 100, 100), ('Bo', 750, 250), ('Chao', 1500, 200);
COMMIT;
CREATE INDEX courier_positions_xy ON courier_positions (x, y) USING rtree;
SELECT courier
FROM courier_positions
WHERE x BETWEEN 0 AND 1000 AND y BETWEEN 0 AND 400
ORDER BY courier;
```

```text
Amina
Bo
```

The index columns must be REAL, FLOAT or DOUBLE PRECISION, in `(X, Y)`
order. Bounds are inclusive. NOT NULL makes a usable location part of the
row's contract; an R-tree omits rows with a NULL coordinate. Do not use
zero coordinates to stand for a courier whose location is unknown.

## Load the map tiles a viewport overlaps

For precomputed rectangular tiles, index their four bounds in
`(min X, min Y, max X, max Y)` order:

```sql
COMMIT;
CREATE TABLE dispatch_tiles (
    id INTEGER PRIMARY KEY,
    name VARCHAR(20) NOT NULL,
    min_x DOUBLE PRECISION NOT NULL,
    min_y DOUBLE PRECISION NOT NULL,
    max_x DOUBLE PRECISION NOT NULL,
    max_y DOUBLE PRECISION NOT NULL
);
INSERT INTO dispatch_tiles VALUES
    (1, 'West', 0, 0, 1000, 400),
    (2, 'East', 1000, 0, 2000, 400),
    (3, 'North', 0, 400, 1000, 1200);
COMMIT;
CREATE INDEX dispatch_tiles_bounds
ON dispatch_tiles (min_x, min_y, max_x, max_y) USING rtree;
SELECT name
FROM dispatch_tiles
WHERE min_x <= 1100 AND max_x >= 900
  AND min_y <= 300 AND max_y >= 100
ORDER BY id;
```

```text
West
East
```

The viewport straddles West and East. Testing only each tile's lower
corner would miss West, whose lower corner is outside the viewport.
These comparisons are exact for rectangles. Rectangles around complex
shapes supply candidates only; use geometry predicates for those shapes.
Numeric bounds must be finite and ordered; zero-width rectangles are
allowed, unlike ST_MAKEENVELOPE's polygon constructor.

## Match courier positions to tiles

The same comparisons can reference the other side of a join:

```sql
SELECT vehicle.courier, tile.name AS tile
FROM courier_positions vehicle
JOIN dispatch_tiles tile
  ON vehicle.x >= tile.min_x AND vehicle.x <= tile.max_x
 AND vehicle.y >= tile.min_y AND vehicle.y <= tile.max_y
ORDER BY vehicle.courier, tile.id;
```

```text
Amina,West
Bo,West
Chao,East
```

A courier exactly on a shared tile edge matches both closed rectangles.
Choose an explicit ownership rule if a downstream operation needs one
tile per courier. An indexed join can probe using the other table's
bounds; indexing both sides does not force a traversal of both trees.

## Nearest points in SQL

For nearest-depot searches, use a `GEOMETRY(srid)` column and ST_DISTANCE
with FETCH FIRST, as in the [distance chapter](05-distance.md). Sorting a
numeric expression such as squared X/Y differences still scans and sorts;
the numeric R-tree does not turn that expression into indexed nearest SQL.

## Measure the workload users will run

Test street-sized and city-wide views, overlapping service areas, and
ongoing location updates. Large or overlapping bounds leave less to
prune, and a join returning millions of pairs must still fetch and send
those pairs. Include complete result fetching when measuring it.

Broad geometry probes can batch nearby row IDs for more local reads;
their candidate buffers are bounded and charged to the database memory
budget. Nearest queries retain distance traversal order. Index nodes
left empty by deletes currently remain in the tree; search quality after
sustained updates matters as well as performance after a fresh load.

The [benchmark instructions](../../../CONTRIBUTING.md#spatial-benchmarks)
distinguish complete SQL workloads from raw index operations. Compare
engines with the same answers, transaction settings and client boundary.
