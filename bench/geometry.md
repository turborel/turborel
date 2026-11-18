# Geometry benchmarks

## Current workload

The harness uses 10,000 grid points and two query polygons: a square and
a 512-edge polygon. Each measured direct-intersection batch evaluates all
points 32 times. Each distance batch evaluates a scattered sample of 1,024
points 16 times. Each decoder batch reads 40,000 values. The longer batches
amortize timing and scheduling overhead; they do not change the SQL cases,
which execute 16 complete queries per worker on one, four and eight threads.

Prepared-function cases use the same point sequences and repetition counts.
They retain one query operand, then call the SQL scalar function with each
point value. These timings include decoding the varying point and constructing
the scalar result. A separate case constructs and drops 10,000 prepared
operands; the harness also prints their retained memory reservations.
Regular polygons with 4, 8, 16, 32, 64 and 128 edges add prepared-function
and preparation cases, to measure the boundary-size crossover separately
from the complete SQL scans.

The measurements below retain the workload used at the time. Comparisons
of a numerical or algorithmic change must run both revisions with the same
batch sizes; older raw-kernel timings are not that baseline.

## Preparing a constant operand

The initial intersection implementation decoded both geometry operands on
every row. Preparing a constant query geometry removes that repeated work.
The prepared representation belongs to one expression and is immutable;
there is no process-wide cache or lock.

The comparison was run on macOS aarch64 with 10 available hardware threads,
Rust 1.99.1, in the optimized bench profile, on 2026-11-16. Both revisions
used the same dataset, queries and benchmark harness:

```
cargo xtask bench geometry --save geometry-initial
cargo xtask bench geometry --save geometry-prepared --compare geometry-initial
```

The first command measured the implementation before constant-operand
preparation; the second measured the implementation with it. Saved TSVs
are in `target/bench/geometry-initial.tsv` and
`target/bench/geometry-prepared.tsv` when these commands are run.

The dataset contains 10,000 points on a 100 × 100 integer grid. Queries
use a square or a 512-edge polygon supplied as a prepared parameter.
Every result row is fetched. Setup and insertion are outside the timed
operation. Each worker opens its own session, prepares the statement and
executes four queries, rolling back each read transaction. The harness
warms each case once and measures five runs.

| SQL scan | Threads | Before, queries/s | Prepared, queries/s | Change reported by harness |
|---|---:|---:|---:|---:|
| Square | 1 | 177 | 288 | +62.5% |
| Square | 4 | 580 | 865 | +49.1% |
| Square | 8 | 596 | 838 | +40.6% |
| 512-edge polygon | 1 | 26 | 216 | +728.8% |
| 512-edge polygon | 4 | 66 | 713 | +976.9% |
| 512-edge polygon | 8 | 91 | 774 | +751.2% |

Rates are rounded here; percentage changes use the unrounded measurements.
The SQL measurement spread was 0.3–2.6% before and 0.9–2.4% after. Direct
intersection and decoding cases remained within the harness's noise
threshold. The improvement comes from avoiding repeated decoding, not
from a different intersection algorithm.

Preparation retains one decoded constant geometry and its payload per
expression, costing memory proportional to that operand. A changed operand
falls back to ordinary evaluation. Tests compare execution with constant
folding enabled and disabled, exercise repeated prepared parameters, and
check that preparation does not raise errors in skipped branches.

These are scans over a modest dataset. They do not measure indexed geometry
filters, mixed readers and writers, large spatial joins, nearest-neighbour
latency, or PostGIS. Those workloads need their own measurements before
claiming a general performance advantage. Rates from the raw geometry
cases are not SQL query rates.

## Keeping preparation after plan reuse

The four-execution comparison above did not reach physical-plan reuse.
After learning a stable plan, the implementation filled its parameter slots
without preparing scalar calls again. A complex polygon was then decoded
for every row. The corrected path folds and prepares expressions after
filling that execution's parameters, including constructors such as
`ST_GEOMFROMTEXT(?)`. It preserves runtime errors and honors the
constant-folding rule's setting and presence.

The benchmark now executes 16 queries per worker, including executions
after learning. The dataset, full result fetching, one warmup and five
measured runs are unchanged. Both sides of this comparison use 16 queries,
on the same machine and toolchain described above:

```
cargo xtask bench geometry --save geometry-kept-unprepared
cargo xtask bench geometry --save geometry-kept-prepared --compare geometry-kept-unprepared
```

