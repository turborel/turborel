# Sustained spatial reads and writes

This benchmark shares one table of 100,000 points between one, four or
eight sessions. Each worker performs 100,000 operations: four complete
spatial reads for every committed update. One set of cases uses width-10
intersection windows; another fetches the ten nearest points, with IDs
breaking distance ties. Each worker retains its prepared statements and
commits after every operation.

Updates move existing geometries and maintain both the primary-key B-tree
and geometry R-tree. Workers own disjoint, interleaved ID partitions;
they wrap around their partition and move a point to a different location
on every visit. Readers query the entire table, including other workers'
updates. Every update must report success. This measures index and commit
contention without competing writes to the same logical row.

The harness creates a fresh database before the warmup and each of five
measured runs. After every run, outside its timer, it checks every row's
coordinates against a separately calculated final state. It also compares
16 intersection queries and 16 nearest queries against integer-coordinate
oracles. Setup, these checks and database teardown are untimed.

Read latency includes opening the query, fetching every result and commit.
Write latency includes executing the update and commit. Parameter
construction is outside the individual latency sample. Aggregate throughput
includes worker startup, parameter construction and statement preparation.
Read and write p50, p95 and p99 are reported separately, pooling all five
measured runs. Workers wait for each operation to finish before beginning
the next: these are closed-loop latencies, with no independent arrival queue.

```text
cargo xtask bench geometry_mixed --save geometry-mixed-sustained
```

`TURBOREL_MIXED_OPERATIONS` changes the per-worker count, rounded down to
a multiple of five with a minimum of five. The count appears in the case
label, so different run lengths do not compare as identical workloads.
The default eight-session case executes 800,000 operations per run,
including 160,000 committed updates, and revisits part of each partition.

Run on an otherwise idle machine. These in-memory, warm-cache cases do
not measure durable-file sync latency, same-row write contention, long
transactions retaining old versions, complex polygons or PostGIS.

## Measurements

The default workload completed on 2026-11-16, with Rust 1.99.1 on macOS
aarch64 and 10 available hardware threads. No other build or benchmark
ran alongside it. All final-state and query checks passed after every
warmup and measured run.

| Read workload | Sessions | Operations/s | Read p50 / p95 / p99, µs | Write p50 / p95 / p99, µs |
|---|---:|---:|---:|---:|
| Width-10 filter | 1 | 40,985 | 20.3 / 23.9 / 26.1 | 36.9 / 41.5 / 45.1 |
| Width-10 filter | 4 | 132,741 | 26.8 / 37.0 / 46.2 | 32.0 / 44.6 / 66.6 |
| Width-10 filter | 8 | 154,175 | 38.2 / 88.7 / 111.5 | 48.0 / 107.4 / 137.9 |
| Nearest 10 | 1 | 32,082 | 28.5 / 39.2 / 44.1 | 37.4 / 42.0 / 46.1 |
| Nearest 10 | 4 | 95,910 | 40.9 / 59.6 / 78.4 | 33.8 / 46.9 / 74.8 |
| Nearest 10 | 8 | 101,156 | 74.4 / 154.8 / 203.1 | 48.2 / 99.8 / 133.1 |

Throughput spreads were 0.2–2.5%. Each one-session case has 400,000 read
samples and 100,000 write samples; four and eight sessions multiply those
counts accordingly. Across both workloads, measured runs performed
13 million operations, including 2.6 million committed updates, plus
the untimed warmups. Each measured run lasted roughly 2.4–7.9 seconds.

Four sessions delivered about three times one-session throughput. Moving
from four to eight added 16% for filters and 5% for nearest queries while
increasing tail latency. These are baseline measurements of this
implementation, not an improvement over an earlier revision or another
database. Longer update histories, retained snapshots and durable files
remain separate workloads.

## After stored-WKB validation changed

The same sustained workload was repeated after replacing stored geometry's
decode-and-re-encode validation with direct canonical checks. The optional
polygon preparation index was also present, but these point and four-edge
window cases do not qualify for it. No other build or benchmark ran during
either measurement.

```text
cargo xtask bench geometry_mixed --save geometry-mixed-canonical --compare geometry-mixed-sustained
```

| Read workload | Sessions | Before, operations/s | After, operations/s | Change | After read p99, µs | After write p99, µs |
|---|---:|---:|---:|---:|---:|---:|
| Width-10 filter | 1 | 40,985 | 45,401 | +10.8% | 22.4 | 45.9 |
| Width-10 filter | 4 | 132,741 | 151,367 | +14.0% | 39.6 | 64.5 |
| Width-10 filter | 8 | 154,175 | 168,965 | +9.6% | 104.7 | 145.3 |
| Nearest 10 | 1 | 32,082 | 34,028 | +6.1% | 41.4 | 46.4 |
| Nearest 10 | 4 | 95,910 | 103,980 | +8.4% | 72.7 | 74.1 |
| Nearest 10 | 8 | 101,156 | 107,825 | +6.6% | 194.0 | 134.7 |

All final-state and query oracles passed again, after another 13 million
measured operations and 2.6 million committed updates. Throughput spreads
were 0.1–2.4%. All six throughput gains exceeded the harness's noise floor,
and read p99 fell in each case. Write p99 did not consistently improve:
for example, eight-session filters rose from 137.9 to 145.3 µs, while
eight-session nearest queries rose from 133.1 to 134.7 µs. Faster aggregate
reads do not establish faster write tails under contention.
