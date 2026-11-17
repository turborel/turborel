# Query Spatial Data with an R-tree

[Spatial Data](README.md) · [Previous: store and exchange geometries](02-interchange.md) ·
[Next: spatial relationships](04-relationships.md)

An R-tree groups nearby bounding rectangles so a query can skip groups
outside its search region. Use a GEOMETRY column when you need shapes,
or numeric columns when your data consists of point coordinates or
rectangles. The database maintains either index as rows change.

## Index geometry intersections

Declare the column's SRID and index the geometry itself:

```sql
SET SQL.FUNCTION_CALLS = 'ON';
CREATE TABLE features (id INTEGER PRIMARY KEY, shape GEOMETRY(0));
COMMIT;
CREATE INDEX features_shape ON features (shape) USING rtree;
INSERT INTO features VALUES
    (1, ST_POINT(5, 5)),
    (2, ST_GEOMFROMTEXT('POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 2 8, 8 8, 8 2, 2 2))')),
    (3, ST_GEOMFROMTEXT('POINT EMPTY')),
    (4, NULL);
COMMIT;
SELECT id FROM features
WHERE ST_INTERSECTS(shape, ST_POINT(5, 5)) = 1
ORDER BY id;
```

```text
1
```

Feature 2's bounding rectangle contains `(5, 5)`, but that point is in
its hole. The exact intersection test removes that candidate. You do
not need separate envelope columns or a second bounding-box condition.
The index updates automatically when the geometry changes.

For a rectangular search window, construct the geometry from its bounds:

```sql
SELECT id FROM features
WHERE ST_INTERSECTS(shape, ST_MAKEENVELOPE(9, 9, 11, 11)) = 1
ORDER BY id;
```

```text
2
```

This window intersects the polygon's upper corner. It uses the same
geometry-index path, and avoids building a polygon text string in the
application. Supply an SRID as the fifth argument when it is not 0.

`GEOMETRY(0)` means all values carry SRID 0; use the SRID appropriate
to your data. An unconstrained `GEOMETRY` column cannot have this index.
The constraint prevents mixing reference labels within the indexed column;
it does not transform coordinates or validate a CRS identifier.

The planner recognizes `ST_INTERSECTS(shape, value) = 1` with a constant
or prepared parameter, including the arguments or comparison reversed.
It chooses by cost, so this tiny example can still use a table scan.
NULL and empty geometries never intersect. A different query SRID still
raises an error; an index cannot silently discard the mismatch.