| SQL scan | Threads | Before, queries/s | After, queries/s | Change reported by harness |
|---|---:|---:|---:|---:|
| Square | 1 | 205.2 | 289.3 | +41.0% |
| Square | 4 | 674.5 | 904.6 | +34.1% |
| Square | 8 | 704.3 | 865.5 | +22.9% |
| 512-edge polygon | 1 | 38.5 | 214.5 | +457.1% |
| 512-edge polygon | 4 | 100.2 | 668.6 | +566.9% |
| 512-edge polygon | 8 | 136.7 | 785.5 | +474.5% |

SQL spreads were 0.5–2.4% before and 0.4–3.3% after. More sessions do not
always improve throughput: four outperformed eight for square scans.
The unchanged direct distance cases also moved: square distance increased
15.6%, while complex-polygon distance decreased 6.8%, both flagged by the
harness. This change does not alter that calculation; these short kernel
measurements do not establish an algorithmic improvement or explain that
variation. The table measures complete SQL scans, not latency percentiles
or a comparison with PostGIS.

Preparation now reserves retained memory from the database budget and
declines if that reservation is unavailable. At this measured revision on
aarch64, a point reserved 202 bytes, the square 498 bytes and the 512-edge
polygon 24,882 bytes. Those totals include the retained payload, decoded
vector capacities and prepared object, not the decoder's transient peak
or allocator overhead. Clones share the reservation, released with the
last owner. Budget tests and mutation checks cover omitted coordinate
storage, stale operand reuse and lost preparation in learned plans.

## Allocating decoded WKB from its checked counts

WKB carries coordinate, ring and component counts. The decoder already
checked them against remaining input and the shared representation limit,
but collecting fallible iterators lost their exact allocation size. The
decoder now allocates from those checked counts before reading elements.
Malformed counts still fail before that allocation.

The same 16-query benchmark and machine measured this change:

```
cargo xtask bench geometry --save geometry-kept-prepared
cargo xtask bench geometry --save geometry-sized-decode --compare geometry-kept-prepared
```

| Decoding case | Before, values/s | After, values/s | Change reported by harness |
|---|---:|---:|---:|
| Square, WKB | 4,299,612 | 7,799,302 | +81.4% |
| 512-edge polygon, WKB | 397,937 | 968,453 | +143.4% |
| 512-edge polygon, SQL value | 287,604 | 489,237 | +70.1% |

The square WKB case is short, with 14.5% and 17.9% spreads. The complex
WKB case had 9.2% and 0.4%; SQL-value decoding had 2.4% and 0.2%. The
SQL-value path also checks canonical stored bytes, so it does more work
than reading interchange WKB. Complete SQL scans mostly stayed within
noise; only the four-session complex-polygon case was flagged faster,
by 9.6%. Their query polygon is already prepared once per execution.

The retained reservation fell from 498 to 378 bytes for the square and
from 24,882 to 16,634 bytes for the complex polygon; a point still reserves
202 bytes. The figures count requested vector capacities, not allocator
overhead or transient decoding memory. Unchanged direct distance cases
again moved in both directions, so these measurements support the decoding
and allocation changes, not a claim that the distance algorithm improved.

## Bounds for geometry distance pruning

The computed distance now respects a monotone conservative envelope bound.
Distance norms have explicit axis and outward L1 bounds, and exact
determinant reconstruction uses encoded powers of two. These changes
establish numerical conditions needed by indexed distance ordering; they
do not themselves add that SQL access path.

Both runs use the longer kernel batches described above, on the same
machine and toolchain, without another build or benchmark running:

```
cargo xtask bench geometry --save geometry-batched-before-bounds
cargo xtask bench geometry --save geometry-batched-bounds --compare geometry-batched-before-bounds
```

| Direct distance | Before, pairs/s | After, pairs/s | Change |
|---|---:|---:|---:|
| Point/square | 9,068,422 | 8,397,207 | -7.4% |
| Point/512-edge polygon | 167,512 | 156,662 | -6.5% |

The harness flagged both regressions. Square spreads were 4.4% and 5.9%;
complex-polygon spreads were 0.1% and 0.8%. SQL intersection scans and
decoders stayed within its noise threshold. This is a measured cost of
the numerical changes, not a speed improvement. No additional retained
geometry storage is introduced. The 1,000 independent rational-distance
fixtures still differ by at most one ULP; the requirement tests also cover
every binary64 exponent, monotone rectangle enlargement and finite-range
distance calculations. This is not a PostGIS or nearest-neighbor SQL result.

