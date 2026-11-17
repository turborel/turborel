# Coordinates and Geometry

[Spatial Data](README.md) · [Sample data](00-sample-data.md) · [Next: store and exchange geometries](02-interchange.md)

The [dispatch examples](00-sample-data.md) keep addresses as points, routes
as lines and service zones as polygons. Choose the shape that answers the
question: a route's centre or bounding rectangle cannot tell you whether
the route itself meets a road closure.

## Coordinates need a coordinate system

An XY coordinate pair has no unit by itself. `(4.9, 52.37)` might be
longitude and latitude in degrees; `(100, 200)` might be coordinates in
metres on a projected map or millimetres on a machine bed. You need the
coordinate system to interpret either.

Choose one coordinate system for values you intend to compare. An SRID
labels that choice. Turborel currently stores the supplied signed
32-bit identifier without looking it up, validating the coordinate
range, swapping axes or transforming coordinates. SRID 0 means no
reference identifier was supplied; it does not select WGS 84. In the sample
dataset, metres come from the agreed local grid, not from the SRID.

Keep a consistent X/Y convention when importing. For longitude/latitude
data, make the convention explicit in the application and verify it
against the source. Giving a value SRID 4326 does not fix reversed axes.
Changing an SRID label also does not reproject the coordinates.

The current R-tree and geometry envelopes use Cartesian XY bounds.
Ordinary numeric distance calculations on longitude/latitude operate
in degrees, not metres. They do not account for the Earth's shape. A
window crossing the longitude wrap needs separate coordinate ranges;
putting its larger longitude before its smaller longitude creates an
invalid rectangle. Polar and global distance queries need additional
coordinate-system-aware support.

## Geometry kinds

GEOMETRY is one registered SQL type with these value kinds:

| Kind | Represents | WKT example |
|---|---|---|
| POINT | One location | `POINT (1 2)` |
| LINESTRING | An ordered path of straight segments | `LINESTRING (0 0, 3 4)` |
| POLYGON | An outer ring and optional holes | `POLYGON ((0 0, 4 0, 4 4, 0 0))` |
| MULTIPOINT | A collection of points | `MULTIPOINT ((1 2), (3 4))` |
| MULTILINESTRING | A collection of lines | `MULTILINESTRING ((0 0, 1 1), (2 2, 3 3))` |
| MULTIPOLYGON | A collection of polygons | `MULTIPOLYGON (((0 0, 4 0, 4 4, 0 0)))` |
| GEOMETRYCOLLECTION | A collection of different kinds | `GEOMETRYCOLLECTION (POINT (1 2), LINESTRING (0 0, 3 4))` |

Keep one location per delivery when it needs one address. Use a polygon
for a service area; its holes can describe excluded grounds. Use a
MultiPolygon when one named service area has disconnected parts. A
GeometryCollection can retain mixed imported shapes, but makes a poor
substitute for separate rows when each object needs its own name or status.

A nonempty line has at least two points. A polygon ring repeats its
first coordinate at the end and has at least four coordinates including
that repetition. All stored coordinates are finite. Z and M ordinates,
curved segments and SQL subtype declarations are not supported yet.

## Empty is different from unknown

`POINT EMPTY` is a known geometry containing no point. SQL NULL means
there is no geometry value. The same distinction applies to empty lines,
polygons and collections.

An empty geometry still has its geometry kind and SRID. A collection
containing only empty components also has no coordinates. The current
constructors and observers propagate NULL; `ST_ISEMPTY` returns 1 for an
empty geometry, 0 for one with coordinates, and NULL for a NULL argument.

For a delivery whose address has not been geocoded, use NULL. An empty
geometry is useful when preserving an upstream geometry value that is
explicitly empty; it does not put the delivery at `(0, 0)`. Keep such rows
out of dispatch and report them for correction.

An empty geometry has no extent or location. It never intersects another
geometry, and its distance to another geometry is NULL.

## Representation checks are not topology checks

Readers validate the structure: coordinate counts, finite ordinates,
closed rings, matching member kinds and complete input. They do not yet
establish that a polygon is topologically valid. A closed ring can cross
itself, and a hole can lie outside its outer ring. Topology validation is
separate from structural decoding and is not implemented yet.

Likewise, an envelope describes the extent of a shape, not its interior.
Two envelopes can overlap while their polygons do not. A point can lie
inside a polygon's envelope but inside a hole. Exact geometry predicates
must examine the actual shapes after an envelope search.

## What equality means today

SQL `=` compares the stored representation, including SRID, geometry
kind and coordinate sequence. It does not implement the topological
ST_Equals relationship. Two lines with opposite coordinate order can
describe the same path but compare unequal. Reordering polygon rings or
starting a ring at a different vertex also changes its representation.

Interchange byte order and signed zero are normalized. Thus equivalent
little- and big-endian encodings of the same coordinate sequence compare
equal, and `-0` is stored as `0`. Coordinate sequences are otherwise kept
as supplied. Ordinary UNIQUE constraints use this representation equality.

SQL ordering of geometries is a deterministic ordering of their stored
bytes. It is not a geographic ordering and does not identify nearby rows.
