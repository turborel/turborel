# Appendix C: Settings

[Contents](README.md)

Each setting, as INFORMATION_SCHEMA.SETTINGS lists it. A setting marked
*session* may be set by a session for itself, by [SET](../sql/statements/set-setting.md);
every setting is the database's to set, by
[SET DATABASE](../sql/statements/set-database.md), by its owner, or as
`turboreld` starts, `--set NAME=VALUE`; one that takes effect only when
the database is made is set only then. *Takes effect* says when a
change does: at once, from the next transaction, when the server
starts, or only when the database is made.

## The database's settings

| Setting | Default | Values | Takes effect | |
|---|---|---|---|---|
| CATALOG.COMPACT_AFTER | 64 | INTEGER FROM 1 TO 1000000 | at once |  |
| CATALOG_NAME | TURBOREL | AN IDENTIFIER | at once |  |
| CHECKPOINT.AFTER_BYTES | 67108864 | INTEGER FROM 1 TO 9223372036854775807 | at once |  |
| CHECKPOINT.AFTER_CHANGES | 100000 | INTEGER FROM 1 TO 9223372036854775807 | at once |  |
| DEFAULT_ENGINE | rowstore | A STORAGE ENGINE | at once |  |
| DIAGNOSTICS.SLOW_STATEMENT | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| EXECUTOR.BATCHES | ON | ONE OF ON, OFF | at once | *session* |
| EXECUTOR.DECORRELATE_AFTER | 8 | INTEGER FROM 0 TO 1000000 | at once |  |
| LOCK_TIMEOUT | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once | *session* |
| LOCK_WAIT | NO_WAIT | ONE OF NO_WAIT, WAIT | from the next transaction | *session* |
| LOG.SEGMENT_SIZE | 67108864 | INTEGER FROM 4096 TO 1099511627775 | when the database is made |  |
| MEMORY_BUDGET | 268435456 | INTEGER FROM 1048576 TO 9223372036854775807 | at once |  |
| OPTIMIZER.MAX_PASSES | 64 | INTEGER FROM 1 TO 1000000 | at once |  |
| PLANNER.EQUAL_SELECTIVITY | 1E-2 | NUMBER FROM 0E0 TO 1E0 | at once |  |
| PLANNER.INDEX_DESCENT_COST | 1E0 | NUMBER FROM 0E0 TO 1.7976931348623157E308 | at once |  |
| PLANNER.NULL_FRACTION | 1E-2 | NUMBER FROM 0E0 TO 1E0 | at once |  |
| PLANNER.RANGE_SELECTIVITY | 3.333333333333333E-1 | NUMBER FROM 0E0 TO 1E0 | at once |  |
| PLANNER.REORDER_GAIN | 9E-1 | NUMBER FROM 0E0 TO 9.99E-1 | at once |  |
| PLANNER.SORT_COMPARE_COST | 1E-1 | NUMBER FROM 0E0 TO 1.7976931348623157E308 | at once |  |
| SESSION_END | COMMIT | ONE OF COMMIT, ROLLBACK | at once | *session* |
| SPILL.PARTITIONS | 64 | INTEGER FROM 2 TO 4096 | at once | *session* |
| SPILL.PARTITION_DEPTH | 8 | INTEGER FROM 0 TO 64 | at once | *session* |
| SPILL_BATCH | 16384 | INTEGER FROM 4096 TO 1048576 | at once | *session* |
| STATEMENT_TIMEOUT | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once | *session* |
| STORAGE.CHECKSUM | CRC-32/ISCSI | A HASH FUNCTION | when the database is made |  |
| STORAGE.ENCODINGS | (empty) | ENCODINGS, BY COMMAS | when the database is made |  |
| STORAGE.PAGE_SIZE | 8192 | INTEGER FROM 4096 TO 65536 | when the database is made |  |
| STORAGE.POOL_BYTES | 33554432 | INTEGER FROM 65536 TO 9223372036854775807 | when the server starts |  |
| TIME_ZONE | +00:00 | A TIME ZONE DISPLACEMENT FROM -12:59 TO +13:00 | at once | *session* |
| TRANSACTION_POLICY | STRICT | ONE OF STRICT, PERMISSIVE | from the next transaction |  |
| UNIQUE_INDEX_METHOD | btree | AN INDEX METHOD | at once |  |
| WORK_MEMORY | 4194304 | INTEGER FROM 65536 TO 9223372036854775807 | at once | *session* |

