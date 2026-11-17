# A Delivery Dispatch Dataset

[Spatial Data](README.md) · [Next: coordinates and geometry](01-model.md)

The examples follow a bicycle delivery service. Dispatchers need to find
orders on a map, match them to service areas, choose a nearby depot and
check whether roadworks affect a route.

These are fictional locations in a local Cartesian grid. X increases east,
Y increases north, and one coordinate unit is one metre. We use SRID 0
because this grid has no assigned reference identifier. The metre unit is
our data convention; SRID 0 does not establish a unit.

## Load the example tables

Connect to `turboreld` and select a schema as in
[Getting Started](../../user/02-getting-started.md). Run this setup once in
an empty schema. The following chapters use these same tables; they do
not require changes made by a previous chapter. Result blocks show the
returned values separated by commas, in SELECT-list order, with NULL
written as `NULL`.

```sql
SET SQL.FUNCTION_CALLS = 'ON';

CREATE TABLE deliveries (
    id INTEGER PRIMARY KEY,
    customer VARCHAR(40) NOT NULL,
    status VARCHAR(12) NOT NULL,
    location GEOMETRY(0)
);
CREATE TABLE delivery_zones (
    id INTEGER PRIMARY KEY,
    name VARCHAR(30) NOT NULL,
    dispatch_priority INTEGER NOT NULL,
    boundary GEOMETRY(0) NOT NULL
);
CREATE TABLE depots (
    id INTEGER PRIMARY KEY,
    name VARCHAR(30) NOT NULL,
    location GEOMETRY(0) NOT NULL
);
CREATE TABLE routes (
    id INTEGER PRIMARY KEY,
    name VARCHAR(30) NOT NULL,
    path GEOMETRY(0) NOT NULL
);

INSERT INTO deliveries VALUES
    (1, 'Museum shop', 'ready', ST_POINT(100, 100)),
    (2, 'University library', 'ready', ST_POINT(500, 500)),
    (3, 'Harbour cafe', 'ready', ST_POINT(900, 100)),
    (4, 'Market stall', 'ready', ST_POINT(700, 300)),
    (5, 'Hill florist', 'ready', ST_POINT(1400, 200)),
    (6, 'New customer', 'ready', NULL),
    (7, 'Warehouse', 'delivered', ST_POINT(200, 200)),
    (8, 'Station kiosk', 'ready', ST_POINT(1000, 100));

INSERT INTO delivery_zones VALUES
    (1, 'Central', 20, ST_GEOMFROMTEXT(
        'POLYGON ((0 0, 1000 0, 1000 1000, 0 1000, 0 0), (400 400, 400 600, 600 600, 600 400, 400 400))')),
    (2, 'East', 10, ST_MAKEENVELOPE(1000, 0, 2000, 1000)),
    (3, 'North', 30, ST_MAKEENVELOPE(0, 1000, 1000, 2000));

INSERT INTO depots VALUES
    (1, 'Canal depot', ST_POINT(100, 700)),
    (2, 'Market depot', ST_POINT(1000, 300)),
    (3, 'South depot', ST_POINT(400, -300)),
    (4, 'North depot', ST_POINT(400, 1100));

INSERT INTO routes VALUES
    (1, 'Canal route', ST_GEOMFROMTEXT('LINESTRING (0 200, 1000 200)')),
    (2, 'North route', ST_GEOMFROMTEXT('LINESTRING (0 800, 1000 800)')),
    (3, 'East route', ST_GEOMFROMTEXT('LINESTRING (1200 0, 1200 1000)'));
COMMIT;

CREATE INDEX deliveries_location ON deliveries (location) USING rtree;
CREATE INDEX delivery_zones_boundary ON delivery_zones (boundary) USING rtree;
CREATE INDEX depots_location ON depots (location) USING rtree;
CREATE INDEX routes_path ON routes (path) USING rtree;
```

Index creation runs outside a transaction, so the setup commits the
insertions first. Subsequent inserts, updates and deletes maintain these
indexes automatically. No separate envelope columns are needed.

## What the addresses tell us

```sql
SELECT customer, status, ST_ASTEXT(location) AS location_wkt
FROM deliveries
ORDER BY id;
```

```text
Museum shop,ready,POINT (100 100)
University library,ready,POINT (500 500)
Harbour cafe,ready,POINT (900 100)
Market stall,ready,POINT (700 300)
Hill florist,ready,POINT (1400 200)
New customer,ready,NULL
Warehouse,delivered,POINT (200 200)
Station kiosk,ready,POINT (1e3 100)
```

Central excludes a pedestrian-only courtyard, represented by the polygon's
hole. University library is inside that courtyard. Station kiosk sits on
the shared boundary of Central and East, so an intersection join matches
both zones. New customer has not supplied a location. Warehouse is already
delivered and should disappear when a dispatcher filters for ready orders.
North has no ready orders, which matters when a report must show zeroes.
When an address matches several zones, dispatch chooses the lowest
`dispatch_priority`; the relationship chapter makes that policy explicit.

The examples keep those distinctions visible. They use names in results,
keep business filters beside spatial filters, and order results explicitly.
A production application should bind changing coordinates and search
geometries as parameters, rather than concatenate them into SQL text.