An inner join can also probe the index with a geometry from the other
table when both columns enforce the same SRID. The
[relationship chapter](04-relationships.md#join-by-intersection) shows
that query. Geometry outer joins and ST_DISJOINT do
not yet use these geometry probes.

## Find points in a window

Use two REAL, FLOAT or DOUBLE PRECISION columns in `(X, Y)` order:

```sql
CREATE TABLE sites (
    id INTEGER PRIMARY KEY,
    x DOUBLE PRECISION NOT NULL,
    y DOUBLE PRECISION NOT NULL
);
COMMIT;
CREATE INDEX sites_xy ON sites (x, y) USING rtree;
INSERT INTO sites VALUES (1, 0, 0), (2, 5, 5), (3, 10, 10), (4, 50, 50);
COMMIT;
SELECT id FROM sites
WHERE x BETWEEN 0 AND 5 AND y BETWEEN 0 AND 5
ORDER BY id;
```

Result rows:

```text
1
2
```

Both bounds are inclusive. The point at `(5, 5)` belongs to this window.
The planner can combine the two coordinate ranges into an R-tree probe.
It still chooses by cost: on a small table or a broad window, scanning
the table can be cheaper. An index does not prescribe result order;
use ORDER BY when order matters.

Creating an index requires no open transaction. Commit or roll back a
previous query or modification first. The examples do so explicitly.

## Find overlapping rectangles

Four columns must be ordered `(min X, min Y, max X, max Y)` in the index.
Each minimum must be at most its maximum; zero-width rectangles are
allowed. All coordinates must be finite.

```sql
COMMIT;
CREATE TABLE regions (
    id INTEGER PRIMARY KEY,
    min_x DOUBLE PRECISION NOT NULL,
    min_y DOUBLE PRECISION NOT NULL,
    max_x DOUBLE PRECISION NOT NULL,
    max_y DOUBLE PRECISION NOT NULL
);
COMMIT;
CREATE INDEX regions_bounds ON regions (min_x, min_y, max_x, max_y) USING rtree;
INSERT INTO regions VALUES
    (1, 0, 0, 5, 5), (2, 5, 5, 10, 10), (3, 20, 20, 30, 30);
COMMIT;
SELECT id FROM regions
WHERE min_x <= 6 AND max_x >= 4
  AND min_y <= 6 AND max_y >= 4
ORDER BY id;
```

```text
1
2
```

The query asks which rectangles overlap the closed window from `(4, 4)`
to `(6, 6)`. Notice that it compares both minima and maxima. Treating
only a rectangle's lower corner as a point would miss rectangles that
extend into the query window from outside it.

These tests are exact for rectangles. If the rectangles instead bound
complex geometries, they provide candidates only. They do not establish
polygon intersection or containment. If you keep separate shape and envelope
columns, keeping them consistent is the application's responsibility.
The geometry index above maintains its envelopes automatically.

## Join points to regions

The same comparisons can use columns from the other side of a join:

```sql
SELECT s.id AS site_id, r.id AS region_id
FROM sites s JOIN regions r
ON s.x >= r.min_x AND s.x <= r.max_x
AND s.y >= r.min_y AND s.y <= r.max_y
ORDER BY s.id, r.id;
```

```text
1,1
2,1
2,2
3,2
```

Site 2 lies on the shared corner and belongs to both closed regions.
An indexed inner join can probe an R-tree using each outer row's bounds,
then check the full join condition on the visible rows. SQL planning
does not yet traverse two R-trees as a single join operation. Do not
assume that indexing both tables forces a particular join algorithm.

## Nearest points in SQL

For small Cartesian datasets, ordinary SQL can sort squared Euclidean
distances. This example asks for the two points nearest `(4, 4)`:

```sql
SELECT id FROM sites
ORDER BY (x - 4) * (x - 4) + (y - 4) * (y - 4), id
FETCH FIRST 2 ROWS ONLY;
```

```text
2
1
```

The coordinates here are small, so squaring their differences is safe.
The formula needs care with extreme magnitudes. It measures the units of
the coordinates and is not a geodesic distance formula.

This numeric expression still scans and sorts. To use indexed nearest
ordering, store a `GEOMETRY(srid)` column, index it and order by
`ST_DISTANCE` with FETCH FIRST. The [distance chapter](05-distance.md)
shows that form. Geometry queries refine the actual shape before ranking;
bounding-box distance alone would be insufficient for polygons with holes.

## Updates, NULLs and performance

The database maintains geometry and numeric indexes as rows change and
applies its ordinary transaction visibility rules. A query rechecks the
geometry version visible to its transaction, including when older and
newer versions have the same bounding rectangle.

Broad geometry probes collect a bounded block of candidates and fetch
nearby row IDs together. Each active cursor may use about 160 KiB for
that block, charged to the database memory budget. If it cannot reserve
the memory, it uses a smaller buffer or streams candidates directly.
Nearest queries retain their distance traversal order.

An R-tree omits a row with any NULL coordinate. The planner uses it only
when that omission cannot hide a matching row. NOT NULL coordinate
columns make the intended data model explicit; do not invent `(0, 0)`
coordinates for unknown locations.

Performance depends on the distribution of the bounds and how much of
the table a query returns. Large or overlapping rectangles give the
tree less to prune. A dense join can legitimately produce millions of
pairs; an index cannot remove the cost of returning those matches.

Measure narrow and broad windows, realistic overlap, and sustained
updates as well as fresh loads. Empty nodes currently remain in a live
tree, and the tree does not perform forced reinsertion or deletion
condensation. Space and search quality after churn matter alongside
initial throughput.

The project's [benchmark instructions](../../../CONTRIBUTING.md#spatial-benchmarks)
separate SQL queries from raw index operations. Raw nearest-entry timings
are not SQL geometry-distance timings. No benchmark comparing this
geometry increment with PostGIS has been performed.