Each in a sentence:

- **CATALOG.COMPACT_AFTER**: The images the catalog file holds after which the next is written, alone, to the other file.
- **CATALOG_NAME**: The name of the catalog the schemata are in, as the Information Schema gives it [ISO/IEC 9075:1992 §21.2.2].
- **CHECKPOINT.AFTER_BYTES**: The bytes written to the data file since the last checkpoint after which the end of a transaction takes one.
- **CHECKPOINT.AFTER_CHANGES**: The row changes logged after which the end of a transaction takes a checkpoint.
- **DEFAULT_ENGINE**: The storage engine of tables a schema creates.
- **DIAGNOSTICS.SLOW_STATEMENT**: The milliseconds of work after which a statement is told to the diagnostics as slow; 0 for none.
- **EXECUTOR.BATCHES**: Whether a query reads its tables a batch of rows at a time where it can: ON; or a row at a time throughout, OFF, as to compare the two.
- **EXECUTOR.DECORRELATE_AFTER**: The runs of a correlated subquery, for as many sets of values, after which it is run once for all of them.
- **LOCK_TIMEOUT**: The milliseconds a statement waits for a lock, under LOCK_WAIT WAIT, before it is refused (SQLCODE -911); 0 for no limit.
- **LOCK_WAIT**: WAIT waits for a lock another transaction holds; NO_WAIT refuses the statement at once.
- **LOG.SEGMENT_SIZE**: The bytes after which a segment of the log is ended and the next begun.
- **MEMORY_BUDGET**: The bytes the database's buffer pools may reserve in all; lowering it takes back none they hold.
- **OPTIMIZER.MAX_PASSES**: The most passes the optimizer makes over a plan.
- **PLANNER.EQUAL_SELECTIVITY**: The fraction of rows an equality keeps when the statistics do not tell.
- **PLANNER.INDEX_DESCENT_COST**: The cost of one level of an index descent, a row read in sequence costing 1.
- **PLANNER.NULL_FRACTION**: The fraction of a column's values taken to be null when the statistics do not tell.
- **PLANNER.RANGE_SELECTIVITY**: The fraction of rows an order comparison, a pattern or a subquery keeps when the statistics do not tell.
- **PLANNER.REORDER_GAIN**: A join order is taken over the FROM clause's if its estimated cost is less than this fraction of that order's.
- **PLANNER.SORT_COMPARE_COST**: The cost of one comparison of a sort, a row read in sequence costing 1.
- **SESSION_END**: What ends a transaction open when its session ends [X3.135-1986 §7.1 GR2]: COMMIT or ROLLBACK.
- **SPILL.PARTITIONS**: The most partitions a spilled operator (a join, grouping, DISTINCT, EXCEPT or INTERSECT) writes at once; it writes fewer if its WORK_MEMORY holds fewer runs.
- **SPILL.PARTITION_DEPTH**: The most times a spilled operator partitions a partition again, before it holds the partition in a table, or, grouping, makes it in passes.
- **SPILL_BATCH**: The bytes of rows a spilling operator gathers before it writes them to its work area; it reads or writes a run for every two batches its WORK_MEMORY holds.
- **STATEMENT_TIMEOUT**: The milliseconds a statement may run, each call and each fetch of a cursor alone, before it is cancelled (SQLCODE -920); 0 for no limit.
- **STORAGE.CHECKSUM**: The hash function every file is checksummed with.
- **STORAGE.ENCODINGS**: The encodings, as compression or encryption, every file passes through, by commas, in order; none if empty.
- **STORAGE.PAGE_SIZE**: The bytes of a page of the data file, a power of two.
- **STORAGE.POOL_BYTES**: The bytes of the buffer pool of the data file, reserved of MEMORY_BUDGET as the database is opened; of a database kept in memory, the pool its engine and index methods share, made with its first table.
- **TIME_ZONE**: The time zone displacement of an SQL-session, ±HH:MM from -12:59 to +13:00 [ISO/IEC 9075:1992 §4.5.1]: of CURRENT_TIME and CURRENT_TIMESTAMP, and implied by a time or timestamp without time zone.
- **TRANSACTION_POLICY**: STRICT refuses an engine that falls short of serializable, recoverable transactions [X3.135-1986 §4.16]; PERMISSIVE allows it.
- **UNIQUE_INDEX_METHOD**: The index method behind the UNIQUE constraints of tables created from now on.
- **WORK_MEMORY**: The bytes a sort, hash join, grouping, DISTINCT, EXCEPT or INTERSECT holds in memory before it spills to a work area of as many, reserved of MEMORY_BUDGET.

