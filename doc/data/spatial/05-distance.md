# Find Nearby Depots and Deliveries

[Spatial Data](README.md) · [Previous: spatial relationships](04-relationships.md)

Load the [dispatch dataset](00-sample-data.md) first. A courier is waiting
at `(400, 300)` and needs a pickup depot. Its coordinates and the sample
locations are in the same local metre grid, so ST_DISTANCE returns metres.

ST_DISTANCE measures minimum planar separation between points, lines,
polygons and collections. This is straight-line geometric distance, not
travel distance or journey time along a road network. Longitude/latitude
inputs would produce angular distances, not metres; supplying an SRID
does not make the calculation geodesic.

## Search within a radius

Find depots at most 600 metres from the courier, including the boundary:

```sql
SELECT name,
       CAST(ST_DISTANCE(location, ST_POINT(400, 300)) AS DECIMAL(8, 1)) AS metres
FROM depots
WHERE ST_DISTANCE(location, ST_POINT(400, 300)) <= 600
ORDER BY metres, id;
```

```text
Canal depot,500.0
Market depot,600.0
South depot,600.0
```

The CAST formats the report to one decimal place; the WHERE condition
uses the unrounded distance. A production application can bind the query
point and radius as parameters. ST_DWithin is not implemented yet.
Distance comparisons in WHERE currently scan, even when the geometry
column has an R-tree. A window condition added alongside the distance
expression does not guarantee an indexed radius query.

## Return the nearest depots

When the dispatcher needs a fixed number of choices rather than every
depot inside a radius, order by distance and use FETCH FIRST:

```sql
SELECT name, ST_DISTANCE(location, ST_POINT(400, 300)) AS distance_metres
FROM depots
WHERE ST_ISEMPTY(location) = 0
ORDER BY distance_metres ASC NULLS LAST, id
FETCH FIRST 2 ROWS ONLY;
```

```text
Canal depot,5E2
Market depot,6E2
```

`5E2` is 500 metres. Market depot and South depot are both 600 metres
away; the ID tie-breaker selects Market depot for the second place.
The emptiness filter excludes empty and NULL locations. Use the
ST_DISTANCE expression itself for indexed nearest ordering; formatting
or other fallible expressions can change the chosen plan.

This form can use the geometry R-tree on a fixed-SRID column. The planner
compares candidate fetches with a scan and sort; four depots are small
enough that a scan can cost less. It measures each candidate's actual,
visible geometry before ranking it. Bounding-box distance alone cannot
rank a polygon with a hole or a long route correctly.

Indexed nearest ordering requires ascending distance with NULLS LAST,
a constant or parameter query geometry, and a FETCH FIRST count known at
planning. OFFSET is supported. WITH TIES, PERCENT, descending distance and
NULLS FIRST retain ordinary scanning and sorting. Other selected
expressions and filters must not raise errors that pruning could hide;
the emptiness filter above is supported.

A NULL or empty query geometry, a mismatched SRID, extreme coordinates or
insufficient work memory can cause a scan fallback. The fallback preserves
results and errors. No index promises that every query uses it.

## Find ready orders near a scheduled route

A dispatcher may want to group deliveries close to a route already being
ridden. Measure each address against the whole route, not its start point:

```sql
SELECT delivery.customer,
       CAST(ST_DISTANCE(delivery.location, route.path) AS DECIMAL(8, 1)) AS offset_metres
FROM deliveries delivery
JOIN routes route ON route.id = 1
WHERE delivery.status = 'ready'
  AND ST_DISTANCE(delivery.location, route.path) <= 100
ORDER BY delivery.id;
```

```text
Museum shop,100.0
Harbour cafe,100.0
Market stall,100.0
Station kiosk,100.0
```

These addresses are within 100 metres of Canal route. This can produce a
shortlist for a dispatcher, but it does not account for fences, bridges,
street access or the cost of a detour. Distance joins of this form
currently use ordinary evaluation, not indexed nearest traversal.

The same function measures separation from a service-area polygon.
An address inside or on the polygon has distance zero; an address inside
its excluded courtyard has a positive distance to the courtyard boundary.

## Missing locations and precision

New customer has no location, so its distance is unknown:

```sql
SELECT customer, ST_DISTANCE(location, ST_POINT(400, 300)) AS distance_metres
FROM deliveries
WHERE id = 6;
```

```text
New customer,NULL
```

Empty geometries also have NULL distance. Both inputs must have the same
SRID, even when empty, and polygon topology must be valid. Distances are
approximate DOUBLE PRECISION values; radius comparisons near a boundary
reflect their rounding. A distance beyond the finite range raises
SQLSTATE 22003. Intermediate overflow alone does not make a finite final
distance fail.

A positive distance below the representable range may round to zero.
Use ST_INTERSECTS for a topological test instead of comparing distance to
zero. The [relationship chapter](04-relationships.md) shows intersection
filters and joins; the [SQL reference](../../sql/spatial.md) lists the
function contracts.
