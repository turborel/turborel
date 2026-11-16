# Measure Distances

[Spatial Data](README.md)

`ST_DISTANCE(a, b)` finds the minimum planar distance between two
geometries. It works with points, lines, polygons and collections. A
point inside a polygon has distance zero; a point inside one of its
holes has a positive distance to the hole boundary.

The result uses the units of the coordinates you supply. If your data
uses a projected coordinate system measured in metres, the result is in
metres. Longitude and latitude produce an angular distance, not metres.
Assigning SRID 4326 does not make the calculation geodesic. Both inputs
must have the same SRID, and polygon topology must be valid.

## Search within a radius

This example uses a local Cartesian coordinate system with SRID 0:

```sql
SET SQL.FUNCTION_CALLS = 'ON';
CREATE TABLE sites (id INTEGER, shape GEOMETRY(0));
INSERT INTO sites VALUES
    (1, ST_POINT(0, 0)),
    (2, ST_POINT(3, 4)),
    (3, ST_GEOMFROMTEXT('LINESTRING (10 0, 10 10)')),
    (4, ST_GEOMFROMTEXT('POINT EMPTY')),
    (5, NULL);
COMMIT;
CREATE INDEX sites_shape ON sites (shape) USING rtree;

SELECT id FROM sites
WHERE ST_DISTANCE(shape, ST_POINT(0, 0)) <= 7
ORDER BY id;
```

```text
1
2
```

The comparison includes the radius boundary. Distances are approximate
DOUBLE PRECISION numbers, so a comparison very close to the boundary
reflects floating-point rounding.
This radius filter currently scans; distance comparisons in WHERE do not
yet use the geometry index.

## Return the nearest geometries

Order by distance and give ties a stable order with a second key:

```sql
SELECT id, ST_DISTANCE(shape, ST_POINT(0, 0)) AS distance
FROM sites
WHERE ST_ISEMPTY(shape) = 0
ORDER BY distance, id
FETCH FIRST 2 ROWS ONLY;
```

```text
1,0E0
2,5E0
```

The WHERE condition excludes empty geometries and NULL. Neither has a
distance, and their position in a nullable sort should not decide which
geometries are nearest. The same query works for lines and polygons;
it measures the geometry itself, including its interior and boundaries.

On a large table, this form can use the geometry R-tree. The indexed
column must constrain its SRID, as `GEOMETRY(0)` does here. The planner
compares the cost of fetching nearby candidates with scanning the table;
the five-row example is small enough that a scan can cost less.

The tree supplies distance bounds. Each candidate's visible geometry is
measured before it enters the result, so holes, long lines and collections
are ranked by their shapes. Search stops only when the remaining bounds
are greater than the current last qualifying distance. Equal bounds stay
in the search so the secondary `id` key can decide ties.

The indexed form requires ascending distance with NULLS LAST, a point or
other geometry supplied as a constant or parameter, and a FETCH FIRST
count known when the query is planned. OFFSET is supported. WITH TIES,
PERCENT, descending distance and NULLS FIRST currently use ordinary
scanning and sorting. Other selected expressions and WHERE terms must
not raise errors on rows that pruning could skip; the emptiness filter
above is supported.

An indexed query can still fall back to a scan for a NULL or empty query
geometry, a mismatched SRID, extreme coordinates or insufficient work memory.
Fallback preserves the query's results and runtime errors. See the
[indexing chapter](03-indexing.md#nearest-points-in-sql) for numeric R-tree scope.

## Empty values and precision

```sql
SELECT id, ST_DISTANCE(shape, ST_POINT(0, 0))
FROM sites WHERE id >= 4 ORDER BY id;
```

```text
4,NULL
5,NULL
```

A reference mismatch is still an error when either geometry is empty.
A distance beyond the finite DOUBLE PRECISION range raises SQLSTATE
22003. Intermediate calculations use exact fallback arithmetic where
needed, so an overflowing segment length does not by itself make a
finite distance fail.

A positive distance below the representable range may round to zero.
Use `ST_INTERSECTS(a, b) = 1` to test whether geometries share a point;
that operation makes exact topological decisions on the supplied
coordinates.
