# Appendix C: Settings

[Contents](README.md)

Each setting, as INFORMATION_SCHEMA.SETTINGS lists it. A setting marked
*session* may be set by a session for itself, by [SET](../sql/statements/set-setting.md);
every setting is the database's to set, by
[SET DATABASE](../sql/statements/set-database.md), by its owner, or as
`turboreld` starts, `--set NAME=VALUE` or a
[configuration file](../operator/02-turboreld.md#configuration-files); one that takes effect only when
the database is made is set only then, and never of a database kept in
memory. *Takes effect* says when a change does: at once, for the
statements that begin after it; from the next transaction; from the
next connection the server takes; at the next start, as the server next
starts listening, or, of STORAGE.POOL_BYTES, as the database is next
opened; or only when the database is made. A setting taken at the next
start shows the value it started with in INFORMATION_SCHEMA.SETTINGS's
STARTED_VALUE, and IS_PENDING YES while the value set differs from it
([Settings](../operator/04-settings.md)).

A plug-in may declare a setting a node's: its value is never logged nor
kept with the database, which other nodes may share, but given as the
node starts (`--set`, a configuration file, or before the program opens
the database) and lasting while it runs. SET DATABASE changes it for the
running database alone. INFORMATION_SCHEMA.SETTINGS shows such a value
with VALUE_SOURCE NODE. No setting listed here is a node's.

## The database's settings

| Setting | Default | Values | Takes effect | |
|---|---|---|---|---|
| CATALOG.COMPACT_AFTER_IMAGES | 64 | INTEGER FROM 1 TO 1000000 | at once |  |
| CATALOG.NAME | TURBOREL | AN IDENTIFIER | at once |  |
| CHECKPOINT.AFTER_BYTES | 67108864 | INTEGER FROM 1 TO 9223372036854775807 | at once |  |
| CHECKPOINT.AFTER_CHANGES | 100000 | INTEGER FROM 1 TO 9223372036854775807 | at once |  |
| CHECKPOINT.OVERDUE_FACTOR | 4 | INTEGER FROM 1 TO 1000000 | at once |  |
| COLUMNSTORE.HEADER_CACHE_BYTES | 16777216 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| DIAGNOSTICS.LEVEL | INFO | ONE OF ERROR, WARNING, INFO | at once |  |
| DIAGNOSTICS.SLOW_STATEMENT_MS | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| EXECUTOR.BATCHES | ON | ONE OF ON, OFF | at once | *session* |
| EXECUTOR.DECORRELATE_AFTER_RUNS | 8 | INTEGER FROM 0 TO 1000000 | at once |  |
| FILES.MAX_OPEN | 1024 | INTEGER FROM 0 TO 4294967295 | at once |  |
| JOBS.POLICY | AUTOMATIC | ONE OF AUTOMATIC, MANUAL, PAUSED | at once |  |
| LOCKS.TIMEOUT_MS | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once | *session* |
| LOCKS.WAIT | OFF | ONE OF ON, OFF | from the next transaction | *session* |
| LOG.COMMIT_SYNC | ON | ONE OF ON, OFF | at once | *session* |
| LOG.DURABILITY | DATA | ONE OF NONE, DATA, FULL | at once |  |
| LOG.SEGMENT_BYTES | 67108864 | INTEGER FROM 4096 TO 1099511627775 | when the database is made |  |
| LOG.SYNC_AFTER_MS | 1000 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| MEMORY.BUDGET_BYTES | 268435456 | INTEGER FROM 1048576 TO 9223372036854775807 | at once |  |
| MEMORY.WORK_BYTES | 4194304 | INTEGER FROM 65536 TO 9223372036854775807 | at once | *session* |
| OPTIMIZER.CONSTANT_FOLDING | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.MAX_PASSES | 64 | INTEGER FROM 1 TO 1000000 | at once |  |
| OPTIMIZER.MERGE_FILTERS | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PREDICATE_TRANSITIVITY | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_FILTER_INTO_OUTER_JOIN | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_FILTER_INTO_PRODUCT | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_FILTER_THROUGH_DISTINCT | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_FILTER_THROUGH_PROJECT | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_HAVING_INTO_WHERE | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_RAISING_FILTER_INTO_PRODUCT | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.REMOVE_REDUNDANT_DISTINCT | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.SIMPLIFY_CONDITIONS | ON | ONE OF ON, OFF | at once | *session* |
| PLANNER.EQUAL_SELECTIVITY | 1E-2 | NUMBER FROM 0E0 TO 1E0 | at once |  |
| PLANNER.INDEX_DESCENT_COST | 1E0 | NUMBER FROM 0E0 TO 1.7976931348623157E308 | at once |  |
| PLANNER.NULL_FRACTION | 1E-2 | NUMBER FROM 0E0 TO 1E0 | at once |  |
| PLANNER.RANGE_SELECTIVITY | 3.333333333333333E-1 | NUMBER FROM 0E0 TO 1E0 | at once |  |
| PLANNER.REORDER_GAIN | 9E-1 | NUMBER FROM 0E0 TO 9.99E-1 | at once |  |
| PLANNER.SORT_COMPARE_COST | 1E-1 | NUMBER FROM 0E0 TO 1.7976931348623157E308 | at once |  |
| SESSION.COMMIT_AT_END | ON | ONE OF ON, OFF | at once | *session* |
| SESSION.TIME_ZONE | +00:00 | A TIME ZONE DISPLACEMENT FROM -12:59 TO +13:00 | at once | *session* |
| SPILL.BATCH_BYTES | 16384 | INTEGER FROM 4096 TO 1048576 | at once | *session* |
| SPILL.ENCODINGS | (empty) | ENCODINGS, BY COMMAS | at once |  |
| SPILL.PARTITIONS | 64 | INTEGER FROM 2 TO 4096 | at once | *session* |
| SPILL.PARTITION_DEPTH | 8 | INTEGER FROM 0 TO 64 | at once | *session* |
| SQL.FUNCTION_CALLS | OFF | ONE OF ON, OFF | at once | *session* |
| STATEMENT.TIMEOUT_MS | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once | *session* |
| STORAGE.CHECKSUM | CRC-32/ISCSI | A HASH FUNCTION | when the database is made |  |
| STORAGE.ENCODINGS | (empty) | ENCODINGS, BY COMMAS | when the database is made |  |
| STORAGE.PAGE_BYTES | 8192 | INTEGER FROM 4096 TO 65536 | when the database is made |  |
| STORAGE.POOL_BYTES | 33554432 | INTEGER FROM 65536 TO 9223372036854775807 | at the next start |  |
| STORAGE.READ_AHEAD_PAGES | 32 | INTEGER FROM 0 TO 4294967295 | at once |  |
| TABLES.ENGINE | rowstore | A STORAGE ENGINE | at once |  |
| TABLES.UNIQUE_INDEX_METHOD | btree | AN INDEX METHOD | at once |  |
| TRANSACTION.STRICT | ON | ONE OF ON, OFF | from the next transaction |  |

Each in a sentence:

- **CATALOG.COMPACT_AFTER_IMAGES**: The images the catalog file holds after which the next is written, alone, to the other file.
- **CATALOG.NAME**: The name of the catalog the schemata are in, as the Information Schema gives it [ISO/IEC 9075:1992 §21.2.2].
- **CHECKPOINT.AFTER_BYTES**: The bytes written to the data file since the last checkpoint after which the end of a transaction takes one.
- **CHECKPOINT.AFTER_CHANGES**: The row changes logged after which the end of a transaction takes a checkpoint.
- **CHECKPOINT.OVERDUE_FACTOR**: How many times CHECKPOINT.AFTER_CHANGES or CHECKPOINT.AFTER_BYTES the log, or the data file's side file, grows to with no checkpoint giving it up before the diagnostics are told that checkpoints are overdue.
- **COLUMNSTORE.HEADER_CACHE_BYTES**: The bytes of the headers of frozen row groups the column store holds in memory at most, of its tables together, reserved of MEMORY.BUDGET_BYTES as they are held; 0 for none, each read as it is needed.
- **DIAGNOSTICS.LEVEL**: The least level of the events told to the diagnostics: ERROR, those alone; WARNING, warnings too; INFO, every event.
- **DIAGNOSTICS.SLOW_STATEMENT_MS**: The milliseconds of work after which a statement is told to the diagnostics as slow; 0 for none.
- **EXECUTOR.BATCHES**: Whether a query reads its tables a batch of rows at a time where it can: ON; or a row at a time throughout, OFF, as to compare the two.
- **EXECUTOR.DECORRELATE_AFTER_RUNS**: The runs of a correlated subquery, for as many sets of values, after which it is run once for all of them.
- **FILES.MAX_OPEN**: The most files of the database open at once, 0 for no limit: past it, the one used least recently is closed (one with writes not yet synced last, and synced first), and opened again as it is next used.
- **JOBS.POLICY**: How the database's background work, its checkpoints, maintenance and syncs of the log (LOG.SYNC_AFTER_MS), runs: on threads of its own as it comes due, AUTOMATIC; only when the program asks, MANUAL; or not until this changes, PAUSED.
- **LOCKS.TIMEOUT_MS**: The milliseconds a statement waits for a lock, under LOCKS.WAIT ON, before it is refused (SQLCODE -911); 0 for no limit.
- **LOCKS.WAIT**: ON waits for a lock another transaction holds; OFF refuses the statement at once.
- **LOG.COMMIT_SYNC**: Whether a commit waits until its commit record is synced to the log, ON; or returns once the record is written, OFF: the transaction is then seen by others at once, and lost if the system fails before the log is next synced, by a commit of ON, a definition, a checkpoint, the database closing, or LOG.SYNC_AFTER_MS passing.
- **LOG.DURABILITY**: How durable a sync of the log makes what it holds: NONE, never synced, so a failure of the system may lose any commit, and TRANSACTION.STRICT takes the log as not durable; DATA, its data on the device with what reading it back needs; FULL, with all of its files' metadata too. A change holds from the log's next sync, which after NONE makes every record before it durable too, and each segment of the log begun after is made as it says; a log plug-in takes it as far as it can.
- **LOG.SEGMENT_BYTES**: The bytes after which a segment of the log is ended and the next begun.
- **LOG.SYNC_AFTER_MS**: The milliseconds after a commit of LOG.COMMIT_SYNC OFF within which the log is synced, by a job of its own if nothing else syncs it first; 0 for none, the commit then synced only by the log's next sync for another reason.
- **MEMORY.BUDGET_BYTES**: The bytes the database's buffer pools, work areas and caches may reserve in all, with the files of a database kept in memory, its pages, temporary tables and spills; lowering it takes back none they hold.
- **MEMORY.WORK_BYTES**: The bytes a sort, hash join, grouping, DISTINCT, EXCEPT or INTERSECT holds in memory before it spills to a work area of as many, reserved of MEMORY.BUDGET_BYTES; so too a cursor's rows read ahead of its fetches or kept to scroll among; the rows an INSERT, UPDATE or DELETE chooses before changing any, what it did, kept for its constraints' checks (a deferred one's until made), and what its referential actions keep; and a session's temporary tables' pages, the rest in a spill file.
- **OPTIMIZER.CONSTANT_FOLDING**: Whether the optimizer's rule constant_folding rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.MAX_PASSES**: The most passes the optimizer makes over a plan.
- **OPTIMIZER.MERGE_FILTERS**: Whether the optimizer's rule merge_filters rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PREDICATE_TRANSITIVITY**: Whether the optimizer's rule predicate_transitivity rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_FILTER_INTO_OUTER_JOIN**: Whether the optimizer's rule push_filter_into_outer_join rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_FILTER_INTO_PRODUCT**: Whether the optimizer's rule push_filter_into_product rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_FILTER_THROUGH_DISTINCT**: Whether the optimizer's rule push_filter_through_distinct rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_FILTER_THROUGH_PROJECT**: Whether the optimizer's rule push_filter_through_project rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_HAVING_INTO_WHERE**: Whether the optimizer's rule push_having_into_where rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_RAISING_FILTER_INTO_PRODUCT**: Whether the optimizer's rule push_raising_filter_into_product rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.REMOVE_REDUNDANT_DISTINCT**: Whether the optimizer's rule remove_redundant_distinct rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.SIMPLIFY_CONDITIONS**: Whether the optimizer's rule simplify_conditions rewrites plans: ON; or OFF, as to compare plans made without it.
- **PLANNER.EQUAL_SELECTIVITY**: The fraction of rows an equality keeps when the statistics do not tell.
- **PLANNER.INDEX_DESCENT_COST**: The cost of one level of an index descent, a row read in sequence costing 1.
- **PLANNER.NULL_FRACTION**: The fraction of a column's values taken to be null when the statistics do not tell.
- **PLANNER.RANGE_SELECTIVITY**: The fraction of rows an order comparison, a pattern or a subquery keeps when the statistics do not tell.
- **PLANNER.REORDER_GAIN**: A join order is taken over the FROM clause's if its estimated cost is less than this fraction of that order's.
- **PLANNER.SORT_COMPARE_COST**: The cost of one comparison of a sort, a row read in sequence costing 1.
- **SESSION.COMMIT_AT_END**: Whether a transaction open when its session ends is committed, ON, or rolled back, OFF [X3.135-1986 §7.1 GR2].
- **SESSION.TIME_ZONE**: The time zone displacement of an SQL-session, ±HH:MM from -12:59 to +13:00 [ISO/IEC 9075:1992 §4.5.1]: of CURRENT_TIME and CURRENT_TIMESTAMP, and implied by a time or timestamp without time zone.
- **SPILL.BATCH_BYTES**: The bytes of rows a spilling operator gathers before it writes them to its work area; it reads or writes a run for every two batches its MEMORY.WORK_BYTES holds.
- **SPILL.ENCODINGS**: The encodings, as compression, the pages of work areas' files pass through as each is written from now on, by commas, in order, before the encryption they pass through while the database encrypts anything; none if empty.
- **SPILL.PARTITIONS**: The most partitions a spilled operator (a join, grouping, DISTINCT, EXCEPT or INTERSECT) writes at once; it writes fewer if its MEMORY.WORK_BYTES holds fewer runs.
- **SPILL.PARTITION_DEPTH**: The most times a spilled operator partitions a partition again, before it holds the partition in a table, or, grouping and an outer join, makes it in passes.
- **SQL.FUNCTION_CALLS**: Whether a statement may call a function by the syntax NAME(arguments), which the standards do not have: ON; or refuses it, as SQL does, OFF.
- **STATEMENT.TIMEOUT_MS**: The milliseconds a statement may run, each call and each fetch of a cursor alone, before it is cancelled (SQLCODE -920); 0 for no limit.
- **STORAGE.CHECKSUM**: The hash function every file is checksummed with.
- **STORAGE.ENCODINGS**: The encodings, as compression or encryption, every file passes through, by commas, in order; none if empty.
- **STORAGE.PAGE_BYTES**: The bytes of a page of the data file, a power of two.
- **STORAGE.POOL_BYTES**: The bytes of the buffer pool of the data file, reserved of MEMORY.BUDGET_BYTES as the database is opened; of a database kept in memory, the pool its engine and index methods share, made with its first table.
- **STORAGE.READ_AHEAD_PAGES**: The pages a scan of a table of the built-in row store, or of a B-tree index, reads in one read of the file after a page not in the buffer pool, at most an eighth of the pool's pages; 0 for none. A change holds from the next page so read, a scan under way included.
- **TABLES.ENGINE**: The storage engine of tables a schema creates.
- **TABLES.UNIQUE_INDEX_METHOD**: The index method behind the UNIQUE constraints of tables created from now on.
- **TRANSACTION.STRICT**: ON refuses an engine that falls short of serializable, recoverable transactions [X3.135-1986 §4.16]; OFF allows it.

## The server's settings

| Setting | Default | Values | Takes effect | |
|---|---|---|---|---|
| SERVER.BATCH_BYTES | 65536 | INTEGER FROM 1 TO 4294967295 | at once |  |
| SERVER.HOST | 127.0.0.1 | TEXT OF AT MOST 1024 CHARACTERS | at the next start |  |
| SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS | 0 | INTEGER FROM 0 TO 4294967295 | at once |  |
| SERVER.IDLE_TIMEOUT_MS | 0 | INTEGER FROM 0 TO 4294967295 | at once |  |
| SERVER.KEEPALIVE_IDLE_MS | 0 | INTEGER FROM 0 TO 32767000 | from the next connection |  |
| SERVER.KEEPALIVE_INTERVAL_MS | 0 | INTEGER FROM 0 TO 32767000 | from the next connection |  |
| SERVER.KEEPALIVE_PROBES | 0 | INTEGER FROM 0 TO 127 | from the next connection |  |
| SERVER.LOGIN_DELAY_MS | 10000 | INTEGER FROM 0 TO 4294967295 | at once |  |
| SERVER.LOGIN_TIMEOUT_MS | 60000 | INTEGER FROM 0 TO 4294967295 | at once |  |
| SERVER.MAX_CONNECTIONS | 256 | INTEGER FROM 1 TO 4294967295 | at once |  |
| SERVER.MAX_LOGINS | 4 | INTEGER FROM 1 TO 64 | at once |  |
| SERVER.MAX_MESSAGE_BYTES | 16777216 | INTEGER FROM 1024 TO 4294967295 | from the next connection |  |
| SERVER.MAX_REFUSALS | 16 | INTEGER FROM 0 TO 4294967295 | at once |  |
| SERVER.MAX_STATEMENTS | 1024 | INTEGER FROM 1 TO 4294967295 | at once |  |
| SERVER.PASSWORD_ITERATIONS | 600000 | INTEGER FROM 10000 TO 4294967295 | at the next start |  |
| SERVER.PORT | 7422 | INTEGER FROM 0 TO 65535 | at the next start |  |
| SERVER.SCRAM_WITHOUT_TLS | OFF | ONE OF ON, OFF | from the next connection |  |
| SERVER.SHUTDOWN_TIMEOUT_MS | 30000 | INTEGER FROM 0 TO 4294967295 | at once |  |
| SERVER.SOCKET | (empty) | TEXT OF AT MOST 1024 CHARACTERS | at the next start |  |
| SERVER.TLS | OFF | ONE OF OFF, ON, REQUIRED | at the next start |  |
| SERVER.TLS_CERTIFICATE | (empty) | TEXT OF AT MOST 1024 CHARACTERS | from the next connection |  |
| SERVER.TLS_CLIENT_CA | (empty) | TEXT OF AT MOST 1024 CHARACTERS | from the next connection |  |
| SERVER.TLS_KEY | (empty) | TEXT OF AT MOST 1024 CHARACTERS | from the next connection |  |
| SERVER.TRUST_LOCAL | OFF | ONE OF ON, OFF | at the next start |  |
| SERVER.WORKERS | the processors | INTEGER FROM 1 TO 1024 | at once |  |

Each in a sentence:

- **SERVER.BATCH_BYTES**: About how many bytes of rows the server sends in one batch.
- **SERVER.HOST**: Hosts or addresses, by commas, where TCP connections are taken.
- **SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS**: The milliseconds the server waits for the next request of a client in a transaction before it closes the connection, the transaction rolled back and its locks let go; 0 for no limit.
- **SERVER.IDLE_TIMEOUT_MS**: The milliseconds the server waits on a client, for its next request or to take what it is sent, before it closes the connection, its transaction rolled back; 0 for no limit.
- **SERVER.KEEPALIVE_IDLE_MS**: The milliseconds a TCP connection is idle before the system probes whether its client is still there, rounded up to whole seconds; 0 for the system's own.
- **SERVER.KEEPALIVE_INTERVAL_MS**: The milliseconds between the system's probes of a TCP connection's client, unanswered, rounded up to whole seconds; 0 for the system's own.
- **SERVER.KEEPALIVE_PROBES**: The probes unanswered after which the system closes a TCP connection, its transaction rolled back; 0 for the system's own.
- **SERVER.LOGIN_DELAY_MS**: The longest, in milliseconds, a login waits after failed ones from its client's address; 0 for no wait.
- **SERVER.LOGIN_TIMEOUT_MS**: The milliseconds a connection has, from when it is taken, to begin TLS and authenticate, the two together, before it is closed; 0 for no limit.
- **SERVER.MAX_CONNECTIONS**: Connections the server takes at once, at most.
- **SERVER.MAX_LOGINS**: Authentications the server checks at once, at most, apart from its statements' workers.
- **SERVER.MAX_MESSAGE_BYTES**: The longest message, in bytes, the server takes from a client, read as the client's connection is taken.
- **SERVER.MAX_REFUSALS**: Connections past SERVER.MAX_CONNECTIONS the server refuses at once, each told why (SQLSTATE 08004) once its TLS, if any, is begun; one past these is closed at once, told nothing.
- **SERVER.MAX_STATEMENTS**: Prepared statements a connection holds at once, at most.
- **SERVER.PASSWORD_ITERATIONS**: The PBKDF2 iterations of a password's verifier made from the start on.
- **SERVER.PORT**: The TCP port connections are taken at.
- **SERVER.SCRAM_WITHOUT_TLS**: Whether SCRAM-SHA-256 is taken over plain TCP (ON), where an eavesdropper may guess the password from the exchange, or only over TLS, a Unix socket or a named pipe (OFF).
- **SERVER.SHUTDOWN_TIMEOUT_MS**: The milliseconds a server told to stop lets each connection finish its statement, before it cancels the statements and closes the connections, their transactions rolled back; 0 for no limit.
- **SERVER.SOCKET**: The Unix socket or named pipe taken at, unless empty.
- **SERVER.TLS**: Whether TCP connections may use TLS (ON), or must (REQUIRED).
- **SERVER.TLS_CERTIFICATE**: The PEM file of the server's certificate chain, taken again for the connections after a change.
- **SERVER.TLS_CLIENT_CA**: The PEM file of the authorities whose certificates a TLS client may present to log in by; none asked for if empty.
- **SERVER.TLS_KEY**: The PEM file of the server's private key, taken again for the connections after a change.
- **SERVER.TRUST_LOCAL**: Whether a client of this machine is trusted as who it says (ON), or proves it as every other does (OFF).
- **SERVER.WORKERS**: Statements the server runs at once, at most.

Plug-ins may declare settings of their own, listed alike: a plug-in of
any kind its own, under its name, as the column store declares
COLUMNSTORE.HEADER_CACHE_BYTES; a rule added to the optimizer, OPTIMIZER
and its name, as the built-in rules have.
