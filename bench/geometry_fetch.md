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
