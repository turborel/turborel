# Appendix G: Table Options

[Contents](README.md)

Each option of a table of the built-in storage engines, `rowstore` and
`columnstore`, as INFORMATION_SCHEMA.TABLE_OPTIONS lists it. The options
whose names begin COLUMNSTORE are the column store's alone; the others
are both's. A table sets one by
[CREATE TABLE](../sql/statements/create-table.md) ... WITH ( ... ), and
changes it by [ALTER TABLE](../sql/statements/alter-table.md) SET
( ... ); one set to DEFAULT takes its default again. *Changed* says how a
change is made: in place, the rows as they are, or by a rebuild, the
table made anew and its rows copied, as ALTER TABLE ... USING makes it.

| Option | Default | Values | Changed |
|---|---|---|---|
| COLUMNSTORE.BLOOM | none | AT MOST 8 OF THE TABLE'S COLUMNS, BY COMMAS | in place |
| COLUMNSTORE.COMPRESSION | AUTO | ONE OF AUTO, NONE | in place |
| COLUMNSTORE.FREEZE_DELAY | 0 | INTEGER FROM 0 TO 31622400 | in place |
| COLUMNSTORE.FREEZE_ROWS | 0 | INTEGER FROM 0 TO 9223372036854775807 | in place |
| COLUMNSTORE.GROUP_ROWS | 65536 | INTEGER FROM 2048 TO 65536 | in place |
| COLUMNSTORE.ORDER | none | AT MOST 8 OF THE TABLE'S COLUMNS, BY COMMAS | in place |
| COLUMNSTORE.REWRITE_FRACTION | 0.2 | NUMBER FROM 0 TO 1 | in place |
| ENCODINGS | none | ENCODINGS, BY COMMAS | in place |
| OUT_OF_LINE | 0 | INTEGER FROM 0 TO 1073741824 | rebuild |
| STATISTICS.BUCKETS | 100 | INTEGER FROM 1 TO 10000 | in place |
| STATISTICS.REFRESH_FRACTION | 0.1 | NUMBER FROM 0 TO 1000 | in place |
| STATISTICS.REFRESH_ROWS | 50 | INTEGER FROM 0 TO 9223372036854775807 | in place |
| STATISTICS.SAMPLE | 10000 | INTEGER FROM 1 TO 1000000 | in place |
| TRANSACTIONS.HELD_BYTES | 1048576 | INTEGER FROM 4096 TO 1099511627776 | in place |
| TRANSACTIONS.TRACKED_ROWS | 65536 | INTEGER FROM 1 TO 9223372036854775807 | in place |

Each in a sentence:

- **COLUMNSTORE.BLOOM**: The columns, by commas, of which a table of the column store keeps a Bloom filter of each 1024 rows of each group it freezes, so an equality or IN on one skips the rows that cannot hold its values, groups frozen before written again so in the background; none if empty.
- **COLUMNSTORE.COMPRESSION**: Whether a table of the column store compresses the chunks of the groups it freezes from then on: AUTO, each in the encoding that takes fewest bytes; NONE, its values as they are.
- **COLUMNSTORE.FREEZE_DELAY**: The seconds a row of a table of the column store stays as it was written after its commit before it may be frozen, so rows changed soon after are not.
- **COLUMNSTORE.FREEZE_ROWS**: The rows committed past the last group after which a table of the column store freezes them; 0 for COLUMNSTORE.GROUP_ROWS.
- **COLUMNSTORE.GROUP_ROWS**: The rows of each group a table of the column store freezes; groups frozen before keep theirs.
- **COLUMNSTORE.ORDER**: The columns, by commas, by which a table of the column store sorts the rows of each group it freezes, ascending, so a condition on the first skips more of them, groups frozen before sorted again in the background; none, in the order they were written.
- **COLUMNSTORE.REWRITE_FRACTION**: The share of a frozen group's rows changed, deleted or added to, past which a table of the column store writes the group again with them; 0 for never.
- **ENCODINGS**: The registered encodings, by commas, such as a compression or an encryption with its key (AES-256-GCM:KEY), that the table's rows pass through as they are written, in its storage and the log; rows written before a change are read through their own. See [Encryption](../operator/07-encryption.md).
- **OUT_OF_LINE**: Character values longer than this, in bytes, are kept out of their row's record; 0 for a quarter of a page.
- **STATISTICS.BUCKETS**: The buckets of each column's histogram.
- **STATISTICS.REFRESH_FRACTION**: The share of the table's rows changed, and STATISTICS.REFRESH_ROWS more, after which its statistics are gathered again.
- **STATISTICS.REFRESH_ROWS**: The changes past STATISTICS.REFRESH_FRACTION of the rows after which the statistics are gathered again.
- **STATISTICS.SAMPLE**: The most values of each column the statistics sample.
- **TRANSACTIONS.HELD_BYTES**: The bytes of a transaction's changes to the table held in memory, to undo them and check others' reads by; past them, changes are written to a spill area, and a committed one's rows kept as each column's least and greatest value.
- **TRANSACTIONS.TRACKED_ROWS**: The rows of a transaction's reads, or writes, of the table tracked one by one to check its commit; past them, every row is counted, which may refuse a commit that need not be.