## Indexing a prepared polygon's edges

Prepared Polygon and MultiPolygon operands can retain a Y-interval tree
for each ring. Point location visits only subtrees whose closed Y range
can meet the query's horizontal ray, then applies the ordinary exact
crossing and boundary tests. Each polygon keeps its shell-minus-holes
meaning. Distance can reuse the intersection decision, but its separation
kernel still examines segments normally.

On the same macOS aarch64 machine and Rust 1.99.1 toolchain, an initial
run indexed every nonempty polygon. Prepared intersections for 4–32-edge
polygons stayed within noise while preparation became slower. The first
gain beyond noise was at 64 edges. The implementation now builds this
optional index only when at least one ring has 64 edges; many small rings
alone do not qualify. This threshold describes the measured crossover,
not a guarantee that every query amortizes preparation.

```text
cargo xtask bench geometry --save geometry-locator-crossover-before
cargo xtask bench geometry --save geometry-locator-all --compare geometry-locator-crossover-before
cargo xtask bench geometry --save geometry-locator-threshold --compare geometry-locator-crossover-before
```

The final threshold run measured:

| Prepared intersection | Before, pairs/s | After, pairs/s | Change |
|---|---:|---:|---:|
| 64-edge polygon | 9,803,196 | 10,634,306 | +8.5% |
| 128-edge polygon | 8,372,653 | 10,466,110 | +25.0% |
| 512-edge polygon | 4,779,579 | 10,364,442 | +116.8% |

| Complete 512-edge polygon SQL scan | Before, queries/s | After, queries/s | Change |
|---|---:|---:|---:|
| One session | 222.2 | 292.8 | +31.8% |
| Four sessions | 698.7 | 919.3 | +31.6% |
| Eight sessions | 791.2 | 896.5 | +13.3% |

Preparation has a measurable cost. For 64, 128 and 512 edges respectively,
building and dropping a prepared operand increased from about 0.55, 0.80
and 2.14 µs to 1.86, 3.42 and 14.16 µs. Retained reservations increased
from 2,298, 4,346 and 16,634 bytes to 4,954, 9,562 and 37,210 bytes. The
square still reserves 378 bytes. These figures include the decoded
geometry and payload, vector capacities and preparation objects, but
exclude allocator overhead. A refused index reservation keeps the decoded
operand available. Clones share the state and release it with the last
owner.

Prepared distance rates changed by only 0.5–1.8%, within noise: outside
points still require the segment-distance kernel. Square SQL scans also
stayed within noise. The harness flagged a 6.9% slowdown in the unchanged
square SQL-value decoder and an 8.5% slowdown in 8-edge preparation, while
some other small cases moved faster. Those movements are included in the
saved results; they are not evidence that the boundary index changes
decoding or accelerates small polygons. A query testing only a handful of
points may pay more preparation cost than it saves. This comparison does
not measure general polygon-to-polygon operations or PostGIS.

## Validating stored WKB without writing it again

Reading a SQL geometry previously decoded its WKB, encoded it into a
second vector and compared the bytes to reject noncanonical storage.
The stored reader now checks byte order, signed zero and the empty-point
NaN representation directly. Coordinate arrays get a contiguous signed-zero
check; the ordinary parser still checks finite coordinates, structure,
counts and complete input. Interchange accepts the same encodings as before.

This comparison keeps the prepared polygon index enabled in both builds.
It uses the same machine, toolchain and workloads as the preceding section,
without another build or benchmark running:

```text
cargo xtask bench geometry --save geometry-locator-threshold
cargo xtask bench geometry --save geometry-canonical-chunks --compare geometry-locator-threshold
```

| Stored-value decoding | Before, values/s | After, values/s | Change |
|---|---:|---:|---:|
| Square | 5,594,568 | 20,083,265 | +259.0% |
| 512-edge polygon | 483,478 | 808,654 | +67.3% |

The square decoder's spreads were 0.8% and 1.4%; the complex polygon's
were 0.3% and 0.1%. An initial implementation checked canonical bits
separately inside every ordinate read and made the complex decoder 16%
slower. That version was replaced by the contiguous check measured here.