## The server's settings

| Setting | Default | Values | Takes effect | |
|---|---|---|---|---|
| SERVER.BATCH_BYTES | 65536 | INTEGER FROM 1 TO 4294967295 | at once |  |
| SERVER.HOST | 127.0.0.1 | TEXT OF AT MOST 1024 CHARACTERS | when the server starts |  |
| SERVER.IDLE_TIMEOUT | 0 | INTEGER FROM 0 TO 4294967295 | at once |  |
| SERVER.LOGIN_TIMEOUT | 60000 | INTEGER FROM 0 TO 4294967295 | at once |  |
| SERVER.MAX_CONNECTIONS | 256 | INTEGER FROM 1 TO 4294967295 | at once |  |
| SERVER.MAX_MESSAGE_BYTES | 16777216 | INTEGER FROM 1024 TO 4294967295 | at once |  |
| SERVER.MAX_STATEMENTS | 1024 | INTEGER FROM 1 TO 4294967295 | at once |  |
| SERVER.PASSWORD_ITERATIONS | 600000 | INTEGER FROM 10000 TO 4294967295 | when the server starts |  |
| SERVER.PORT | 7422 | INTEGER FROM 0 TO 65535 | when the server starts |  |
| SERVER.SOCKET | (empty) | TEXT OF AT MOST 1024 CHARACTERS | when the server starts |  |
| SERVER.TLS | OFF | ONE OF OFF, ON, REQUIRED | when the server starts |  |
| SERVER.TLS_CERTIFICATE | (empty) | TEXT OF AT MOST 1024 CHARACTERS | when the server starts |  |
| SERVER.TLS_KEY | (empty) | TEXT OF AT MOST 1024 CHARACTERS | when the server starts |  |
| SERVER.TRUST | OFF | ONE OF LOCAL, OFF | when the server starts |  |
| SERVER.WORKERS | the processors | INTEGER FROM 1 TO 1024 | at once |  |

Each in a sentence:

- **SERVER.BATCH_BYTES**: About how many bytes of rows the server sends in one batch.
- **SERVER.HOST**: Hosts or addresses, by commas, where TCP connections are taken.
- **SERVER.IDLE_TIMEOUT**: The milliseconds the server waits on a client, for its next request or to take what it is sent, before it closes the connection, its transaction rolled back; 0 for no limit.
- **SERVER.LOGIN_TIMEOUT**: The milliseconds a connection has to begin TLS and authenticate before it is closed; 0 for no limit.
- **SERVER.MAX_CONNECTIONS**: Connections the server takes at once, at most.
- **SERVER.MAX_MESSAGE_BYTES**: The longest message, in bytes, the server takes from a client.
- **SERVER.MAX_STATEMENTS**: Prepared statements a connection holds at once, at most.
- **SERVER.PASSWORD_ITERATIONS**: The PBKDF2 iterations of a password's verifier made from the start on.
- **SERVER.PORT**: The TCP port connections are taken at.
- **SERVER.SOCKET**: The Unix socket connections are taken at, unless empty.
- **SERVER.TLS**: Whether TCP connections may use TLS (ON), or must (REQUIRED).
- **SERVER.TLS_CERTIFICATE**: The PEM file of the server's certificate chain.
- **SERVER.TLS_KEY**: The PEM file of the server's private key.
- **SERVER.TRUST**: Whether a client of this machine is trusted as who it says (LOCAL), or proves it as every other does (OFF).
- **SERVER.WORKERS**: Statements the server runs at once, at most.

Plug-ins may declare settings of their own, listed alike.
