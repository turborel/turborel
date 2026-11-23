# Settings *(extension)*

[Contents](README.md)

## Scopes

A setting has the database's value, kept with the database, and may
have a session's own: a session's value of a setting a session may set
stays above the database's for that session alone. The server's settings,
SERVER.*, are the database's too, kept with it. [Settings](../appendix/c-settings.md)
lists every setting, its default, what values it takes, and when a
change takes effect.

## SET DATABASE

The database's value is set as the server starts, by `--set NAME=VALUE`
or a [configuration file](02-turboreld.md#configuration-files), or on a
running server by its owner, in SQL:

```sql
SET DATABASE LOCKS.WAIT = 'OFF';
SET DATABASE SERVER.MAX_CONNECTIONS TO 512;
SET DATABASE LOCKS.WAIT = DEFAULT;
```

```
OK
OK
OK
```

DEFAULT gives a setting its default again. A session sets its own value
with [SET](../sql/statements/set-setting.md); only the owner sets the
database's ([SET DATABASE](../sql/statements/set-database.md)).

## When a change takes effect

INFORMATION_SCHEMA.SETTINGS tells when a change of each setting takes
effect, in its column TAKES_EFFECT, by one of five values; Appendix C
says the same in words:

| TAKES_EFFECT | Appendix C | When | Of |
|---|---|---|---|
| STATEMENT | at once | for each statement that begins after the change | most |
| TRANSACTION | from the next transaction | for each transaction that begins after the change | LOCKS.WAIT, TRANSACTION.ISOLATION, TRANSACTION.STRICT |
| CONNECTION | from the next connection | for each connection the server takes after the change | SERVER.MAX_MESSAGE_BYTES, SERVER.TLS_CERTIFICATE, SERVER.TLS_KEY, SERVER.TLS_CLIENT_CA, SERVER.SCRAM_WITHOUT_TLS, SERVER.KEEPALIVE_* |
| START | at the next start | as the server next starts, or the database is next opened | SERVER.HOST, SERVER.PORT, SERVER.SOCKET, SERVER.TLS, SERVER.TRUST_LOCAL, SERVER.PASSWORD_ITERATIONS, STORAGE.POOL_BYTES, REPLICATION.* ([Replication](09-replication.md)) |
| CREATION | only when the database is made | as the database is made in its directory; fixed after | STORAGE.PAGE_BYTES, STORAGE.CHECKSUM, LOG.SEGMENT_BYTES |

```sql
SELECT SETTING_NAME, TAKES_EFFECT FROM INFORMATION_SCHEMA.SETTINGS
    WHERE TAKES_EFFECT <> 'STATEMENT' ORDER BY TAKES_EFFECT, SETTING_NAME;
```

```
 SETTING_NAME                                                 | TAKES_EFFECT
--------------------------------------------------------------+--------------
 SERVER.COMPRESSION_MIN_BYTES                                 | CONNECTION
 SERVER.KEEPALIVE_IDLE_MS                                     | CONNECTION
 SERVER.KEEPALIVE_INTERVAL_MS                                 | CONNECTION
 SERVER.KEEPALIVE_PROBES                                      | CONNECTION
 SERVER.MAX_MESSAGE_BYTES                                     | CONNECTION
 SERVER.SCRAM_WITHOUT_TLS                                     | CONNECTION
 SERVER.TLS_CERTIFICATE                                       | CONNECTION
 SERVER.TLS_CLIENT_CA                                         | CONNECTION
 SERVER.TLS_KEY                                               | CONNECTION
 LOG.SEGMENT_BYTES                                            | CREATION
 STORAGE.CHECKSUM                                             | CREATION
 STORAGE.PAGE_BYTES                                           | CREATION
 PLUGINS.HOST                                                 | START
 REPLICATION.CA                                               | START
 REPLICATION.LISTEN                                           | START
 REPLICATION.MEMBERS                                          | START
 REPLICATION.REPLICA                                          | START
 REPLICATION.RETAIN_BYTES                                     | START
 REPLICATION.TICK_MS                                          | START
 REPLICATION.TLS                                              | START
 SERVER.HOST                                                  | START
 SERVER.PASSWORD_ITERATIONS                                   | START
 SERVER.PORT                                                  | START
 SERVER.SOCKET                                                | START
 SERVER.TLS                                                   | START
 SERVER.TRUST_LOCAL                                           | START
 STORAGE.POOL_BYTES                                           | START
 LOCKS.WAIT                                                   | TRANSACTION
 TRANSACTION.ISOLATION                                        | TRANSACTION
 TRANSACTION.STRICT                                           | TRANSACTION
(30 rows)
```

A setting fixed when the database is made is refused after:

```
turborel: SQLCODE -103 (SQLSTATE 2A000): STORAGE.PAGE_BYTES is fixed when the database is created
```

A database kept in memory, as `turboreld --memory` serves, is created
in no directory, so it takes none of them: `--set` of one is refused as
the server starts, and SET DATABASE of one once the database holds a
schema.

A change of a setting taken at the next start waits for it: STARTED_VALUE
shows the value the server, or the database as it opened, started with,
and IS_PENDING is YES while the value set differs from it. Of a setting
not taken so, or not started yet, STARTED_VALUE is null and IS_PENDING
NO.

```sql
COMMIT;
SET DATABASE SERVER.TLS = 'REQUIRED';
SELECT SETTING_NAME, SETTING_VALUE, STARTED_VALUE, IS_PENDING FROM INFORMATION_SCHEMA.SETTINGS
    WHERE SETTING_NAME = 'SERVER.TLS';
COMMIT;
SET DATABASE SERVER.TLS = DEFAULT;
```

```
OK
OK
 SETTING_NAME                                                 | SETTING_VALUE                                                | STARTED_VALUE                                                | IS_PENDING
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+------------
 SERVER.TLS                                                   | REQUIRED                                                     | OFF                                                          | YES
(1 row)
OK
OK
```

## Reading settings

INFORMATION_SCHEMA.SETTINGS shows each setting's value for the session
reading it, and where it comes from:

```sql
SELECT SETTING_NAME, SETTING_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.SETTINGS
    WHERE SETTING_NAME LIKE 'SERVER.MAX%' ORDER BY SETTING_NAME;
```

```
 SETTING_NAME                                                 | SETTING_VALUE                                                | VALUE_SOURCE
--------------------------------------------------------------+--------------------------------------------------------------+--------------
 SERVER.MAX_CONNECTIONS                                       | 512                                                          | DATABASE
 SERVER.MAX_LOGINS                                            | 4                                                            | DEFAULT
 SERVER.MAX_MESSAGE_BYTES                                     | 16777216                                                     | DEFAULT
 SERVER.MAX_REFUSALS                                          | 16                                                           | DEFAULT
 SERVER.MAX_STATEMENTS                                        | 1024                                                         | DEFAULT
(5 rows)
```

A change of the database's value is told to the diagnostics,
`setting_changed`.

## A session's own

A session may turn some things on or off for itself, over the
database's value, with [SET](../sql/statements/set-setting.md):
whether its commits wait for the log, LOG.COMMIT_SYNC; whether it takes
calls of registered functions, SQL.FUNCTION_CALLS; whether the XQuery
expressions of the statements it prepares are statically typed,
XQUERY.STATIC_TYPING ([Static typing](../sql/19-xml.md#static-typing));
each of the optimizer's rules, OPTIMIZER and the rule's name, as to
see a plan made without it; and how the planner orders joins,
PLANNER.JOIN_ENUMERATION, as to compare the enumerated plan with the
greedy one ([EXPLAIN](../sql/statements/explain.md)). A rule a program adds to the optimizer has
its setting too. A definition, such as a view, is typed as it is made by
the XQUERY.STATIC_TYPING of the session making it, and read as it was
made by every session.

```sql
SET LOG.COMMIT_SYNC = 'OFF';
SET OPTIMIZER.PREDICATE_TRANSITIVITY = 'OFF';
SET PLANNER.JOIN_ENUMERATION = 'GREEDY';
SELECT SETTING_NAME, SETTING_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.SETTINGS
    WHERE SETTING_NAME IN ('LOG.COMMIT_SYNC', 'OPTIMIZER.PREDICATE_TRANSITIVITY', 'PLANNER.JOIN_ENUMERATION', 'SQL.FUNCTION_CALLS')
    ORDER BY SETTING_NAME;
SET LOG.COMMIT_SYNC TO DEFAULT;
SET OPTIMIZER.PREDICATE_TRANSITIVITY TO DEFAULT;
SET PLANNER.JOIN_ENUMERATION TO DEFAULT;
```

```
OK
OK
OK
 SETTING_NAME                                                 | SETTING_VALUE                                                | VALUE_SOURCE
--------------------------------------------------------------+--------------------------------------------------------------+--------------
 LOG.COMMIT_SYNC                                              | OFF                                                          | SESSION
 OPTIMIZER.PREDICATE_TRANSITIVITY                             | OFF                                                          | SESSION
 PLANNER.JOIN_ENUMERATION                                     | GREEDY                                                       | SESSION
 SQL.FUNCTION_CALLS                                           | ON                                                           | DEFAULT
(4 rows)
OK
OK
OK
```

A commit of LOG.COMMIT_SYNC OFF returns once its commit record is in
the log's buffer, before it is written and before the log is synced:
its changes are seen by others at once, and a crash before the log is
next synced, by a commit that waits, a definition, a checkpoint or the
database closing, loses it, as it loses every commit after it. The
buffer is written to the log's file when it fills, at each sync, and
within a second of a commit while others come, so a failure of the
database's process alone, the system going on, loses at most the
commits of that second. The database stays whole either way.
So that such a commit is not left unsynced while the database is at
rest, a job of the database's own, `turborel-log-sync`, syncs the log
within LOG.SYNC_AFTER_MS of it, a second by default, if nothing else has;
0 leaves it to those other syncs alone, the job then only writing the
log's buffer, a second after the commit. The job runs as JOBS.POLICY says
([Background work](03-databases.md#background-work)); a sync of it that
fails stops the database taking changes, as any failed sync of the log
does.

## Parallel queries

A query reading many rows runs on several threads at once: its tables
are read in morsels, parts of about EXECUTOR.MORSEL_ROWS rows, each
taken by whichever thread is free, and the operators over them, a
grouping, DISTINCT, a sort, a hash join, window functions, EXCEPT and
INTERSECT, are made of each morsel's part. The threads are the
database's workers, `turborel-exec-1` and on, EXECUTOR.WORKERS of
them, 0 for as many as the machine runs at once, shared by every
statement; the statement's own thread works beside them. A statement
takes at most EXECUTOR.DEGREE threads, its own among them, 0 for as
many as the workers, 1 for its own thread alone. The planner runs a
part on several only if its tables hold at least
EXECUTOR.PARALLEL_MIN_ROWS rows, so a query of a few rows, as one by a
key, stays on its own thread. A grouping or DISTINCT of so many
groups, as the optimizer estimates them, that each morsel holds
nearly as many groups as rows runs on several only when enough threads
share its work. Its morsels' groups are merged a few morsels at a time,
so it holds about what its groups hold, and a morsel of few groups is
merged on the statement's thread at once. A scan or sort of a column
store table stays on it too while EXECUTOR.BATCHES is on, since its
scan by batch is faster alone; a join, window functions, a grouping or
DISTINCT over one still run in parallel, a grouping a batch at a time
on each thread. A bare COUNT(*) of a row store table stays on
it as well, since its storage counts it faster than any split.
[EXPLAIN](../sql/statements/explain.md)
shows the part and its degree. The rows, and their order, are the same
on any number of threads.

An operator holds its rows in memory up to a grant of the memory
budget, half of MEMORY.BUDGET_BYTES left free as it starts, at most
MEMORY.WORK_BYTES, then spills. Grants shrink as more statements run at
once, and never together pass the budget. A part run in parallel holds
one grant for all its threads. One of 4 MiB or less is held unreserved,
as a session that sets MEMORY.WORK_BYTES below 4 MiB asks.

```sql
SET EXECUTOR.DEGREE = 4;
SELECT SETTING_NAME, SETTING_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.SETTINGS
    WHERE SETTING_NAME IN ('EXECUTOR.DEGREE', 'EXECUTOR.MORSEL_ROWS', 'EXECUTOR.PARALLEL_MIN_ROWS', 'EXECUTOR.WORKERS', 'MEMORY.WORK_BYTES')
    ORDER BY SETTING_NAME;
SET EXECUTOR.DEGREE TO DEFAULT;
```

```
OK
 SETTING_NAME                                                 | SETTING_VALUE                                                | VALUE_SOURCE
--------------------------------------------------------------+--------------------------------------------------------------+--------------
 EXECUTOR.DEGREE                                              | 4                                                            | SESSION
 EXECUTOR.MORSEL_ROWS                                         | 16384                                                        | DEFAULT
 EXECUTOR.PARALLEL_MIN_ROWS                                   | 65536                                                        | DEFAULT
 EXECUTOR.WORKERS                                             | 0                                                            | DEFAULT
 MEMORY.WORK_BYTES                                            | 268435456                                                    | DEFAULT
(5 rows)
OK
```

Data changes write on the statement's own thread. The query choosing
the rows an INSERT, UPDATE, DELETE or MERGE changes may run in
parallel, as any query does.

### Batches and morsels

While EXECUTOR.BATCHES is on, a query reads its tables a batch of up to
2,048 rows at a time, each column a vector: integers, approximate
numbers, strings, dates and timestamps, and truth values each kept as
their own kind, their nulls as bits. A filter comparing such a column
with a constant, testing it for null, or an OR of such tests, judges a
whole vector at once. Any other expression of a batch's columns, in a
filter, a select list or a set function's argument, is compiled once and
evaluated a vector at a time: arithmetic, comparisons, AND, OR and NOT,
CASE, COALESCE, NULLIF, CAST, and ABS, CHAR_LENGTH, OCTET_LENGTH, UPPER,
LOWER and EXTRACT. A batch in which some row would raise an error or a
warning is evaluated again a row at a time, so the error, the warnings
and the rows before them are those a row at a time gives. A part run in parallel reads each morsel the same
way, its filters, projections and grouping done a batch at a time, so a
thread of a parallel query costs a row what the statement's own thread
alone does. A part whose morsels pass through a hash join's probe reads
them a row at a time.

An ORDER BY of a batch's rows makes each row's sort keys one string of
bytes, numbers in as few bytes as their range needs and strings up to
their first 32 bytes, and sorts the strings, by radix sort when they are
16 bytes or fewer, its rows made only as they are given. A sort past
MEMORY.WORK_BYTES writes the keys beside its rows and merges its runs by
them. A sort under FETCH FIRST keeps a batch's rows only if their keys
are below the greatest of those it holds, so of a long input it makes
only the rows it gives. Rows of equal keys keep the order they came in,
whatever the degree.

A grouping or DISTINCT run in parallel groups each morsel apart, then
merges the groups each morsel made. Of very many groups, each morsel's
groups are nearly as many as its rows, so the merge costs more than the
threads save. The planner takes the groups the optimizer estimates from
the table's statistics, and keeps such a grouping, and the scan under
it, on the statement's thread.

```sql
CREATE SCHEMA AUTHORIZATION ANN
    CREATE TABLE VISITS (K INTEGER, VISITED DATE, PAID BOOLEAN);
INSERT INTO ANN.VISITS VALUES (1, DATE '2026-01-05', TRUE), (2, DATE '2026-02-10', FALSE),
    (3, DATE '2026-03-15', NULL), (4, DATE '2026-03-20', TRUE);
SET EXECUTOR.PARALLEL_MIN_ROWS = 0;
SELECT PAID, COUNT(*) AS N FROM ANN.VISITS WHERE VISITED >= DATE '2026-02-01' OR PAID = TRUE
    GROUP BY PAID ORDER BY PAID;
SET EXECUTOR.PARALLEL_MIN_ROWS TO DEFAULT;
```

```
OK
OK
OK
 PAID  |                    N
-------+----------------------
 FALSE |                    1
 TRUE  |                    2
 NULL  |                    1
(3 rows)
OK
```

## Settings of plug-ins

An extension's manifest may declare settings of its own, and a host
plug-in's too ([Plug-ins](../client/04-plugins.md#settings)). Each is
named after its plug-in, as EXAMPLE.DEPTH, and from the time it is
declared it is set and shown as a built-in setting is, with the same
scopes, privileges, checks, keeping and reload. An extension's settings
are declared by CREATE EXTENSION, changed by ALTER EXTENSION ... UPDATE
and retired by DROP EXTENSION, and INFORMATION_SCHEMA.EXTENSION_OBJECTS
lists them with their extension. A host plug-in's are NODE settings,
given as PLUGINS.HOST is and listed in INFORMATION_SCHEMA.HOST_PLUGINS.
A configuration file may name a plug-in's setting before the plug-in is
loaded. A name in a group no built-in setting has is taken this way, and
its value waits for the setting, refused when the setting is declared if
it does not fit. A name in a built-in group must still be a setting of
it, so a misspelt built-in setting is refused as the file is read.

```toml
[VAULT]
KEEP_PAGES = 64
```

A plug-in's program reads the values its calls run with, a session's
own over the database's, and the built-in settings its manifest says it
reads, which it never writes. Sending them costs no call. A frame goes
before a call only when the values changed for that process.