| Complete SQL scan | Sessions | Before, queries/s | After, queries/s | Change |
|---|---:|---:|---:|---:|
| Square | 1 | 291.2 | 438.6 | +50.6% |
| Square | 4 | 905.7 | 1,213.7 | +34.0% |
| Square | 8 | 890.5 | 989.3 | +11.1% |
| 512-edge polygon | 1 | 292.8 | 441.4 | +50.8% |
| 512-edge polygon | 4 | 919.3 | 1,195.3 | +30.0% |
| 512-edge polygon | 8 | 896.5 | 991.8 | +10.6% |

SQL-scan spreads were at most 2.2%. Each scan still visits 10,000 points;
removing repeated stored-point encoding helps even when the query polygon
was already prepared. The prepared-scalar intersection cases, which include
decoding each varying SQL point, rose from 9.95 to 21.28 million pairs/s for
the square and from 10.36 to 21.56 million for the complex polygon.

Preparing a 512-edge operand rose from 70,639 to 82,798 operands/s, or about
14.16 to 12.08 µs per prepare-and-drop. Retained reservations are unchanged:
202 bytes for a point, 378 for the square and 37,210 for the indexed complex
polygon. The change removes transient encoding allocation, not the retained
geometry or its boundary index. Direct interchange decoding and the raw
distance kernels stayed within the harness's noise threshold.

## Filtering distance calculations

The first distance implementation used exact dyadic integers for every
point-to-segment projection. The next implementation uses outward-rounded
intervals to certify ordinary endpoint classifications and bound the
rounding uncertainty of perpendicular measures. Uncertain calculations
still use the exact path. This changes calculation cost, not the defined
distance or the polygon-validity requirement.

The same machine and harness measured both implementations:

```
cargo xtask bench geometry --save geometry-distance-exact
cargo xtask bench geometry --save geometry-distance-filtered --compare geometry-distance-exact
```

| Decoded geometry distance | Exact path, pairs/s | Interval filter, pairs/s |
|---|---:|---:|
| Point to square | 102,077 | 2,718,603 |
| Point to 512-edge polygon | 2,236 | 169,014 |

Each case uses 128 points scattered through the same 10,000-point grid,
including inside and outside points. The square case is short: its
measurement spread was 15.5% before and 8.6% after. The complex polygon
spread was 0.7% and 0.9%. These measurements establish a large improvement
over always using exact arithmetic; the precise ratios are not latency
guarantees. The SQL intersection cases stayed within the harness's noise
threshold. Some unchanged, short codec and intersection microbenchmarks
also varied substantially between these runs.

Distance tests include independently computed rational fixtures for
cancellation, subnormal coordinates and intermediate overflow, an integer
projection oracle over 10,000 deterministic cases, and every finite
binary64 exponent. Exact topological intersection tests remain separate
from approximate distance measurements. This comparison does not measure
SQL distance sorting, spatial indexes or PostGIS.

## Exact signs from expansions before big integers

The stage-A bound cannot prove a zero determinant, so every collinear
triple went on to intervals and then to allocating big integers. On
integer and other grid-aligned data such triples are common: points on a
square's edges, shared vertices, axis-parallel edges. When a product's
differences are exact, or one is an exact zero, orientation now decides
the sign from a four-term floating-point expansion (RULES.md D85),
falling back as before otherwise.

Measured on an Apple M4, Rust 1.99.0. The baseline is the parent commit:

```text
cargo xtask bench geometry --save q8-before
cargo xtask bench geometry --compare q8-before
```

| Case | Change |
|---|---:|
| Point/square intersection, decoded | +307.4% |
| Point/4-edge polygon, prepared intersection | +24.5% |
| Point/square distance | +10.9% |
| Every other kernel and SQL scan | within noise |

The complete joins of `geometry_shapes` moved +2.7% to +5.5%, all but
one within noise; their integer points rarely land on region boundaries.

This machine runs on-access malware scanning. The first run after a
build scans the new binaries and slowed every case, including decoders
this change does not touch, by 10-45%. Repeat a comparison after the
first run, as here, and discard a run whose unchanged cases moved.

## Exact cell certificates for prepared polygons

