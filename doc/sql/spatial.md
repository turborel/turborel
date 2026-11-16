# Geometry Values *(extension)*

[Contents](README.md)

GEOMETRY stores XY points, lines, polygons and collections, with an SRID
identifying their spatial reference system. It is included in `turboreld`;
embedding programs register it explicitly. Enable the existing
SQL.FUNCTION_CALLS setting to call its functions. It supports storage,
interchange, XY intersection tests and planar distances. An R-tree on a
`GEOMETRY(srid)` column can accelerate intersection filters, inner joins
and ascending ST_DISTANCE ordering with FETCH FIRST. See the
[distance guide](../data/spatial/05-distance.md) for the indexed form and
its scan fallbacks.

For an introduction and complete SQL examples, read the
[Spatial Guide](../data/spatial/README.md). Application developers enable
the feature through the [embedding API](../client/03-embedding.md#geometry-values).

## Type

`GEOMETRY` accepts any signed 32-bit SRID. `GEOMETRY(srid)` constrains
all non-NULL values, including empty ones, to that identifier; the column
parameter must be an integer from 0 through 2147483647. Assignment with
a different SRID fails with SQLCODE -301. An invalid type parameter fails
with -103.

Values can be POINT, LINESTRING, POLYGON, MULTIPOINT, MULTILINESTRING,
MULTIPOLYGON or GEOMETRYCOLLECTION. These are value kinds within one SQL
type, not structured SQL subtypes. There is no geometry-kind parameter.

## Functions

| Function | Result |
|---|---|
| `ST_POINT(x, y [, srid])` | Point from two finite numeric coordinates; SRID defaults to 0 |
| `ST_MAKEENVELOPE(min_x, min_y, max_x, max_y [, srid])` | Rectangular polygon from finite bounds with positive extent on both axes; SRID defaults to 0 |
| `ST_GEOMFROMTEXT(text [, srid])` | Geometry read from WKT; SRID defaults to 0 |
| `ST_GEOMFROMWKB(binary [, srid])` | Geometry read from WKB; SRID defaults to 0 |
| `ST_ASTEXT(geometry)` | WKT, without the SRID |
| `ST_ASBINARY(geometry)` | Little-endian WKB, without the SRID |
| `ST_SRID(geometry)` | The supplied signed 32-bit identifier |
| `ST_SETSRID(geometry, srid)` | The same coordinates with a different reference label; no transformation |
| `ST_X(point)`, `ST_Y(point)` | DOUBLE PRECISION ordinate; NULL for POINT EMPTY, -301 for another geometry kind |
| `ST_MINX(geometry)`, `ST_MAXX(geometry)` | Minimum or maximum X as DOUBLE PRECISION; NULL for an empty point set |
| `ST_MINY(geometry)`, `ST_MAXY(geometry)` | Minimum or maximum Y as DOUBLE PRECISION; NULL for an empty point set |
| `ST_ISEMPTY(geometry)` | INTEGER 1 if no component has coordinates, otherwise 0 |
| `ST_GEOMETRYTYPE(geometry)` | A SQL/MM-style name such as `ST_Point`; see the mapping below |
| `ST_INTERSECTS(left, right)` | INTEGER 1 if the closed XY geometries share any point, otherwise 0 |
| `ST_DISJOINT(left, right)` | INTEGER 1 if the closed XY geometries share no point, otherwise 0 |
| `ST_DISTANCE(left, right)` | Minimum planar distance as DOUBLE PRECISION, in coordinate units; NULL for an empty input |

Every function returns NULL if any argument is NULL. An empty geometry
is a value, distinct from NULL. `CAST(text AS GEOMETRY)` reads WKT with
SRID 0. `CAST(text AS GEOMETRY(srid))` reads WKT with the destination
SRID. Casting an existing geometry checks that reference without changing
it. Implicit text assignment is not enabled.

SRIDs are stored as supplied. Zero does not imply WGS 84. No CRS catalog
is consulted, no axes are exchanged and no coordinates are transformed.
To preserve SRID through WKT or WKB, pass it separately to the constructor.

`ST_MAKEENVELOPE` requires each minimum to be strictly less than its
maximum. Reversed or zero-area bounds raise SQLCODE -301; use `ST_POINT`
for a point query. It constructs a polygon from supplied numbers, and is
not the SQL/MM ST_Envelope method for deriving a geometry's envelope.
The four extrema observers work with all supported kinds and collections;
they do not construct a new geometry or alter its SRID.

`ST_GEOMETRYTYPE` returns these case-sensitive names. The generic
collection name is `ST_GeomCollection`, the SQL/MM spelling; this differs
from PostGIS's `ST_GeometryCollection`.

| WKT kind | SQL geometry type name |
|---|---|
| POINT | ST_Point |
| LINESTRING | ST_LineString |
| POLYGON | ST_Polygon |
| MULTIPOINT | ST_MultiPoint |
| MULTILINESTRING | ST_MultiLineString |
| MULTIPOLYGON | ST_MultiPolygon |
| GEOMETRYCOLLECTION | ST_GeomCollection |

## Supported representations

WKT is case-insensitive and accepts decimal and exponent coordinates.
MULTIPOINT members use parentheses, for example `MULTIPOINT ((1 2), (3 4))`.
WKB accepts either byte order, including mixed orders within a collection.
For POINT EMPTY it reads and writes the pair-of-NaNs convention used by
[GEOS](https://libgeos.org/specifications/wkb/). Other non-finite
coordinates are rejected. Z/M coordinates, curves and EWKB flags are
not supported.

Readers reject incomplete or trailing input, a one-point line, a polygon
ring with fewer than four coordinates or without its closing coordinate,
and a WKB collection containing a member of the wrong kind. They do not
check polygon self-intersection or whether holes lie inside their shell.
Successfully reading a polygon does not establish topological validity.

Intersection tests include shared boundaries and exclude polygon hole
interiors. They require valid polygon topology; these functions do not
validate it. Decisions use exact orientation signs for the supplied
binary64 coordinates, without a tolerance. They are Cartesian tests,
including when the SRID names a geographic coordinate system.

Both operands must have the same SRID, even when empty; a mismatch fails
with -301. An empty geometry intersects nothing, so `ST_INTERSECTS`
returns 0 and `ST_DISJOINT` returns 1 if either operand is empty. NULL
still propagates. Use `ST_INTERSECTS(a, b) = 1` in a WHERE or ON condition.
`ST_DISTANCE` also requires matching SRIDs and valid polygon topology.
It returns zero for intersecting geometries and NULL for an empty input.
Otherwise it finds the minimum distance between their points or boundary
segments. Its DOUBLE PRECISION result is approximate; a positive distance
smaller than that type can represent may round to zero. Use
`ST_INTERSECTS` for a topological test. A distance exceeding the finite
range raises SQLSTATE 22003 (SQLCODE -301). Geographic coordinates remain
angular coordinates: this function does not calculate metres or geodesics.

An R-tree on a `GEOMETRY(srid)` column can accelerate
`ST_INTERSECTS(column, value) = 1`, with a constant or parameter in either
argument position. It checks the exact geometry after finding envelope
candidates. An inner join can also use a geometry index when both columns
enforce the same SRID; each outer row supplies the query geometry and the
complete ON condition checks each candidate pair. See
[geometry indexes](../data/spatial/03-indexing.md).
ST_DISJOINT, distance comparisons in WHERE and geometry outer joins do
not yet use those probes. Ascending ST_DISTANCE with FETCH FIRST can use
a nearest-candidate traversal; the [distance guide](../data/spatial/05-distance.md)
lists its requirements and fallbacks.
With constant folding enabled and enough memory available, a constant
geometry operand is decoded once per expression and reused while evaluating
rows. This also applies when a prepared statement reuses its query plan;
each execution prepares its own parameter value.

SQL `=`, ordering, grouping and ordinary index keys compare the stored
representation, including SRID and coordinate order. This is not the
spatial ST_Equals relationship. For example, reversing a line produces a
different representation. Signed zero and interchange byte order are
normalized. B-tree/hash keys remain subject to their usual size limits.

Malformed geometry input fails with SQLCODE -301. Unsupported kinds,
dimensions and [representation limits](../appendix/d-limits.md) fail
with -900. Damaged stored payloads and unknown versions fail with -916.
The type and functions are a subset implemented as extensions; they do
not claim Simple Features or SQL/MM Spatial conformance.

## Example

With a schema selected:

```sql
SET SQL.FUNCTION_CALLS = 'ON';
CREATE TABLE places (id INTEGER, shape GEOMETRY);
INSERT INTO places VALUES
    (1, ST_POINT(4.9, 52.37, 4326)),
    (2, ST_GEOMFROMTEXT('POINT EMPTY', 4326));
SELECT id, ST_ASTEXT(shape), ST_SRID(shape), ST_ISEMPTY(shape)
FROM places ORDER BY id;
```

The rows, in order, are:

```text
1,POINT (4.9 52.37),4326,0
2,POINT EMPTY,4326,1
```

The SQL functions follow the existing 32,767-character/octet string
limits. Geometry values use a versioned payload within the existing
database formats. A host opening a database containing GEOMETRY must
enable the type before opening its files.
