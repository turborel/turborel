# Spatial Data

[Specialized Data](../README.md)

Spatial data describes where something is and what shape it has. A shop
may be a point, a road a line, and a parcel a polygon. Coordinates supply
the numbers; the geometry and its coordinate system give them meaning.

This guide is for SQL users. It covers geometry values and functions,
geometry indexes, and queries over numeric coordinates.

## Read the guide

1. [Coordinates and geometry](01-model.md): coordinate systems, shape
   kinds, empty values and the meaning of equality.
2. [Store and exchange geometries](02-interchange.md): creating a table,
   importing WKT/WKB, retaining SRIDs and reading values back.
3. [Query spatial data with an R-tree](03-indexing.md): geometry
   intersection filters, point windows, rectangle overlap and performance.
4. [Test spatial relationships](04-relationships.md): geometry
   intersections, polygon holes, boundaries and spatial joins.
5. [Measure distances](05-distance.md): radius searches, nearest results
   and the distinction between coordinate units and metres.

## What works today

| Task | Availability |
|---|---|
| Store numeric XY points or rectangles and index them with `rtree` | Built in |
| Filter and join numeric coordinate columns | Built in; the planner can choose R-tree probes |
| Store the seven basic XY geometry kinds | Included in `turboreld`; embedding applications register GEOMETRY |
| Import/export geometry WKT and WKB | Available with the geometry extension |
| Construct rectangular search windows and inspect geometry bounds | Available through ST_MAKEENVELOPE and the four extrema observers |
| Retain, inspect or constrain a column to an SRID | Available; identifiers are not resolved against a CRS catalog |
| Test XY intersection or disjointness | Available; valid polygon topology required |
| Measure planar distance between geometries | Available; valid polygon topology required |
| Evaluate containment or other topological relationships | Not implemented |
| Index geometry intersection filters | Available on `GEOMETRY(srid)` columns; exact geometry rechecks |
| Accelerate inner joins between geometry columns with R-trees | Available when both columns enforce the same SRID |
| Accelerate SQL nearest-neighbour ordering with an R-tree | Available for ascending ST_DISTANCE with FETCH FIRST on GEOMETRY(srid); candidates are refined against visible geometries |
| Discover geometry columns through GIS metadata tables | Not implemented |
| Transform coordinate systems or calculate geodesic distances | Not implemented |

The supplied `turboreld` program includes the geometry type and functions.
Enable function calls in the SQL session before using the examples:

```sql
SET SQL.FUNCTION_CALLS = 'ON';
```

Embedding applications register the geometry extension explicitly.
Application developers can use the separate
[embedding instructions](../../client/03-embedding.md#geometry-values).
Numeric R-tree queries need no geometry registration.

## Standards and scope

The geometry representation implements the seven basic XY kinds from
OGC Simple Features. WKT and WKB follow the corresponding representation
formats, with the documented empty-point WKB convention. This is an
initial subset, not a claim of complete Simple Features or SQL/MM Spatial
conformance. SQL/MM structured geometry types, topology validation and metadata
remain separate work. See [conformance](../../../CONFORMANCE.md#geometry-representations)
for the precise implemented scope.

The [Geometry SQL reference](../../sql/spatial.md) lists every current
function. [CREATE INDEX](../../sql/statements/create-index.md)
defines the index syntax and transaction requirements.