A prepared polygon of at least 32 edges now builds a grid of exact cells
once it has located eight points per edge plus 256 (RULES.md D85). A
point in a cell no edge touches is decided by one lookup; others keep the
interval tree or ring scan. Three combs join the suite: a base and 64,
1,024 or 8,192 narrow teeth over the point grid, so a horizontal ray
crosses many edges, the worst case for interval trees.

Measured on an Apple M4, Rust 1.99.0. The baseline is the parent commit
with the comb cases added. Each side ran twice and the second run is
reported:

```text
cargo xtask bench geometry --save cells-before
cargo xtask bench geometry --save cells --compare cells-before
```

| Prepared intersection | Before, pairs/s | After, pairs/s | Change |
|---|---:|---:|---:|
| 32-edge polygon | 23,388,607 | 27,335,241 | +16.9% |
| 64-edge polygon | 24,209,946 | 27,787,728 | +14.8% |
| 128-edge polygon | 23,920,391 | 27,649,171 | +15.6% |
| 512-edge polygon | 22,884,590 | 27,799,496 | +21.5% |
| 260-edge comb | 1,526,806 | 2,348,096 | +53.8% |
| 4,100-edge comb | 106,509 | 107,878 | within noise |
| 32,772-edge comb | 13,168 | 12,681 | within noise |

Above 32 edges the rate is now about 27.5 million pairs per second
whatever the polygon's size: decoding each SQL point dominates. The two
large combs gain nothing: their teeth are narrower than a cell can be at
1,024 cells per axis, so every cell over the teeth touches an edge and
falls back to the interval tree, whose horizontal rays cross nearly every
tooth (9.3 µs and 79 µs per point). Edge lists per boundary cell would
address that; they are not part of this change. Preparation rates stayed
within noise: certificates are not built until an operand has located
enough points. Unprepared decoders and kernels, whose code this change
does not touch, ran 5-11% slower in the same runs than in the baseline
runs half an hour earlier; that drift applies to both sides of the
prepared comparison.

## Edge intersection through grid buckets

Whether two polygons, a line and a polygon, or two lines intersect
tested every pair of edges. Lines and rings with at least 4,096 edge
pairs now file one side's edges in an exact dyadic grid over the two
envelopes' overlap and test each edge of the other only against the
edges filed under the cells it may touch (RULES.md D85). Edges whose
envelopes miss the overlap are neither filed nor probed. Three large
pairs join the suite: 4,096-edge circles whose boundaries cross, one
nested in the other, and interleaved 1,024-tooth combs whose envelopes
overlap but whose edges never meet.

Measured on an Apple M4, Rust 1.99.0, second runs of each side:

```text
cargo xtask bench geometry --save buckets-before
cargo xtask bench geometry --save buckets --compare buckets-before
```

| Decoded intersection | Before, pairs/s | After, pairs/s | Change |
|---|---:|---:|---:|
| 4,096-edge circles crossing | 368 | 11,589 | 31.5x |
| 4,096-edge circles nested | 80 | 60,489 | 756x |
| 1,024-tooth combs interleaved | 80 | 181 | 2.3x |

Every other case stayed within noise. Nested circles gain most: no edge
of the outer ring reaches the inner one's envelope, so the test is a
scan of the edges and one point location. The combs gain least: their
teeth are narrower than the cells, so each cell files edges of both.

## Prepared polygons keep their edges filed

A prepared polygon of at least 32 edges now files its edges in the grid
over its envelope from the second line or polygon it tests, and each
later operand probes only the filed edges near its own (RULES.md D85).
Unprepared, every test filed the larger side again, or tested every
pair of edges below 4,096 pairs. A new case tests 10,000 small squares,
one per grid point, against a prepared polygon, as a join's inner
candidates meet its prepared outer row.

Measured on an Apple M4, Rust 1.99.0, second runs of each side:

```text
cargo xtask bench geometry --save filed-before
cargo xtask bench geometry --save filed --compare filed-before
```

| Prepared intersection | Before, pairs/s | After, pairs/s | Change |
|---|---:|---:|---:|
| Squares against a 4,096-edge circle | 452,643 | 10,446,592 | 23.1x |
| Squares against a 4,100-edge comb | 160,982 | 4,572,038 | 28.4x |

Preparation rates stayed within noise, apart from the square's at -6.2%
with spreads of 3.2-4.2%; a square is never filed. The filed edges of a
4,096-edge polygon hold its cells' offsets and each edge once per cell it
may touch, reserved from the budget and released with the operand.
