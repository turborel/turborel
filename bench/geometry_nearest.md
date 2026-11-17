# Nearest geometry queries

This benchmark runs complete SQL queries over 100,000 points, then over
100,000 rectangular polygons:

```sql
SELECT id, ST_DISTANCE(g, ?) AS d
FROM points
ORDER BY d, id
FETCH FIRST 10 ROWS ONLY;
```

Each table has a geometry R-tree and a primary-key B-tree. Points have
integer coordinates in a 1,000 by 1,000 domain. Rectangles extend four
coordinate units right and up from those points. Queries use a separate
position sequence, offset by a quarter unit on each axis. Before timing,
every query's ten returned IDs are checked against integer squared
distances, with IDs breaking ties. This oracle does not call the geometry
implementation.

Each case has one, four or eight concurrent sessions. A worker keeps one
prepared statement, fetches every result and commits after every query.
Latency covers opening the cursor, index traversal or scanning, geometry
refinement, sorting, fetching and commit. Parameter construction is outside
each latency sample. Throughput also includes worker startup and statement
preparation. The harness warms once and measures five runs.

The default is 16 queries per worker per run. Set
`TURBOREL_NEAREST_QUERIES` for longer runs; the count appears in the case
label so different workloads do not compare as if they were identical.
Reported percentiles pool the five measured runs. Each worker waits for a
query to finish before starting the next: these are closed-loop latencies,
without an independent arrival queue.

```text
cargo xtask bench geometry_nearest --save before
cargo xtask bench geometry_nearest --save after --compare before
```

Run both builds with the same toolchain, data, settings, query count and
machine resources, without another build or benchmark competing for CPU.
The benchmark prints the physical plan through `Session::physical_plan`
after each dataset so the access path can be checked. Its results describe
these point and rectangle workloads;
they do not establish performance for complex polygons, heavy envelope
overlap, cold files, concurrent updates or PostGIS.

## Scan and index comparison

Measured on 2026-11-16 with Rust 1.99.1, macOS aarch64 and 10 available
hardware threads. Both builds used the same data, geometry functions,
indexes and 16 queries per worker. For the baseline only, the planner's
nearest-path selection was disabled with `.filter(|_| false)` after
`self.nearest(keys, input)` in `PhysicalPlanner::sort`. Removing that guard
restored the index path. The printed plans reported `SeqScan` for both
baseline datasets and `NearestScan` for both indexed datasets.

```text
cargo xtask bench geometry_nearest --save geometry-nearest-scan
cargo xtask bench geometry_nearest --save geometry-nearest-indexed --compare geometry-nearest-scan
```

| Dataset | Sessions | Scan, queries/s | Indexed, queries/s | Indexed p50 / p95 / p99, µs |
|---|---:|---:|---:|---:|
| Points | 1 | 29.9 | 28,616 | 31.4 / 43.3 / 60.5 |
| Points | 4 | 92.6 | 80,685 | 40.3 / 66.1 / 135.5 |
| Points | 8 | 121.2 | 71,165 | 79.5 / 180.7 / 247.9 |
| Rectangles | 1 | 16.4 | 30,573 | 29.7 / 40.1 / 55.0 |
| Rectangles | 4 | 48.8 | 73,539 | 45.1 / 65.0 / 75.1 |
| Rectangles | 8 | 67.0 | 64,880 | 84.0 / 195.5 / 245.4 |

The gain comes from stopping after a small candidate set instead of
evaluating distance for every row. These indexed runs are short: each
one-session case has only 80 measured latency samples, with 13–17%
throughput spread. Four- and eight-session cases have 320 and 640 samples.
Eight sessions delivered less throughput than four in this run; these
figures do not demonstrate linear scaling. Longer runs and concurrent
writes need their own measurements.

## Longer indexed runs

The same indexed build was run with 1,024 queries per worker. All 8,192
distinct query positions were validated for each dataset before timing.

```text
TURBOREL_NEAREST_QUERIES=1024 cargo xtask bench geometry_nearest --save geometry-nearest-1024
```

| Dataset | Sessions | Queries/s | p50 / p95 / p99, µs | Measured samples |
|---|---:|---:|---:|---:|
| Points | 1 | 35,570 | 26.8 / 37.8 / 43.0 | 5,120 |
| Points | 4 | 110,538 | 34.4 / 48.4 / 59.3 | 20,480 |
| Points | 8 | 95,945 | 75.4 / 147.4 / 184.3 | 40,960 |
| Rectangles | 1 | 34,636 | 27.8 / 35.3 / 39.1 | 5,120 |
| Rectangles | 4 | 104,018 | 36.5 / 48.1 / 59.0 | 20,480 |
| Rectangles | 8 | 95,438 | 73.1 / 150.6 / 189.4 | 40,960 |

Throughput spreads were 0.3–1.9%. More queries amortize worker startup and
initial plan learning, but individual runs are still fractions of a second.
The four-to-eight-session throughput drop remains. These are warm,
read-only, closed-loop results, not sustained-load or PostGIS results.

## Stored-geometry decoding

The polygon boundary preparation change was first checked with the same
1,024-query workload, saved as `geometry-nearest-locator`. All six cases
remained within noise of `geometry-nearest-1024`. The subsequent stored-WKB
reader removes a temporary re-encoding when decoding each candidate's
geometry; [the geometry benchmark](geometry.md#validating-stored-wkb-without-writing-it-again)
describes its validation rules and decoder measurements.

```text
TURBOREL_NEAREST_QUERIES=1024 cargo xtask bench geometry_nearest --save geometry-nearest-locator
TURBOREL_NEAREST_QUERIES=1024 cargo xtask bench geometry_nearest --save geometry-nearest-canonical-chunks --compare geometry-nearest-locator
```

The same machine and toolchain measured the final decoder, with all 8,192
query oracles passing for each dataset and both plans still using
`NearestScan`:

| Dataset | Sessions | Queries/s | Change from prior decoder | p50 / p95 / p99, µs |
|---|---:|---:|---:|---:|
| Points | 1 | 38,455 | +7.9% | 24.5 / 35.7 / 40.6 |
| Points | 4 | 116,184 | +7.4% | 32.4 / 46.0 / 57.3 |
| Points | 8 | 97,208 | +3.1%, within noise | 74.8 / 146.5 / 181.3 |
| Rectangles | 1 | 39,899 | +15.6% | 23.8 / 31.6 / 35.8 |
| Rectangles | 4 | 120,880 | +15.0% | 31.4 / 42.1 / 52.7 |
| Rectangles | 8 | 100,984 | +3.3%, within noise | 70.9 / 143.3 / 177.1 |

Throughput spreads were 0.2–1.4%. The decoder helps the one- and
four-session cases; the eight-session changes remain below the harness's
noise floor. There is still less throughput at eight sessions than four.
The candidate bounds, tie handling and SQL results are unchanged.
