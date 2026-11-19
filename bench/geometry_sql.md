# Geometry SQL filters and concurrent updates

An R-tree now supplies envelope candidates for
`ST_INTERSECTS(G, ?) = 1` on a `GEOMETRY(srid)` column. The executor fetches
the visible row and checks its complete geometry before returning it.
This comparison measures that access path against a full table scan.
Both builds maintain the same R-tree during writes.

The run used Rust 1.99.1 on macOS aarch64, with 10 available hardware
threads, on 2026-11-16. The bench profile uses optimization, fat LTO,
one code-generation unit and overflow checks. No other build or benchmark
ran alongside it.

```
cargo xtask bench geometry_sql --save geometry-scan
cargo xtask bench geometry_sql --save geometry-indexed --compare geometry-scan
```

For the first build only, `physical/access.rs` passed an empty condition
slice to `spatial::candidates`, disabling geometry filter selection.
The second build restored the original condition slice. Storage, data,
index maintenance and benchmark settings were otherwise the same.
`Session::physical_plan` showed a sequential scan before and a geometry
index scan with the complete residual filter after.

## Workload

The database holds 100,000 deterministic points in a 1,000 × 1,000
coordinate domain, a primary key and a geometry R-tree. Query windows
come from the benchmark's separate fixed sequence. Narrow windows are
10 units wide and return 2–14 points across the 128 windows used, with
a median of 13. Broad windows are 500 units wide and return 45–25,107
points, with a median of 13,103.5. Windows near the domain's edge can
extend past it, so equal widths do not imply equal result counts.

Every worker opens its own session, prepares its query once, executes
16 changing windows and fetches every result. Each query commits its
read transaction. The latency timer covers opening, fetching and
committing; it excludes constructing the query geometry. Throughput
also includes worker startup and statement preparation. The harness
warms each case once and measures five runs.

| Window | Workers | Scan, queries/s | Index, queries/s |
|---|---:|---:|---:|
| Narrow | 1 | 21.6 | 27,275 |
| Narrow | 4 | 68.4 | 64,679 |
| Narrow | 8 | 95.4 | 64,322 |
| Broad | 1 | 20.8 | 27.6 |
| Broad | 4 | 61.8 | 68.8 |
| Broad | 8 | 91.7 | 96.7 |

The narrow-window indexed throughput spread was 2.4–5.5%; the broad
spread was 0.4–3.2%. Narrow queries avoid decoding nearly all stored
geometries. Broad queries still fetch many rows, so their benefit is
much smaller. These end-to-end results include prepared-plan learning;
they do not isolate tree traversal from planning and scalar preparation.

| Narrow window | Scan p50, µs | Index p50, µs | Scan p95, µs | Index p95, µs | Scan p99, µs | Index p99, µs |
|---|---:|---:|---:|---:|---:|---:|
| 1 worker | 54,491 | 32.5 | 56,306 | 36.8 | 57,392 | 55.9 |
| 4 workers | 68,469 | 53.2 | 72,946 | 66.3 | 77,413 | 71.8 |
| 8 workers | 94,921 | 85.7 | 105,132 | 189.5 | 109,320 | 224.3 |

There are only 80, 320 and 640 timed queries respectively. In particular,
the single-worker p99 is a small-sample observation. Each worker waits
for one operation before starting another: these are closed-loop
latencies, without an independent arrival queue or a correction for
coordinated omission. They are not production latency guarantees.

## Mixed reads and writes

Each worker performs 100 operations: 80 narrow spatial filters and
20 committed geometry updates. Writers update distinct row IDs. Every
round starts with a fresh 100,000-row database outside the timer, so
later rounds do not inherit extra versions or moved points. Reads use
the default read-committed isolation; lock waits are enabled.

| Workers | Scan, operations/s | Index, operations/s |
|---|---:|---:|
| 1 | 23.7 | 27,042 |
| 4 | 69.2 | 81,060 |
| 8 | 100.0 | 86,791 |

Indexed throughput spread was 1.2–2.8%. These are short bursts, with at
most 160 writes in a measured database. They establish that readers and
writers can use the geometry path together; they do not measure
long-running version reclamation, sustained contention or checkpoint
pressure. The database is embedded and in memory, with no network or
durable file I/O in these measurements.

## Scope and costs