## The column store

A table of the column store, `CREATE TABLE ... USING columnstore`, keeps
the rows written to it as a table of the row store does, each row's values together, so writing and
fetching a row by its key cost what they cost there. Once a group's
worth of them, COLUMNSTORE.GROUP_ROWS, is committed and no transaction may
still see them otherwise, they are frozen: written again as a group of
each column's values together, and the least and greatest of each page
of a column's values kept. The database freezes them in the background,
soon after, so no transaction waits for it; queries read the table
meanwhile as before, the rows not yet frozen from where they were
written. A query that reads a few columns of many
rows then reads only those columns' pages, and a condition on a column
skips, 1,024 rows at a time, the values it cannot match. Each page of a
column's values is compressed, unless COLUMNSTORE.COMPRESSION is NONE: a
value the same throughout kept once, numbers kept as their differences
from the least, a number repeated over rows in a row kept once for them
all, an approximate number written as a decimal of few digits kept as
those digits, or few distinct values kept once each, whichever takes
fewest bytes, so a page holds more of them. Approximate numbers so kept
are read back exactly as written; 65,536 prices of two decimal places
take 44 pages where they take 134 plain, and are summed about a tenth
more slowly while their pages are in memory. The more of a table's rows
are frozen, and the fewer of its columns a query reads, the faster it
reads them.

A frozen row may be updated or deleted as any other; it is then kept as
rows written are, its frozen values no longer read, until a fifth of its
group's rows are so (COLUMNSTORE.REWRITE_FRACTION): the group is then written
again with them, the deleted rows gone from it. Fetching one frozen
row by its key reads a page of each of its columns, so it costs more
than in a table of the row store. COLUMNSTORE.FREEZE_DELAY keeps rows that
are changed soon after they are written from being frozen while they
still are.

COLUMNSTORE.ORDER sorts the rows of each group as it is frozen, or
written again, by the columns it names, ascending, nulls as ORDER BY
puts them: each page of the first column's values then holds a narrow
range of them, so a condition on that column skips most pages. Counting
a hundredth of 200,000 rows by a column whose values are scattered ran
4.7 times as fast with the table sorted by it; a condition on a column
the rows were in the order of before ran a sixth slower, and fetching a
row by its key a twentieth slower. Once the option is set or changed,
groups frozen before are written again in the background, sorted as it
says, a few at a time, until every group is; queries read them as before
meanwhile, in either order. A column the option names is not
dropped until it names it no more; each is named as an identifier is,
a regular one in any case, a delimited one in double quotes.

COLUMNSTORE.BLOOM keeps, of each 1,024 frozen rows of each column it
names, a Bloom filter: about 10 bits for each row, from which a query
learns that a value is surely not among those rows, or may be. A
condition `column = value`, or `column IN (...)`, of a column so named
then reads only the rows that may hold one of its values, though they
lie scattered through the table, where its least and greatest values
skip none. Of 393,216 frozen rows of distinct values in no order,
finding one by a number ran in 80 microseconds where it ran in 2.1
milliseconds without, and by a text of up to 20 characters in 157
microseconds where it ran in 13.2 milliseconds; a value not among the
rows is taken as perhaps among them about once in 120 times, and those
rows are then read and judged as any are. The filters take about 1.25
bytes for each row, a sixth as many again as a column of numbers of 8
bytes plain, so name only the columns a query looks up by equality; a
column of few values, each among most 1,024 rows, gains nothing. Rows
not yet frozen are read as before.
Once the option is set or changed, groups frozen before are written
again in the background, as for COLUMNSTORE.ORDER, and it names columns
as that option does.

Of each frozen group the database keeps in memory only a summary: the
range of rows it holds, and each column's least and greatest values and
nulls, so a query passes over a group whose summary says no row of it
can match without reading more of it. The rest of what describes a
group, of each 1,024 rows of each column, is read as a query needs it,
and up to 16 MiB of it kept for the queries after, of MEMORY_BUDGET:
INFORMATION_SCHEMA.MEMORY_RESERVATIONS lists it as the headers of frozen
row groups, a MiB at a time. What was read longest ago is given up
first, and read again when it is needed.

What a table holds, and what a query of it gives, is the same in either
store; only how fast each query runs differs. ALTER TABLE ... USING
moves a table to the other, its rows copied; the setting DEFAULT_ENGINE
gives tables created without USING theirs.

Plug-in engines declare options of their own, listed alike.
