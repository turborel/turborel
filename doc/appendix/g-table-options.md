# Appendix G: Table Options

[Contents](README.md)

Each option of a table of the built-in storage engine, `rowstore`, as
INFORMATION_SCHEMA.TABLE_OPTIONS lists it. A table sets one by
[CREATE TABLE](../sql/statements/create-table.md) ... WITH ( ... ), and
changes it by [ALTER TABLE](../sql/statements/alter-table.md) SET
( ... ); one set to DEFAULT takes its default again. *Changed* says how a
change is made: in place, the rows as they are, or by a rebuild, the
table made anew and its rows copied, as ALTER TABLE ... USING makes it.

| Option | Default | Values | Changed |
|---|---|---|---|
| COLUMNS.FREEZE_DELAY | 0 | INTEGER FROM 0 TO 31622400 | in place |
| COLUMNS.FREEZE_ROWS | 0 | INTEGER FROM 0 TO 9223372036854775807 | in place |
| COLUMNS.GROUP_ROWS | 65536 | INTEGER FROM 2048 TO 65536 | in place |
| ENCODINGS | none | ENCODINGS, BY COMMAS | in place |
| LAYOUT | ROWS | ONE OF ROWS, COLUMNS | rebuild |
| OUT_OF_LINE | 0 | INTEGER FROM 0 TO 1073741824 | rebuild |
| STATISTICS.BUCKETS | 100 | INTEGER FROM 1 TO 10000 | in place |
| STATISTICS.REFRESH_FRACTION | 0.1 | NUMBER FROM 0 TO 1000 | in place |
| STATISTICS.REFRESH_ROWS | 50 | INTEGER FROM 0 TO 9223372036854775807 | in place |
| STATISTICS.SAMPLE | 10000 | INTEGER FROM 1 TO 1000000 | in place |
| TRANSACTIONS.HELD_BYTES | 1048576 | INTEGER FROM 4096 TO 1099511627776 | in place |
| TRANSACTIONS.TRACKED_ROWS | 65536 | INTEGER FROM 1 TO 9223372036854775807 | in place |

Each in a sentence:

- **COLUMNS.FREEZE_DELAY**: The seconds a row of a table of the COLUMNS layout stays as it was written after its commit before it may be frozen, so rows changed soon after are not.
- **COLUMNS.FREEZE_ROWS**: The rows committed past the last group after which a table of the COLUMNS layout freezes them; 0 for COLUMNS.GROUP_ROWS.
- **COLUMNS.GROUP_ROWS**: The rows of each group a table of the COLUMNS layout freezes; groups frozen before keep theirs.
- **ENCODINGS**: The registered encodings, by commas, such as a compression or an encryption with its key (AES-256-GCM:KEY), that the table's rows pass through as they are written, in its storage and the log; rows written before a change are read through their own. See [Encryption](../operator/07-encryption.md).
- **LAYOUT**: How the table keeps its rows: ROWS, each row's values together; COLUMNS, rows written kept so until they are frozen into groups, each column's values together.
- **OUT_OF_LINE**: Character values longer than this, in bytes, are kept out of their row's record; 0 for a quarter of a page.
- **STATISTICS.BUCKETS**: The buckets of each column's histogram.
- **STATISTICS.REFRESH_FRACTION**: The share of the table's rows changed, and STATISTICS.REFRESH_ROWS more, after which its statistics are gathered again.
- **STATISTICS.REFRESH_ROWS**: The changes past STATISTICS.REFRESH_FRACTION of the rows after which the statistics are gathered again.
- **STATISTICS.SAMPLE**: The most values of each column the statistics sample.
- **TRANSACTIONS.HELD_BYTES**: The bytes of a transaction's changes to the table held in memory, to undo them and check others' reads by; past them, changes are written to a spill area, and a committed one's rows kept as each column's least and greatest value.
- **TRANSACTIONS.TRACKED_ROWS**: The rows of a transaction's reads, or writes, of the table tracked one by one to check its commit; past them, every row is counted, which may refuse a commit that need not be.

## The COLUMNS layout

A table of the COLUMNS layout keeps the rows written to it as a table of
the ROWS layout does, each row's values together, so writing and
fetching a row by its key cost what they cost there. Once a group's
worth of them, COLUMNS.GROUP_ROWS, is committed and no transaction may
still see them otherwise, they are frozen: written again as a group of
each column's values together, and the least and greatest of each page
of a column's values kept. A query that reads a few columns of many
rows then reads only those columns' pages, and a condition on a column
skips the pages whose values it cannot match. The more of a table's rows
are frozen, and the fewer of its columns a query reads, the faster it
reads them.

A frozen row may be updated or deleted as any other; it is then kept as
rows written are, its frozen values no longer read. Fetching one frozen
row by its key reads a page of each of its columns, so it costs more
than in a table of the ROWS layout. COLUMNS.FREEZE_DELAY keeps rows that
are changed soon after they are written from being frozen while they
still are.

What a table holds, and what a query of it gives, is the same in either
layout; only how fast each query runs differs.

Plug-in engines declare options of their own, listed alike.
