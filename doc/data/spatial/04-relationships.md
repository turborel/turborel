# Match Deliveries, Service Areas and Routes

[Spatial Data](README.md) · [Previous: R-tree queries](03-indexing.md) ·
[Next: distances](05-distance.md)

Load the [dispatch dataset](00-sample-data.md) first. The questions here
combine ordinary business columns with geometry: which ready orders each
zone serves, which orders need attention, and which routes meet roadworks.

`ST_INTERSECTS(a, b)` returns integer 1 when the shapes share any point,
including a boundary point. Write `= 1` in WHERE and ON conditions.
`ST_DISJOINT(a, b)` returns 1 when they share none. Both support points,
lines, polygons and collections.

## Join by intersection

Return every ready delivery and the service areas it falls in:

```sql
SELECT delivery.customer, area.name AS service_area
FROM deliveries delivery
JOIN delivery_zones area
  ON ST_INTERSECTS(delivery.location, area.boundary) = 1
WHERE delivery.status = 'ready'
ORDER BY delivery.id, area.id;
```

```text
Museum shop,Central
Harbour cafe,Central
Market stall,Central
Hill florist,East
Station kiosk,Central
Station kiosk,East
```

University library is in Central's excluded courtyard, so it has no match.
Its point is inside the polygon's bounding rectangle, but the exact
predicate excludes the hole. New customer's NULL location also has no
match. Station kiosk appears twice because it lies on the shared boundary
of Central and East.

For point addresses, intersection answers whether the point is inside or
on the boundary of the service area. It is not a general containment test:
a route that merely crosses the area also intersects it. ST_Contains and
ST_Within are not implemented yet.

Both columns enforce the same SRID. The planner can probe a geometry
index with the other table's shapes, or use an adaptive scan for broad
joins. It considers both table orders and chooses by cost; these few
rows may be scanned. See [indexing](03-indexing.md) for the supported forms.

## Report workload by zone

Keep zones with no ready orders in the report. Put the status filter in
the ON clause so it does not discard the unmatched zone:

```sql
SELECT area.id AS zone_id, area.name, COUNT(delivery.id) AS ready_orders
FROM delivery_zones area
LEFT JOIN deliveries delivery
  ON ST_INTERSECTS(delivery.location, area.boundary) = 1
 AND delivery.status = 'ready'
GROUP BY area.id, area.name
ORDER BY area.id;
```

```text
1,Central,4
2,East,2
3,North,0
```

COUNT(delivery.id) counts actual matches; COUNT(*) would count North's
unmatched outer-join row as one. These are counts of zone memberships,
not unique deliveries across all zones. Station kiosk contributes to both
Central and East. Geometry outer joins currently use ordinary execution,
not the indexed spatial inner-join path.

## Assign one service area using a business priority

When dispatch needs exactly one area per address, make the overlap rule
part of the query. Here a smaller `dispatch_priority` wins, with the zone
ID breaking equal priorities:

```sql
WITH ranked_areas AS (
    SELECT delivery.id AS delivery_id, delivery.customer,
           area.name AS service_area,
           ROW_NUMBER() OVER (
               PARTITION BY delivery.id
               ORDER BY area.dispatch_priority, area.id
           ) AS choice
    FROM deliveries delivery
    JOIN delivery_zones area
      ON ST_INTERSECTS(delivery.location, area.boundary) = 1
    WHERE delivery.status = 'ready'
)
SELECT customer, service_area
FROM ranked_areas
WHERE choice = 1
ORDER BY delivery_id;
```

```text
Museum shop,Central
Harbour cafe,Central
Market stall,Central
Hill florist,East
Station kiosk,East
```

Station kiosk is assigned to East by the explicit priority. An index's
traversal order is not an assignment policy. Orders with no matching zone
remain absent and need the separate exception report below.

## Find deliveries without a service area

Find located orders for which no area matches, rather than orders that
are disjoint from just one area:

```sql
SELECT delivery.customer
FROM deliveries delivery
WHERE delivery.status = 'ready'
  AND ST_ISEMPTY(delivery.location) = 0
  AND NOT EXISTS (
      SELECT 1
      FROM delivery_zones area
      WHERE ST_INTERSECTS(delivery.location, area.boundary) = 1
  )
ORDER BY delivery.id;
```

```text
University library
```

Joining on ST_DISJOINT would answer a different question: an order inside
Central is still disjoint from North. NOT EXISTS asks whether *any* zone
serves the address. The emptiness filter also excludes NULL, keeping
unlocated orders out of this geographic exception report.

List those unlocated orders separately for address correction:

```sql
SELECT customer
FROM deliveries
WHERE status = 'ready'
  AND (location IS NULL OR ST_ISEMPTY(location) = 1)
ORDER BY id;
```

```text
New customer
```

## Check routes against roadworks

A roadworks notice closes the rectangle from `(300, 100)` to `(500, 300)`.
Find routes whose actual path meets it:

```sql
SELECT name
FROM routes
WHERE ST_INTERSECTS(path, ST_MAKEENVELOPE(300, 100, 500, 300)) = 1
ORDER BY id;
```

```text
Canal route
```

This checks the line, not just its endpoints. Neither endpoint of Canal
route is inside the closure, but the route crosses it. To list unaffected
routes instead:

```sql
SELECT name
FROM routes
WHERE ST_DISJOINT(path, ST_MAKEENVELOPE(300, 100, 500, 300)) = 1
ORDER BY id;
```

```text
North route
East route
```

ST_DISJOINT currently scans rather than using geometry index probes.
An empty path is disjoint from everything; add ST_ISEMPTY(path) = 0 if
empty imported routes should not appear in an operational route list.

## Reference systems and valid shapes

Both inputs must have the same SRID, even when empty or far apart.
ST_SETSRID only changes a label; it cannot repair coordinates from a
different reference system. All relationships use the XY plane, including
for geographic coordinates. There is no geodesic or antimeridian handling.

NULL produces NULL. An empty geometry intersects nothing. Polygon inputs
must have valid topology: constructors check ring structure, but do not
validate self-intersections or hole placement. Import success is not a
validity certificate. Boundary decisions use exact orientation signs on
the stored binary64 coordinates, without an epsilon; they cannot restore
precision lost before import.
