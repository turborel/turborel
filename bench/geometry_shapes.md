# Spatial joins with varied geometry and distribution

Run `cargo xtask bench geometry_shapes --save before`, then run the same
command with `--compare before` on the change. The suite uses the shared
benchmark harness: one untimed warm-up and five measured runs per case.

| Features | Regions | Distribution and shape |
|---|---|---|
| 100,000 points | 32 | Uniform points; diamonds with diamond-shaped holes |
| 100,000 points | 256 | 80% of points and region centres in two dense clusters; holed diamonds |
| 100,000 points | 256 | Uniform points; concave polygons with 128 edges |
| 20,000 lines | 256 | Short segments plus 10% long segments; holed diamonds |

Each case runs this complete query with one, four and eight sessions:

```sql
SELECT F.ID, R.ID
FROM REGIONS R JOIN FEATURES F ON ST_INTERSECTS(F.G, R.G) = 1;
```

The feature table has an R-tree. A measured operation opens a cursor,
fetches every pair and commits its read transaction, including query
planning. Data loading and validation are outside the timer. The actual
physical plan is printed after each case. No timing threshold is asserted.

The independent oracle uses integer ray crossings, boundary membership
and segment intersections over the generated coordinates. It does not call
Turborel's geometry predicates. The complete sorted result is compared,
including duplicate feature locations, polygon holes and shared boundaries.

Coordinates and SQL parameter types are the same in every build. The 256
region cases require more than one boundary-mask leaf in an adaptive
scan. The printed plan records whether the build being measured supports
that path. These cases assess larger inputs and geometry refinement costs
alongside points against rectangles.

The feature tables are synthetic, embedded and in memory. Temporary spill
files use disk by default, with the normal 1 GiB database memory budget
and 4 MiB work areas. Their writes and reads are inside the timed query.
`TURBOREL_SHAPES_SPILL=memory` instead uses `Database::new()`'s default
in-memory files, which count against that same budget. The case label
records the choice; compare only identical modes. Resource exhaustion is
a failed run, not a throughput result, and the budget is never enlarged
automatically to make a case pass.

The suite does not measure durable table I/O, concurrent updates, CRS transformations, geodesic distances,
multipart geometries or inputs larger than memory. Report those limitations
with any results; this suite alone does not establish general geospatial
performance or an advantage over PostGIS.
