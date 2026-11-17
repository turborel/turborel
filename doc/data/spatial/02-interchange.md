# Store and Exchange Geometries

[Spatial Data](README.md) · [Previous: coordinates and geometry](01-model.md) ·
[Next: R-tree queries](03-indexing.md)

Load the [dispatch dataset](00-sample-data.md) first. This chapter imports a
small delivery feed, checks whether its addresses can be dispatched, and
exports geometry together with its reference identifier.

## Turn coordinate columns into locations

A CSV import or upstream application may supply coordinates as separate
columns. Keep them in a staging table, then construct points with
`ST_POINT`; there is no need to assemble WKT strings:

```sql
COMMIT;
CREATE TABLE delivery_feed (
    id INTEGER,
    customer VARCHAR(40),
    easting DOUBLE PRECISION,
    northing DOUBLE PRECISION
);
INSERT INTO delivery_feed VALUES (9, 'Garden centre', 300, 900);
CREATE TABLE incoming_deliveries (
    id INTEGER PRIMARY KEY,
    customer VARCHAR(40) NOT NULL,
    location GEOMETRY(0)
);
INSERT INTO incoming_deliveries
SELECT id, customer, ST_POINT(easting, northing)
FROM delivery_feed;
COMMIT;
SELECT customer, ST_ASTEXT(location) AS location_wkt
FROM incoming_deliveries ORDER BY id;
```

```text
Garden centre,POINT (300 900)
```

Both coordinate columns must describe the same grid and unit. For a
longitude/latitude feed, X is longitude and Y is latitude if that is the
convention you have chosen. Supply its SRID as the third argument and use
a matching column type; do not load angular coordinates into this metre
example merely by giving them SRID 0.

`GEOMETRY(0)` requires SRID 0 on every non-NULL value. A column declared
`GEOMETRY(4326)` requires 4326 instead. The identifier is checked on
assignment and storage, including for empty geometries. Plain `GEOMETRY`
permits mixed identifiers, but cannot use the geometry R-tree index.
A fixed-SRID column accepts all seven geometry kinds; applications that
require addresses to be points must check the kind too.

## Review an imported batch before dispatch

WKT feeds can contain empty shapes, missing values or the wrong geometry
kind for an address. Retain that distinction in the staging table:

```sql
INSERT INTO incoming_deliveries VALUES
    (10, 'Night market', ST_GEOMFROMTEXT('POINT EMPTY')),
    (11, 'New office', NULL),
    (12, 'Rail office', ST_GEOMFROMTEXT('LINESTRING (100 100, 200 100)'));
COMMIT;
SELECT customer,
       TRIM(CASE
           WHEN location IS NULL THEN 'missing location'
           WHEN ST_ISEMPTY(location) = 1 THEN 'empty geometry'
           WHEN ST_GEOMETRYTYPE(location) <> 'ST_Point' THEN 'not a point'
           ELSE 'ready to import'
       END) AS import_status
FROM incoming_deliveries
ORDER BY id;
```

```text
Garden centre,ready to import
Night market,empty geometry
New office,missing location
Rail office,not a point
```

Copy only usable addresses to the live table. This example rolls the
insertion back after inspecting it, so later chapters keep the original
dispatch data. In an application, commit the accepted batch:

```sql
INSERT INTO deliveries
SELECT id, customer, 'ready', location
FROM incoming_deliveries
WHERE ST_GEOMETRYTYPE(location) = 'ST_Point'
  AND ST_ISEMPTY(location) = 0;
SELECT id, customer, ST_ASTEXT(location) AS location_wkt
FROM deliveries WHERE id >= 9 ORDER BY id;
ROLLBACK;
```

```text
9,Garden centre,POINT (300 900)
```

NULL fails these WHERE tests, while an empty point fails the emptiness
test. Do not replace either with `(0, 0)`: that would create a real,
queryable address at the grid's origin. Geometry-kind constraints are not
yet available, and registered functions cannot currently be used in
stored CHECK conditions.