The index retains four envelope coordinates per entry, row identifiers,
tree pages and counted entries for distinct visible versions. Empty
geometries use an origin envelope and can add false positives; NULLs
have no entry. These runs did not measure resident-memory growth or
write amplification. Candidate rows still require a geometry decode and
an exact intersection check.

These measurements compare turborel's own scan and indexed access paths.
They do not compare PostGIS, measure spatial joins or nearest-neighbour
queries, or establish performance on million-row datasets. Broad-window
selectivity, complex shapes, sustained writes and durable operation need
separate workloads.

A subsequent run saved as `geometry-indexed-validated` checked the exact
row IDs for all 256 narrow and broad windows against integer-coordinate
inclusion before timing. Every result matched. The timed query workloads
were unchanged; most rates remained within the earlier measurement noise.

## Reusing prepared geometry plans

The next comparison includes preparing calls when a learned plan is filled,
recognizing equivalent prepared spatial functions while learning, and
allocating decoded WKB from its checked element counts:

```
cargo xtask bench geometry_sql --save geometry-indexed-validated
cargo xtask bench geometry_sql --save geometry-indexed-reused --compare geometry-indexed-validated
```

| Workload | Sessions | After, operations/s | Change reported by harness |
|---|---:|---:|---:|
| Narrow filter | 1 | 34,998 | +29.5% |
| Narrow filter | 4 | 80,740 | +22.8% |
| Narrow filter | 8 | 73,756 | +11.3% |
| Broad filter | 1 | 32.2 | +18.2% |
| Broad filter | 4 | 104.3 | +43.2% |
| Broad filter | 8 | 138.8 | +40.0% |
| 80% filters / 20% updates | 1 | 31,318 | +17.9% |
| 80% filters / 20% updates | 4 | 95,837 | +18.9% |
| 80% filters / 20% updates | 8 | 94,229 | +13.6% |

The dataset, operation counts and pre-timing result validation are unchanged.
For narrow filters, p50/p95/p99 latency was 21.4/37.1/49.9 µs with one
session, 38.5/62.5/66.0 µs with four, and 71.1/193.5/218.1 µs with eight.
These remain small closed-loop samples, not sustained-load latency claims.
Four sessions outperform eight in the narrow and mixed cases. The complete
comparison measures the combined changes; it does not isolate their
individual contributions.

## Realistic shapes, large joins, disk pressure and sustained moves

`geometry_sql/realistic.rs` runs complete queries over shapes generated
deterministically in the bench (`geometry_sql/shapes.rs`), never
downloaded: 22,500 parcels, star-shaped polygons of 32 to 256 vertices
(121 on average) on a jittered grid of a 10,000-unit square, and 3,600
road pieces of 17 vertices wiggling along 60 lines each way. Both tables
have an `rtree` and their statistics gathered. `TURBOREL_GEOMETRY_REALISTIC=1`
runs these cases alone. Each case is a complete query, opened, fetched
whole and committed, with p50 and p99 of every sample.

One run in release on Linux x86-64, 2026-11-18:

| Case | Sessions | Rate | p50 | p99 |
|---|---:|---:|---:|---:|
| Join of every parcel with every road it meets | 1 | 0.07 queries/s | 14.2 s | 18.8 s |
| Parcel windows of side 400, in memory | 1 | 53 queries/s | 18.6 ms | 26.8 ms |
| Roads within 150 of a parcel, in memory | 1 | 353 queries/s | 2.3 ms | 8.2 ms |
| Parcel windows, on disk, 8 MiB pool | 1 | 54 queries/s | 17.7 ms | 30.1 ms |
| Roads within 150, on disk, 8 MiB pool | 1 | 376 queries/s | 2.2 ms | 7.8 ms |
| Parcel windows, one in five operations a move | 4 | 254 operations/s | reads 18.0 ms, writes 0.13 ms | reads 41.8 ms, writes 0.24 ms |

The join runs as an adaptive spatial join, each road probing the parcels
or one bounded scan, and nearly all its time is the exact tests of
polygons against lines. The windows read about 45 parcels each through
the index, and their time too is mostly exact tests of polygons of many
vertices. The database on disk had a pool of 8 MiB for some 60 MiB of
data, but the operating system's cache held its files, so it shows the
pool's misses, not the disk's. The sustained case moves parcels of
disjoint partitions while the others read, on a fresh database each run.
Latencies are closed loop, five runs of each case pooled, with no
independent arrival queue.
