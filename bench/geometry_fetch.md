# Geometry row-fetch locality

`cargo xtask bench geometry_fetch --save initial` reads 100,000 indexed
points through 32 broad spatial windows. Every candidate causes a full
row fetch and geometry decode. The same 408,781 candidates are read in
R-tree traversal order or sorted by row id in blocks of 64, 256, 1024,
4096 or 32768 entries. Each mode runs on one, four and eight threads.

The integer-coordinate oracle checks the complete candidate count before
timing. The standard harness warms each case once, measures five runs,
and reports the median rate and spread. Database and index construction
are outside the timer; candidate collection and sorting are inside it.

This isolates row-store locality: it uses `NO_TRANSACTION` and does not
evaluate SQL predicates, check visible projections or encode wire results.
Rows per second here cannot be compared with complete SQL queries per
second. Use `geometry_sql`, `geometry_join` and `geometry_mixed` to check
whether a storage improvement survives those additional costs.

The production geometry index caps sorting blocks at 4096 candidates.
That is 160 KiB of envelope/row-id elements per active cursor, with the
actual allocated capacity reserved from the database memory budget. Small
queries retain a 64-entry buffer; allocation failure preserves streaming.
Nearest-neighbor traversal keeps its distance order and uses no such sort.

## Phases of a broad read

`geometry_fetch` also takes one broad read apart (`geometry_fetch/phases.rs`).
The same 32 windows of width 500 are read five ways, each adding one
phase to the one before. The first only walks the tree. The second also
fetches each candidate's row, sorted by row id in blocks of 4096 as the
production cursor sorts them. The third decodes its geometry, the fourth
tests it exactly against the window, and the fifth encodes the row as the
wire protocol's Rows reply encodes values, the geometry's text included.
`TURBOREL_FETCH_PHASES=1` runs these and the planner's weights alone.

One run in release on Linux x86-64, 2026-11-18, one thread:

| Phase | Rows/s | Time a row adds |
|---|---:|---:|
| 1 tree traversal | 49,870,073 | 0.02 µs |
| 2 + row materialization | 2,182,506 | 0.44 µs |
| 3 + geometry decode | 1,926,538 | 0.06 µs |
| 4 + exact test | 153,019 | 6.02 µs |
| 5 + wire encoding | 142,741 | 0.47 µs |

The tree is a small share of a broad read. The exact test of each
candidate dominates, then row materialization and wire encoding, whose
cost for points is mostly the geometry's text.

The weights of RULES.md D137 come from the same run. A row of a table
scan took 229 ns (4,368,738 rows/s), a point decoded 40 ns (25,130,686
values/s) and a polygon of 129 vertices 833 ns (1,200,010 values/s),
6.2 ns a vertex.