## Export coordinates for map markers

Use the numeric observers when a client expects separate coordinates:

```sql
SELECT id, customer, ST_X(location) AS x, ST_Y(location) AS y
FROM deliveries
WHERE status = 'ready' AND ST_GEOMETRYTYPE(location) = 'ST_Point'
ORDER BY id;
```

```text
1,Museum shop,1E2,1E2
2,University library,5E2,5E2
3,Harbour cafe,9E2,1E2
4,Market stall,7E2,3E2
5,Hill florist,1.4E3,2E2
8,Station kiosk,1E3,1E2
```

ST_X and ST_Y accept points, return NULL for POINT EMPTY, and reject
other kinds. They return DOUBLE PRECISION; `1E2` in the output means 100.
They do not convert units or infer longitude and latitude.

## Keep the SRID beside exported WKT or WKB

A shape's text is useful in an export, a support ticket or an audit log.
Keep its SRID in a separate field:

```sql
SELECT name, ST_ASTEXT(path) AS route_wkt, ST_SRID(path) AS srid
FROM routes
ORDER BY id;
```

```text
Canal route,LINESTRING (0 200, 1e3 200),0
North route,LINESTRING (0 800, 1e3 800),0
East route,LINESTRING (1200 0, 1200 1e3),0
```

WKT output may normalize whitespace and number spelling: `1e3` means
1000. WKT contains no SRID. `ST_GEOMFROMTEXT(route_wkt, srid)` restores both
parts of that export. WKB has the same requirement: export
`ST_ASBINARY(path)` and `ST_SRID(path)`, then pass both to
`ST_GEOMFROMWKB`. Here is a binary round trip for a route:

```sql
SELECT name,
       ST_ASTEXT(ST_GEOMFROMWKB(ST_ASBINARY(path), ST_SRID(path))) AS restored_path
FROM routes WHERE id = 1;
```

```text
Canal route,LINESTRING (0 200, 1e3 200)
```

The binary constructor accepts standard XY WKB in either byte order and
writes little-endian WKB. It does not accept EWKB's embedded SRID or Z/M
flags. POINT EMPTY uses the interoperable pair-of-NaNs convention; other
non-finite coordinates are refused.

`CAST(text AS GEOMETRY(4326))` supplies 4326 while reading WKT.
`CAST(existing_geometry AS GEOMETRY(4326))` checks its existing label.
`ST_SETSRID` changes only a label and is appropriate only when the
coordinates already belong to that reference system. None of these
operations reprojects coordinates.

## Fit the map to a route

Send a route's bounds to a map client without building another polygon:

```sql
SELECT name, ST_MINX(path) AS west, ST_MINY(path) AS south,
       ST_MAXX(path) AS east, ST_MAXY(path) AS north
FROM routes WHERE id = 1;
```

```text
Canal route,0E0,2E2,1E3,2E2
```

All four extrema functions work for every supported kind. An empty shape
has no bounds. A horizontal route has equal minimum and maximum Y;
expand the display bounds before using them to construct a nonzero-area
`ST_MAKEENVELOPE` search polygon.

## Storage, errors and limits

Geometry columns participate in ordinary transactions, rollback,
checkpoints and recovery. Stored values keep their SRID and coordinates.
An embedding application must register the type before reopening files;
`turboreld` does that automatically. The native client can carry the value
opaquely, but GIS discovery tables and dedicated GIS drivers are not yet
provided.

Malformed WKT or WKB fails with SQLCODE -301. Unsupported dimensions,
kinds or representation limits fail with -900. Constructors check
structure, not polygon topology. An invalid row in a multirow insertion
fails the whole statement; it does not leave its earlier rows inserted.

The SQL interchange functions use the ordinary 32,767-character/octet
string limits. CLOB/BLOB interchange is not provided by these geometry
functions. The Rust codecs have larger [representation limits](../../appendix/d-limits.md).
See the [function reference](../../sql/spatial.md) for signatures and errors.
