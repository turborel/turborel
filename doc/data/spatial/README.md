# Spatial Data

[Specialized Data](../README.md)

Store locations and shapes, then use ordinary SQL to ask where they are
and which other objects they meet. A point can be a delivery address, a
line a courier route, and a polygon a service area with excluded grounds.

## Start with a task

The [delivery dispatch dataset](00-sample-data.md) supplies runnable data
for these workflows:

| Task | Example |
|---|---|
| Import delivery coordinates and spot missing locations | [Store and exchange geometries](02-interchange.md) |
| Show ready orders in the current map view | [Search a map window](03-indexing.md#show-ready-deliveries-in-a-map-window) |
| Match deliveries to service areas | [Join by intersection](04-relationships.md#join-by-intersection) |
| Count orders per area, including areas with none | [Report workload by zone](04-relationships.md#report-workload-by-zone) |
| Find orders that no area serves | [Find deliveries without a service area](04-relationships.md#find-deliveries-without-a-service-area) |
| Find routes affected by roadworks | [Check routes against roadworks](04-relationships.md#check-routes-against-roadworks) |
| Find depots within a pickup radius | [Search within a radius](05-distance.md#search-within-a-radius) |
| Choose the nearest depots, with stable tie-breaking | [Return the nearest depots](05-distance.md#return-the-nearest-depots) |

Read [coordinates and geometry](01-model.md) before using real-world
coordinates. In particular, distances depend on their units, and an SRID
label does not transform coordinates.

## What works today

| Capability | Availability |
|---|---|
| Seven basic XY geometry kinds, WKT and WKB | Included in `turboreld`; embedding applications register GEOMETRY |
| Numeric point and rectangle R-tree indexes | Built in |
| Geometry intersection filters and inner joins | R-tree candidates with exact geometry checks on fixed-SRID columns |
| Planar distances and nearest-neighbour queries | ST_DISTANCE; eligible nearest ordering can use an R-tree |
| Outer joins, grouped spatial reports and unmatched-row queries | Ordinary SQL; these forms do not all use spatial index probes |
| SRID labels and constraints | Stored and checked; no CRS catalog or coordinate transformation |
| Containment predicates, topology validation, geodesic distances and GIS metadata tables | Not implemented |

The geometry functions use the existing opt-in SQL call setting:

```sql
SET SQL.FUNCTION_CALLS = 'ON';
```

The [sample setup](00-sample-data.md) includes it. A new connection must
enable it too. Rust applications use the separate
[embedding instructions](../../client/03-embedding.md#geometry-values).

## Reference and scope

The [Geometry SQL reference](../../sql/spatial.md) lists signatures, NULL
handling and errors. [CREATE INDEX](../../sql/statements/create-index.md)
covers index syntax. The [indexing chapter](03-indexing.md) explains which
query forms can use spatial indexes and which currently scan.

This is an initial XY subset. It does not claim complete Simple Features
or SQL/MM Spatial conformance. SQL/MM structured geometry types, topology
validation and metadata remain separate work. See
[conformance](../../../CONFORMANCE.md#geometry-representations).
