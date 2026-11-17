# Geometry joins through an R-tree

This benchmark joins 100,000 points to 32 polygons through
`ST_INTERSECTS(P.G, W.G) = 1`, fetching every pair. The baseline evaluates
the Cartesian product. The indexed build probes the points' R-tree for
each polygon and checks the complete geometries of the candidates.

The current harness also writes the point table first in the SQL join.
Those cases have `points first` in their labels. Both written orders return
the same independently checked pairs. The planner can reverse the inputs;
measuring both orders also covers the projection that restores SQL column
order. Each case prints its actual physical plan. The historical results
below measured only the query with windows written first.

The run used Rust 1.99.1 on macOS aarch64, with 10 available hardware
threads, on 2026-11-16. Both builds used the optimized bench profile,
fat LTO, one code-generation unit and overflow checks. No other build
or benchmark ran during the timed operations.

```
cargo xtask bench geometry_join --save geometry-join-product
cargo xtask bench geometry_join --save geometry-join-indexed --compare geometry-join-product
```

For the baseline build only, `physical/indexed/spatial.rs` returned
`best.filter(|_| false)` from `geometry_probe`, preventing selection of
the geometry join path. The second build restored `best`. Both builds
held the same data, maintained the same indexes and used the same scalar
functions. `Session::physical_plan` confirmed a Cartesian product followed
by a filter before, and an index nested-loop join afterward.

## Results

The points and query positions use the same deterministic coordinate
sequence as the [geometry filter benchmark](geometry_sql.md). Each query
joins all 32 square windows. Width 10 produces 408 pairs; width 500
produces 408,781. Before timing each case, the benchmark compares every
returned pair with a separate integer-coordinate inclusion calculation.
Both builds passed that check for both widths.

Each worker opens a session, starts the complete SQL query, fetches all
rows and commits its read transaction. The rate includes worker startup,
parsing and planning. Each case is warmed once and measured five times;
the table reports the median aggregate rate across workers.

| Window width | Sessions | Product, joins/s | Indexed, joins/s |
|---|---:|---:|---:|
| 10 | 1 | 0.771 | 1,260.770 |
| 10 | 4 | 2.317 | 2,882.189 |
| 10 | 8 | 3.221 | 2,488.207 |
| 500 | 1 | 0.707 | 1.857 |
| 500 | 4 | 2.094 | 5.962 |
| 500 | 8 | 3.022 | 7.370 |

Narrow joins avoid almost all of the 3.2 million possible pairs. Their
indexed rate improves by roughly 773–1,635 times in these cases. The
indexed single-session operation takes about 0.79 ms, including startup
and planning, so these are very short measurements. Their throughput
spread was 0.3–4.3%. Eight sessions were slower than four; the result does
not show linear scaling.

Broad joins improve by 2.44–2.85 times. Returning over 400,000 pairs costs
work even after candidate pruning. Their indexed spread was 1.6–2.2%.
These figures are complete joins per second, not individual geometry
comparisons, index probes or returned pairs per second. No latency
percentiles were collected for this suite.

## What the measurement leaves open

The database is embedded and in memory. These runs include no network,
durable file I/O or concurrent writes. The polygons are simple squares;
polygon holes and visible-version changes are covered by correctness
tests, but their performance is not measured here. The filter suite
separately measures short mixed read/write bursts.

The planner currently uses a generic two-dimensional range estimate:
The printed plan estimates 11,111 candidates per window for both widths.
Actual average result counts here are 12.75 and 12,774.4. Geometry distribution
statistics and the cost of complex shape checks are not modeled. Other
distributions can therefore need different plan choices.

An indexed join streams its outer rows and candidates; it does not build
a table of all matching pairs. Resident memory, index maintenance costs
and long-running contention were not measured. The implementation probes
one tree per outer row, rather than traversing two spatial indexes together.
This is a comparison of turborel's own execution paths, not a PostGIS
benchmark.
