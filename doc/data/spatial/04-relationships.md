# Test Spatial Relationships

[Spatial Data](README.md) · [Previous: R-tree queries](03-indexing.md)

Use `ST_INTERSECTS(a, b)` to ask whether two geometries share any point.
It returns integer 1 for a match and 0 otherwise. `ST_DISJOINT(a, b)`
asks the opposite question. Both functions support XY points, lines,
polygons and collections.

## Find points in a polygon

This parcel has an outer boundary and one square hole:

```sql
CREATE TABLE parcels (id INTEGER, shape GEOMETRY(0));
INSERT INTO parcels VALUES (1, ST_GEOMFROMTEXT(
    'POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 8 2, 8 8, 2 8, 2 2))'));
CREATE TABLE sites (id INTEGER, shape GEOMETRY(0));
INSERT INTO sites VALUES
    (1, ST_POINT(1, 1)),
    (2, ST_POINT(5, 5)),
    (3, ST_POINT(2, 5)),
    (4, ST_POINT(11, 5));
SELECT s.id, ST_INTERSECTS(s.shape, p.shape)
FROM sites s CROSS JOIN parcels p ORDER BY s.id;
```

```text
1,1
2,0
3,1
4,0
```

Site 1 is inside the parcel. Site 2 lies inside its hole and does not
intersect the parcel. Site 3 lies on the hole's boundary and does
intersect it. Site 4 is outside the outer boundary.

A bounding rectangle alone cannot distinguish sites 1, 2 and 3.
`ST_INTERSECTS` checks the actual shape. Sharing a boundary counts;
this is different from a containment test that requires an interior
point. ST_Contains and ST_Within are not implemented yet.

When many points are tested against one polygon, a fixed query value or
prepared parameter lets turborel reuse the polygon's decoded form during
that execution. With constant folding enabled, it can also prepare the
polygon boundary to skip edges that cannot meet a point's horizontal ray.
This uses the database's memory budget. If the extra index does not fit,
the query uses ordinary geometry evaluation with the same results.

## Join by intersection

Put the relationship in the join condition to return matching pairs.
Indexing the sites gives the planner a way to find candidates for each
parcel:

```sql
COMMIT;
CREATE INDEX sites_shape ON sites (shape) USING rtree;
SELECT s.id AS site_id, p.id AS parcel_id
FROM sites s JOIN parcels p ON ST_INTERSECTS(s.shape, p.shape) = 1
ORDER BY s.id, p.id;
```

```text
1,1
3,1
```

A site on the boundary of two parcels can match both. Add the appropriate
business rule if an application needs one owner per site; the geometric
relationship alone does not choose an owner.

Both columns enforce SRID 0. With matching SRID constraints, the planner
can use each parcel's envelope to probe the sites' R-tree, then check
the actual geometries. Holes and boundaries keep the same meaning as
in the query above. Both table orders are considered. The planner chooses
by cost, so these four sites can still be scanned.

An unconstrained `GEOMETRY` column, different column SRIDs, or another
join condition that can raise an error prevents this optimization.
Geometry outer joins also use their ordinary execution path. Indexing
both tables does not make the planner traverse both trees together:
the indexed inner join probes one tree for each row of the other input.

## Reference systems, empty values and validity

Both arguments must have the same SRID. A mismatch is an error even if
one geometry is empty or their envelopes are disjoint. `ST_SETSRID` does
not repair a coordinate-system mismatch: it changes a label, without
transforming the coordinates.

An empty geometry intersects nothing:

```sql
VALUES ST_INTERSECTS(ST_POINT(1, 1), ST_GEOMFROMTEXT('POINT EMPTY'));
VALUES ST_DISJOINT(ST_POINT(1, 1), ST_GEOMFROMTEXT('POINT EMPTY'));
```

The results are 0 and 1, respectively. NULL arguments produce NULL.

Polygon inputs must describe valid simple-feature topology. Constructors
check ring length and closure, but do not establish that rings are simple,
that holes are inside their shell, or that the polygon interior is
connected. A successful WKT import is therefore not a validity check.
Invalid polygons do not have a guaranteed spatial relationship result.

The orientation tests distinguish collinearity from very small offsets
without using an epsilon. This protects the decisions made on the stored
coordinates; it cannot recover precision already lost before import.
All relationships use the XY plane. Supplying SRID 4326 does not turn a
straight segment into a geodesic or introduce antimeridian handling.
