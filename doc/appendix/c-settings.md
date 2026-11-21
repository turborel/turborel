# Appendix C: Settings *(extension)*

[Contents](README.md)

Each setting, as INFORMATION_SCHEMA.SETTINGS lists it. A setting marked
*session* may be set by a session for itself, by [SET](../sql/statements/set-setting.md);
one marked *node* is the node's alone, given as its server starts and
kept nowhere ([Replication](../operator/09-replication.md));
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
with VALUE_SOURCE NODE. The settings marked *node* here are the database's own of the kind: PLUGINS.DIRECTORIES, PLUGINS.TRUSTED_KEYS, PLUGINS.REQUIRE_SIGNED and PLUGINS.HOST, which bound what the node runs, so only its operator sets them, never SQL.

## The database's settings

| Setting | Default | Values | Takes effect | |
|---|---|---|---|---|
| ARCHIVE.MODE | OFF | ONE OF ON, OFF | at once |  |
| ARCHIVE.RETENTION_SECONDS | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| ARCHIVE.SEGMENT_BYTES | 16777216 | INTEGER FROM 1 TO 9223372036854775807 | at once |  |
| ARCHIVE.TARGET | (empty) | TEXT OF AT MOST 4096 CHARACTERS | at once |  |
| ARCHIVE.TIMELINE | 1 | INTEGER FROM 1 TO 4294967295 | at once |  |
| ARCHIVE.TIMEOUT_MS | 60000 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| BACKUP.COMPRESSION | OFF | ONE OF ON, OFF | at once |  |
| CATALOG.COMPACT_AFTER_IMAGES | 64 | INTEGER FROM 1 TO 1000000 | at once |  |
| CATALOG.NAME | TURBOREL | AN IDENTIFIER | at once |  |
| CHECKPOINT.AFTER_BYTES | 1073741824 | INTEGER FROM 1 TO 9223372036854775807 | at once |  |
| CHECKPOINT.AFTER_CHANGES | 4000000 | INTEGER FROM 1 TO 9223372036854775807 | at once |  |
| CHECKPOINT.OVERDUE_FACTOR | 4 | INTEGER FROM 1 TO 1000000 | at once |  |
| COLUMNSTORE.HEADER_CACHE_BYTES | 16777216 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| DIAGNOSTICS.LEVEL | INFO | ONE OF ERROR, WARNING, INFO | at once |  |
| DIAGNOSTICS.SLOW_STATEMENT_MS | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| EXECUTOR.BATCHES | ON | ONE OF ON, OFF | at once | *session* |
| EXECUTOR.DECORRELATE_AFTER_RUNS | 8 | INTEGER FROM 0 TO 1000000 | at once |  |
| EXECUTOR.DEGREE | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once | *session* |
| EXECUTOR.GRAPH_THREADS | 1 | INTEGER FROM 1 TO 9223372036854775807 | at once | *session* |
| EXECUTOR.KEEP_SUBQUERY_PLANS | ON | ONE OF ON, OFF | at once | *session* |
| EXECUTOR.KERNELS | (empty) | TEXT OF AT MOST 64 CHARACTERS | at once | *session* |
| EXECUTOR.MAX_ITERATIONS | 1000000 | INTEGER FROM 0 TO 9223372036854775807 | at once | *session* |
| EXECUTOR.MORSEL_ROWS | 16384 | INTEGER FROM 1 TO 9223372036854775807 | at once | *session* |
| EXECUTOR.PARALLEL_MIN_ROWS | 65536 | INTEGER FROM 0 TO 9223372036854775807 | at once | *session* |
| EXECUTOR.RUNTIME_FILTERS | ON | ONE OF ON, OFF | at once | *session* |
| EXECUTOR.WORKERS | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| FILES.MAX_OPEN | 1024 | INTEGER FROM 0 TO 4294967295 | at once |  |
| GQL.GRAPH | (empty) | TEXT OF AT MOST 257 CHARACTERS | at once | *session* |
| GRAPH.DETERMINISTIC_PATHS | OFF | ONE OF ON, OFF | at once | *session* |
| GRAPH.MAX_PATHS_PER_PARTITION | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once | *session* |
| GRAPH.MAX_PATH_LENGTH | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once | *session* |
| GRAPH.MAX_SEARCH_STATES | 10000000 | INTEGER FROM 0 TO 9223372036854775807 | at once | *session* |
| GRAPH.ON_LIMIT | ERROR | ONE OF ERROR, TRUNCATE | at once | *session* |
| GRAPH.PARALLEL_MIN_STATES | 16384 | INTEGER FROM 0 TO 9223372036854775807 | at once | *session* |
| JOBS.POLICY | AUTOMATIC | ONE OF AUTOMATIC, MANUAL, PAUSED | at once |  |
| LOCKS.TIMEOUT_MS | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once | *session* |
| LOCKS.WAIT | OFF | ONE OF ON, OFF | from the next transaction | *session* |
| LOG.COMMIT_SYNC | ON | ONE OF ON, OFF | at once | *session* |
| LOG.DURABILITY | DATA | ONE OF NONE, DATA, FULL | at once |  |
| LOG.SEGMENT_BYTES | 67108864 | INTEGER FROM 4096 TO 1099511627775 | when the database is made |  |
| LOG.SYNC_AFTER_MS | 1000 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| MEMORY.BUDGET_BYTES | 1073741824 | INTEGER FROM 1048576 TO 9223372036854775807 | at once |  |
| MEMORY.WORK_BYTES | 268435456 | INTEGER FROM 65536 TO 9223372036854775807 | at once | *session* |
| OPTIMIZER.CONSTANT_FOLDING | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.DISTINCT_AGGREGATES | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.EAGER_AGGREGATION | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.ELIMINATE_JOINS | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.EXTREMES_BY_ORDER | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.FACTOR_DISJUNCTIONS | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.HOIST_RAISING_EXPRESSIONS | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.INLINE_FUNCTIONS | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.MAX_PASSES | 64 | INTEGER FROM 1 TO 1000000 | at once |  |
| OPTIMIZER.MERGE_FILTERS | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PREDICATE_TRANSITIVITY | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_FILTER_INTO_OUTER_JOIN | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_FILTER_INTO_PRODUCT | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_FILTER_THROUGH_DISTINCT | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_FILTER_THROUGH_PROJECT | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_FILTER_THROUGH_SET_OPERATION | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_FILTER_THROUGH_WINDOW | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_HAVING_INTO_WHERE | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_LIMIT_THROUGH_UNION | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.PUSH_RAISING_FILTER_INTO_PRODUCT | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.REMOVE_REDUNDANT_DISTINCT | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.SIMPLIFY_CONDITIONS | ON | ONE OF ON, OFF | at once | *session* |
| OPTIMIZER.UNNEST_SUBQUERIES | ON | ONE OF ON, OFF | at once | *session* |
| PLANNER.DP_BUDGET | 10000 | INTEGER FROM 1 TO 1000000 | at once | *session* |
| PLANNER.EAGER_GAIN | 7E-1 | NUMBER FROM 0E0 TO 1E6 | at once | *session* |
| PLANNER.EQUAL_SELECTIVITY | 1E-2 | NUMBER FROM 0E0 TO 1E0 | at once |  |
| PLANNER.INDEX_DESCENT_COST | 6E0 | NUMBER FROM 0E0 TO 1.7976931348623157E308 | at once |  |
| PLANNER.JOIN_ENUMERATION | ADAPTIVE | ONE OF ADAPTIVE, GREEDY | at once | *session* |
| PLANNER.LINEARIZED_RELATIONS | 100 | INTEGER FROM 2 TO 128 | at once | *session* |
| PLANNER.NULL_FRACTION | 1E-2 | NUMBER FROM 0E0 TO 1E0 | at once |  |
| PLANNER.RANGE_JOINS | ON | ONE OF ON, OFF | at once | *session* |
| PLANNER.RANGE_SELECTIVITY | 3.333333333333333E-1 | NUMBER FROM 0E0 TO 1E0 | at once |  |
| PLANNER.REORDER_GAIN | 9E-1 | NUMBER FROM 0E0 TO 9.99E-1 | at once |  |
| PLANNER.SORT_COMPARE_COST | 1E-1 | NUMBER FROM 0E0 TO 1.7976931348623157E308 | at once |  |
| PLANNER.STREAMED_GROUPING | ON | ONE OF ON, OFF | at once | *session* |
| PLUGINS.CALL_TIMEOUT_MS | 30000 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| PLUGINS.CANCEL_GRACE_MS | 1000 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| PLUGINS.DIRECTORIES | (empty) | TEXT OF AT MOST 65536 CHARACTERS | at once | *node* |
| PLUGINS.HOST | (empty) | TEXT OF AT MOST 65536 CHARACTERS | at the next start | *node* |
| PLUGINS.MAX_CALL_BYTES | 67108864 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| PLUGINS.MAX_MESSAGE_BYTES | 16777216 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| PLUGINS.MEMORY_BYTES | 268435456 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| PLUGINS.PROCESSES | 4 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| PLUGINS.REQUIRE_SIGNED | OFF | ONE OF ON, OFF | at once | *node* |
| PLUGINS.TRUSTED_KEYS | (empty) | TEXT OF AT MOST 65536 CHARACTERS | at once | *node* |
| SESSION.COMMIT_AT_END | ON | ONE OF ON, OFF | at once | *session* |
| SESSION.LANGUAGE | SQL | ONE OF SQL, GQL | at once | *session* |
| SESSION.TIME_ZONE | +00:00 | A TIME ZONE DISPLACEMENT FROM -12:59 TO +14:00 | at once | *session* |
| SPILL.BATCH_BYTES | 16384 | INTEGER FROM 4096 TO 1048576 | at once | *session* |
| SPILL.ENCODINGS | (empty) | ENCODINGS, BY COMMAS | at once |  |
| SPILL.PARTITIONS | 64 | INTEGER FROM 2 TO 4096 | at once | *session* |
| SPILL.PARTITION_DEPTH | 8 | INTEGER FROM 0 TO 64 | at once | *session* |
| SQL.FUNCTION_CALLS | ON | ONE OF ON, OFF | at once | *session* |
| STATEMENT.TIMEOUT_MS | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once | *session* |
| STATEMENTS.KEPT | 1000 | INTEGER FROM 0 TO 4294967295 | at once |  |
| STORAGE.CHECKSUM | XXH3_64 | A HASH FUNCTION | when the database is made |  |
| STORAGE.CHECKSUM_KEY | (empty) | TEXT OF AT MOST 64 CHARACTERS | at once |  |
| STORAGE.ENCODINGS | (empty) | ENCODINGS, BY COMMAS | at once |  |
| STORAGE.PAGE_BYTES | 8192 | INTEGER FROM 4096 TO 65536 | when the database is made |  |
| STORAGE.POOL_BYTES | 134217728 | INTEGER FROM 65536 TO 9223372036854775807 | at the next start |  |
| STORAGE.READ_AHEAD_PAGES | 32 | INTEGER FROM 0 TO 4294967295 | at once |  |
| STORAGE.REKEY_BYTES_PER_SECOND | 8388608 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| STORAGE.SYSTEM_CACHE_BYTES | 4294967296 | INTEGER FROM 0 TO 9223372036854775807 | at once |  |
| TABLES.ENGINE | rowstore | A STORAGE ENGINE | at once |  |
| TABLES.UNIQUE_INDEX_METHOD | btree | AN INDEX METHOD | at once |  |
| TRANSACTION.ISOLATION | READ COMMITTED | ONE OF READ UNCOMMITTED, READ COMMITTED, REPEATABLE READ, SERIALIZABLE | from the next transaction | *session* |
| TRANSACTION.STRICT | ON | ONE OF ON, OFF | from the next transaction |  |
| VECTOR.APPROX_DEFAULT | OFF | ONE OF ON, OFF | at once | *session* |
| VECTOR.APPROX_FALLBACK | ON | ONE OF ON, OFF | at once | *session* |
| VECTOR.BEAM_WIDTH | 0 | INTEGER FROM 0 TO 16 | at once | *session* |
| VECTOR.BUILD_MEMORY_BYTES | 268435456 | INTEGER FROM 65536 TO 9223372036854775807 | at once |  |
| VECTOR.SEARCH_LIST | 0 | INTEGER FROM 0 TO 10000 | at once | *session* |
| XQUERY.STATIC_TYPING | OFF | ONE OF ON, OFF | at once | *session* |

Each in a sentence:

- **ARCHIVE.MODE**: Whether the log's records are archived to ARCHIVE.TARGET as they are made durable, for recovery to a point in time: ON; or not, OFF. While ON, a checkpoint keeps every record not yet archived.
- **ARCHIVE.RETENTION_SECONDS**: The seconds an archived segment is kept, 0 for ever. An older one is removed once the newest backup no longer needs its records.
- **ARCHIVE.SEGMENT_BYTES**: The bytes of records gathered before they are archived as one segment.
- **ARCHIVE.TARGET**: Where the log is archived: a directory's path, or a location of a target provider's scheme, as scheme:rest; none if empty.
- **ARCHIVE.TIMELINE**: The timeline the database's archived records are kept under: 1 for a database never restored, and a new one for each restore, set by the restore so it never writes over the archive of the database it was restored from.
- **ARCHIVE.TIMEOUT_MS**: The most milliseconds a durable record waits to be archived, whatever ARCHIVE.SEGMENT_BYTES says; 0 archives every record as soon as the archiver runs.
- **BACKUP.COMPRESSION**: Whether backups and archived segments pass through the LZ4 encoding, which must be registered, before the database's own encodings: ON; or not, OFF.
- **CATALOG.COMPACT_AFTER_IMAGES**: The images the catalog file holds after which the next is written, alone, to the other file.
- **CATALOG.NAME**: The name of the catalog the schemata are in, as the Information Schema gives it [ISO/IEC 9075:1992 §21.2.2].
- **CHECKPOINT.AFTER_BYTES**: The bytes of the data file's side file, each page once however often it was written, after which the end of a transaction takes a checkpoint.
- **CHECKPOINT.AFTER_CHANGES**: The row changes logged after which the end of a transaction takes a checkpoint.
- **CHECKPOINT.OVERDUE_FACTOR**: How many times CHECKPOINT.AFTER_CHANGES or CHECKPOINT.AFTER_BYTES the log, or the data file's side file, grows to with no checkpoint giving it up before the diagnostics are told that checkpoints are overdue.
- **COLUMNSTORE.HEADER_CACHE_BYTES**: The bytes of the headers of frozen row groups the column store holds in memory at most, of its tables together, reserved of MEMORY.BUDGET_BYTES as they are held; 0 for none, each read as it is needed.
- **DIAGNOSTICS.LEVEL**: The least level of the events told to the diagnostics: ERROR, those alone; WARNING, warnings too; INFO, every event.
- **DIAGNOSTICS.SLOW_STATEMENT_MS**: The milliseconds of work after which a statement is told to the diagnostics as slow; 0 for none.
- **EXECUTOR.BATCHES**: Whether a query reads its tables a batch of rows at a time where it can: ON; or a row at a time throughout, OFF, as to compare the two.
- **EXECUTOR.DECORRELATE_AFTER_RUNS**: The runs of a correlated subquery, for as many sets of values, after which it is run once for all of them.
- **EXECUTOR.DEGREE**: The most threads a statement's query runs on at once, its own among them; 0, the default, for as many as the workers, and 1 for its own thread alone. Its rows are the same either way.
- **EXECUTOR.GRAPH_THREADS**: The threads a graph algorithm of GRAPH_ALGO, or a GRAPH_TABLE's search from many starts, runs on at once, the statement's own among them; its results are the same on any number.
- **EXECUTOR.KEEP_SUBQUERY_PLANS**: Whether a correlated subquery, once its first runs show the values of its outer references choose no other plan, keeps its plan made with them as parameters and fills it at each later run: ON; or plans it at each run with its values, OFF, as to compare the two. Its rows are the same either way.
- **EXECUTOR.KERNELS**: The kernels a statement's batches run on: PORTABLE for the built-in ones, as to diagnose others; empty, the default, or the registered set's name, for the set registered, such as SIMD.
- **EXECUTOR.MAX_ITERATIONS**: The most iterations a recursive query of WITH RECURSIVE takes, each evaluating its recursive elements once more, before it is refused (SQLCODE -918) as never reaching its fixpoint; 0 for no limit.
- **EXECUTOR.MORSEL_ROWS**: The rows of a table, about, that a worker of a parallel query reads at a time, a morsel.
- **EXECUTOR.PARALLEL_MIN_ROWS**: The least rows a table a query reads holds, as the planner estimates, for the query to read it on workers; a smaller one is read by the statement's own thread alone. 0 for any table.
- **EXECUTOR.RUNTIME_FILTERS**: Whether a hash join, once it has built its right operand, tells the scan of its left operand the least and greatest of its keys and filters the rows read by a Bloom filter of them: ON; or not, OFF, as to compare the two. Its rows are the same either way.
- **EXECUTOR.WORKERS**: The executor's workers, threads shared by every statement of the database that runs in parallel; 0, the default, for as many as the system says it runs at once.
- **FILES.MAX_OPEN**: The most files of the database open at once, 0 for no limit: past it, the one used least recently is closed (one with writes not yet synced last, and synced first), and opened again as it is next used.
- **GQL.GRAPH**: The property graph a GQL statement reads when it names none, as SESSION SET GRAPH sets it, named in the default schema if unqualified; empty for none.
- **GRAPH.DETERMINISTIC_PATHS**: Whether a GRAPH_TABLE's selectors that keep some of equal paths, as ANY and ANY SHORTEST, keep the same ones each run, ON, ordered by the elements' identities; or any, OFF.
- **GRAPH.MAX_PATHS_PER_PARTITION**: The most paths a GRAPH_TABLE's search finds between one start and one end; 0 for no limit. Reaching it does as GRAPH.ON_LIMIT says.
- **GRAPH.MAX_PATH_LENGTH**: The most edges of a path a GRAPH_TABLE's search follows; 0 for no limit. Reaching it does as GRAPH.ON_LIMIT says.
- **GRAPH.MAX_SEARCH_STATES**: The most states a GRAPH_TABLE's search makes, each a vertex reached at a place of its pattern; 0 for no limit. Reaching it does as GRAPH.ON_LIMIT says.
- **GRAPH.ON_LIMIT**: What a GRAPH_TABLE's search does on reaching a limit: fails, SQLSTATE 54061, ERROR; or ends with the paths found, warning 01W61, TRUNCATE.
- **GRAPH.PARALLEL_MIN_STATES**: The least states, estimated from the graph's size, a GRAPH_TABLE's search makes for it to run on the workers EXECUTOR.GRAPH_THREADS allows; a smaller one runs on the statement's thread alone; 0 for any search. Its results are the same either way.
- **JOBS.POLICY**: How the database's background work, its checkpoints, maintenance, syncs of the log (LOG.SYNC_AFTER_MS), read-ahead (STORAGE.READ_AHEAD_PAGES), the writing of its files again in new formats (STORAGE.REKEY_BYTES_PER_SECOND) and the archiving of its log (ARCHIVE.MODE), runs: on threads of its own as it comes due, AUTOMATIC; only when the program asks, MANUAL; or not until this changes, PAUSED.
- **LOCKS.TIMEOUT_MS**: The milliseconds a statement waits for a lock or a row, under LOCKS.WAIT ON, before it is refused (SQLCODE -911); 0 for no limit.
- **LOCKS.WAIT**: ON waits for a lock, or a row, another transaction holds until it ends; OFF refuses the statement at once. ON in a database a server serves.
- **LOG.COMMIT_SYNC**: Whether a commit waits until its commit record is synced to the log, ON; or returns once the record is written, OFF: the transaction is then seen by others at once, and lost if the system fails before the log is next synced, by a commit of ON, a definition, a checkpoint, the database closing, or LOG.SYNC_AFTER_MS passing.
- **LOG.DURABILITY**: How durable a sync of the log makes what it holds: NONE, never synced, so a failure of the system may lose any commit, and TRANSACTION.STRICT takes the log as not durable; DATA, its data on the device with what reading it back needs; FULL, with all of its files' metadata too. A change holds from the log's next sync, which after NONE makes every record before it durable too, and each segment of the log begun after is made as it says; a log plug-in takes it as far as it can.
- **LOG.SEGMENT_BYTES**: The bytes after which a segment of the log is ended and the next begun.
- **LOG.SYNC_AFTER_MS**: The milliseconds after a commit of LOG.COMMIT_SYNC OFF within which the log is synced, by a job of its own if nothing else syncs it first; 0 for none, the commit then synced only by the log's next sync for another reason.
- **MEMORY.BUDGET_BYTES**: The bytes the database's buffer pools, work areas and caches may reserve in all, with the files of a database kept in memory, its pages, temporary tables and spills; lowering it takes back none they hold.
- **MEMORY.WORK_BYTES**: The most a sort, hash join, grouping, DISTINCT, EXCEPT, INTERSECT or window holds in memory before it spills: half the free MEMORY.BUDGET_BYTES as it starts, reserved, but 4 MiB unreserved where that is less, and itself below 4 MiB. The lesser of it and 4 MiB bounds a cursor's rows read ahead or kept to scroll, the rows a change chooses and keeps for its checks and referential actions, and a session's temporary tables' pages, the rest spilled.
- **OPTIMIZER.CONSTANT_FOLDING**: Whether the optimizer's rule constant_folding rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.DISTINCT_AGGREGATES**: Whether the optimizer's rule distinct_aggregates rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.EAGER_AGGREGATION**: Whether the optimizer's rule eager_aggregation rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.ELIMINATE_JOINS**: Whether the optimizer's rule eliminate_joins rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.EXTREMES_BY_ORDER**: Whether the optimizer's rule extremes_by_order rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.FACTOR_DISJUNCTIONS**: Whether the optimizer's rule factor_disjunctions rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.HOIST_RAISING_EXPRESSIONS**: Whether the optimizer's rule hoist_raising_expressions rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.INLINE_FUNCTIONS**: Whether the optimizer's rule inline_functions rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.MAX_PASSES**: The most passes the optimizer makes over a plan.
- **OPTIMIZER.MERGE_FILTERS**: Whether the optimizer's rule merge_filters rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PREDICATE_TRANSITIVITY**: Whether the optimizer's rule predicate_transitivity rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_FILTER_INTO_OUTER_JOIN**: Whether the optimizer's rule push_filter_into_outer_join rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_FILTER_INTO_PRODUCT**: Whether the optimizer's rule push_filter_into_product rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_FILTER_THROUGH_DISTINCT**: Whether the optimizer's rule push_filter_through_distinct rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_FILTER_THROUGH_PROJECT**: Whether the optimizer's rule push_filter_through_project rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_FILTER_THROUGH_SET_OPERATION**: Whether the optimizer's rule push_filter_through_set_operation rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_FILTER_THROUGH_WINDOW**: Whether the optimizer's rule push_filter_through_window rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_HAVING_INTO_WHERE**: Whether the optimizer's rule push_having_into_where rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_LIMIT_THROUGH_UNION**: Whether the optimizer's rule push_limit_through_union rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.PUSH_RAISING_FILTER_INTO_PRODUCT**: Whether the optimizer's rule push_raising_filter_into_product rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.REMOVE_REDUNDANT_DISTINCT**: Whether the optimizer's rule remove_redundant_distinct rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.SIMPLIFY_CONDITIONS**: Whether the optimizer's rule simplify_conditions rewrites plans: ON; or OFF, as to compare plans made without it.
- **OPTIMIZER.UNNEST_SUBQUERIES**: Whether the optimizer's rule unnest_subqueries rewrites plans: ON; or OFF, as to compare plans made without it.
- **PLANNER.DP_BUDGET**: The connected subsets of a query's joined tables up to which ADAPTIVE join ordering finds the cheapest plan exactly. Past it the search is linearized, and parts of a greedy plan are planned again by dynamic programming until this many table entries are made.
- **PLANNER.EAGER_GAIN**: How much less work a grouping over joins, planned with one of its tables grouped first by the columns the joins and the grouping read, must be estimated to do than the grouping planned as written, to be planned so. 0 never groups first, and a value past 1 groups first even where that is estimated to cost more, as to compare the two.
- **PLANNER.EQUAL_SELECTIVITY**: The fraction of rows an equality keeps when the statistics do not tell.
- **PLANNER.INDEX_DESCENT_COST**: The cost of one level of an index descent, its page held by the buffer pool, a row read in sequence costing 1.
- **PLANNER.JOIN_ENUMERATION**: How the joins of a query are ordered. ADAPTIVE orders them by dynamic programming over bushy plans, outer joins among them, linearized or guided greedily as the query grows, and GREEDY orders them left-deep, each next join the cheapest, as to compare the two.
- **PLANNER.LINEARIZED_RELATIONS**: The most tables ADAPTIVE join ordering plans whole by linearized dynamic programming when exact search is past its budget, and the size of the parts of a greedy plan it plans again so for larger queries.
- **PLANNER.NULL_FRACTION**: The fraction of a column's values taken to be null when the statistics do not tell.
- **PLANNER.RANGE_JOINS**: Whether a join whose condition compares a value of each table by <, <=, > or >=, a band or an overlap of periods among them, may sort one table and have each row of the other search it, rather than judge every pair. OFF plans such joins as nested loops, as to compare the two.
- **PLANNER.RANGE_SELECTIVITY**: The fraction of rows an order comparison, a pattern or a subquery keeps when the statistics do not tell.
- **PLANNER.REORDER_GAIN**: A join order is taken over the FROM clause's if its estimated cost is less than this fraction of that order's.
- **PLANNER.SORT_COMPARE_COST**: The cost of one comparison of a sort, a row read in sequence costing 1.
- **PLANNER.STREAMED_GROUPING**: Whether a grouping or DISTINCT whose rows come in the order of its keys, or cost less sorted, is made a group at a time as its rows come, with nothing held, ON, or always by a table of its groups, OFF, as to compare the two.
- **PLUGINS.CALL_TIMEOUT_MS**: The milliseconds a call of a plug-in may take before its process is stopped and the call fails with SQLSTATE 38T02; 0 for no limit.
- **PLUGINS.CANCEL_GRACE_MS**: The milliseconds a plug-in has to answer a cancelled call before its process is stopped.
- **PLUGINS.DIRECTORIES**: The directories an extension's plug-in may be in, separated by ';', each holding any directory below it; empty, as it is until the host sets it, for none. The host sets it as the database opens; SQL may not.
- **PLUGINS.HOST**: The host plug-ins the node starts before the database opens, each name=directory, its program's SHA-256 digest pinned by @digest after the directory, separated by ';'; empty, as it is until the host sets it, for none. The host sets it; SQL may not.
- **PLUGINS.MAX_CALL_BYTES**: The most bytes a plug-in sends in answer to one call, all its messages together; more stops its process, and the call fails with SQLSTATE 38T04.
- **PLUGINS.MAX_MESSAGE_BYTES**: The most bytes of one message a plug-in sends; a longer one stops its process, and the call fails with SQLSTATE 38T04.
- **PLUGINS.MEMORY_BYTES**: The memory a plug-in is told it may use, in its HELLO; a program that uses more is the operator's to stop, as the database makes no calls of the operating system to cap it.
- **PLUGINS.PROCESSES**: The most processes of one extension running at once; a call waits for one to be free. 0 is taken as 1.
- **PLUGINS.REQUIRE_SIGNED**: Whether a package must be signed by a key of PLUGINS.TRUSTED_KEYS to be installed or loaded, ON, or may be unsigned, OFF. The host sets it; SQL may not.
- **PLUGINS.TRUSTED_KEYS**: The Ed25519 public keys whose signatures on a package are trusted, each 64 hexadecimal digits, separated by ';', or '@' and a file of one a line. The host sets it; SQL may not.
- **SESSION.COMMIT_AT_END**: Whether a transaction open when its session ends is committed, ON, or rolled back, OFF [X3.135-1986 §7.1 GR2].
- **SESSION.LANGUAGE**: The language a session's direct statements are read in, SQL or GQL; in GQL each statement but SET is a GQL request.
- **SESSION.TIME_ZONE**: The time zone displacement of an SQL-session, ±HH:MM from -12:59 to +14:00 [ISO/IEC 9075-2:2023 §4.7.2]: of CURRENT_TIME and CURRENT_TIMESTAMP, and implied by a time or timestamp without time zone.
- **SPILL.BATCH_BYTES**: The bytes of rows a spilling operator gathers before it writes them to its work area; it reads or writes a run for every two batches its MEMORY.WORK_BYTES holds.
- **SPILL.ENCODINGS**: The encodings, as compression, the pages of work areas' files pass through as each is written from now on, by commas, in order, before the encryption they pass through while the database encrypts anything; none if empty.
- **SPILL.PARTITIONS**: The most partitions a spilled operator (a join, grouping, DISTINCT, EXCEPT or INTERSECT) writes at once; it writes fewer if its MEMORY.WORK_BYTES holds fewer runs.
- **SPILL.PARTITION_DEPTH**: The most times a spilled operator partitions a partition again, before it holds the partition in a table, or, grouping and an outer join, makes it in passes.
- **SQL.FUNCTION_CALLS**: Whether a statement may call a function the program registered, of no schema, by NAME(arguments): ON; or takes calls of the schema's own functions alone, as SQL does, OFF. A definition, as a CHECK, is checked as it is written, and keeps its calls whatever the setting is later.
- **STATEMENT.TIMEOUT_MS**: The milliseconds a statement may run, each call and each fetch of a cursor alone, before it is cancelled (SQLCODE -920); 0 for no limit.
- **STATEMENTS.KEPT**: The most statements, by normalized text and user, INFORMATION_SCHEMA.STATEMENT_STATISTICS keeps, each session's and those of sessions ended alike, past which a new one takes the place of the one called least; 0 keeps none, and setting it to 0 forgets them all.
- **STORAGE.CHECKSUM**: The hash function every file is checksummed with.
- **STORAGE.CHECKSUM_KEY**: The name of the key, of the keyring, a keyed checksum (STORAGE.CHECKSUM, as HMAC-SHA-256) makes its digests with from now on; empty, the key the function was registered with. What was checksummed with a key before is verified with it while the keyring holds it.
- **STORAGE.ENCODINGS**: The encodings, as compression or encryption, every file passes through from now on, by commas, in order; none if empty. What was written through others before is read through them while they are registered, and written again through these by the job turborel-rekey.
- **STORAGE.PAGE_BYTES**: The bytes of a page of the data file, a power of two.
- **STORAGE.POOL_BYTES**: The bytes of the buffer pool of the data file, reserved of MEMORY.BUDGET_BYTES as the database is opened; of a database kept in memory, the pool its engine and index methods share, made with its first table.
- **STORAGE.READ_AHEAD_PAGES**: The pages a scan of a table of the built-in stores, or of a B-tree index, has read ahead of it at once, at most an eighth of the buffer pool's pages: read by the database's job turborel-read-ahead while the scan goes on, each run of them in one read of the file; 0 for none. A change holds at once, a scan under way included.
- **STORAGE.REKEY_BYTES_PER_SECOND**: The bytes of the data file the job turborel-rekey reads a second, writing again the pages still in other formats than STORAGE.ENCODINGS and STORAGE.CHECKSUM_KEY name, until every file is in them; 0 pauses it. A change holds at once.
- **STORAGE.SYSTEM_CACHE_BYTES**: The bytes of the data file the planner takes the operating system's cache to hold beyond the buffer pool: a fetch of a page the pool misses is priced as read from that cache by the share of the file past the pool it holds, the rest as read from the disk; 0 prices every such page as read from the disk. Only the planner's costs read it, and not of a database kept in memory; a change holds for statements planned after it.
- **TABLES.ENGINE**: The storage engine of tables a schema creates.
- **TABLES.UNIQUE_INDEX_METHOD**: The index method behind the UNIQUE constraints of tables created from now on.
- **TRANSACTION.ISOLATION**: The isolation level of a transaction SET TRANSACTION gives none: READ COMMITTED as PostgreSQL's default is, where ISO/IEC 9075 says SERIALIZABLE [ISO/IEC 9075:1992 §14.1 SR2], so SERIALIZABLE here conforms.
- **TRANSACTION.STRICT**: ON refuses an engine that falls short of serializable, recoverable transactions [X3.135-1986 §4.16]; OFF allows it.
- **VECTOR.APPROX_DEFAULT**: Whether a FETCH FIRST of neither APPROX nor EXACT may read an approximate index, as FETCH APPROX does: ON; or is exact, OFF.
- **VECTOR.APPROX_FALLBACK**: Whether an approximate search that finds fewer rows than asked, once widened, or whose index declines, reads the whole table exactly: ON; or is refused, OFF.
- **VECTOR.BEAM_WIDTH**: The nodes whose neighbours a DiskANN search reads together each hop, unless the query names its own; 0 for each index's own.
- **VECTOR.BUILD_MEMORY_BYTES**: The bytes a vector index's build holds in memory; past them it builds parts and merges them.
- **VECTOR.SEARCH_LIST**: The candidates an approximate search keeps, HNSW's ef and DiskANN's L, unless the query names its own; 0 for each index's own.
- **XQUERY.STATIC_TYPING**: Whether the XQuery expressions of XMLQUERY, XMLEXISTS and XMLTABLE are statically typed as a statement is prepared, each type error raised then, before any is evaluated: ON; or as a query declaring the option static-typing of urn:turborel:option asks, OFF. A definition is typed as it is made, as the value of the session making it or its query's option asks, and keeps that typing whenever it is bound again.

## The server's settings

| Setting | Default | Values | Takes effect | |
|---|---|---|---|---|
| REPLICATION.CA | (empty) | TEXT OF AT MOST 1024 CHARACTERS | at the next start | *node* |
| REPLICATION.LISTEN | (empty) | TEXT OF AT MOST 1024 CHARACTERS | at the next start | *node* |
| REPLICATION.MEMBERS | (empty) | TEXT OF AT MOST 1024 CHARACTERS | at the next start | *node* |
| REPLICATION.READ_AFTER | 0 | INTEGER FROM 0 TO 9223372036854775807 | at once | *session* |
| REPLICATION.REPLICA | 0 | INTEGER FROM 0 TO 4294967295 | at the next start | *node* |
| REPLICATION.RETAIN_BYTES | 268435456 | INTEGER FROM 0 TO 9223372036854775807 | at the next start | *node* |
| REPLICATION.TICK_MS | 100 | INTEGER FROM 10 TO 60000 | at the next start | *node* |
| REPLICATION.TLS | ON | ONE OF ON, OFF | at the next start | *node* |
| SERVER.BATCH_BYTES | 65536 | INTEGER FROM 1 TO 4294967295 | at once |  |
| SERVER.COMPRESSION_MIN_BYTES | 1024 | INTEGER FROM 0 TO 16777216 | from the next connection |  |
| SERVER.HOST | 127.0.0.1 | TEXT OF AT MOST 1024 CHARACTERS | at the next start |  |
| SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS | 0 | INTEGER FROM 0 TO 4294967295 | at once |  |
| SERVER.IDLE_TIMEOUT_MS | 0 | INTEGER FROM 0 TO 4294967295 | at once |  |
| SERVER.KEEPALIVE_IDLE_MS | 0 | INTEGER FROM 0 TO 32767000 | from the next connection |  |
| SERVER.KEEPALIVE_INTERVAL_MS | 0 | INTEGER FROM 0 TO 32767000 | from the next connection |  |
| SERVER.KEEPALIVE_PROBES | 0 | INTEGER FROM 0 TO 127 | from the next connection |  |
| SERVER.LOB_INLINE_BYTES | 65536 | INTEGER FROM 0 TO 8388608 | at once |  |
| SERVER.LOGIN_DELAY_MS | 10000 | INTEGER FROM 0 TO 4294967295 | at once |  |
| SERVER.LOGIN_TIMEOUT_MS | 60000 | INTEGER FROM 0 TO 4294967295 | at once |  |
| SERVER.MAX_CONNECTIONS | 100 | INTEGER FROM 1 TO 4294967295 | at once |  |
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
| SERVER.WORKERS | four per processor | INTEGER FROM 1 TO 1024 | at once |  |

Each in a sentence:

- **REPLICATION.CA**: The PEM file of the authorities the members' certificates are verified against, each member presenting SERVER.TLS_CERTIFICATE and SERVER.TLS_KEY to the others.
- **REPLICATION.LISTEN**: The host and port this node takes the other members' frames at; empty, those its own member names in REPLICATION.MEMBERS.
- **REPLICATION.MEMBERS**: The members of the replica group: each its number, =, and the host and port its replica takes frames at, by commas, as 1=db1.example.com:7423,2=db2.example.com:7423.
- **REPLICATION.READ_AFTER**: The log position, as a commit's Complete gave it, that a session's statements wait for the database to have applied before they run, within STATEMENT.TIMEOUT_MS; 0 for none.
- **REPLICATION.REPLICA**: This node's number among the members of its database's replica group; 0, the database is not replicated.
- **REPLICATION.RETAIN_BYTES**: The bytes of the group's log the replica keeps before its database's last checkpoint at least: a backup behind by less catches up from the log, one behind by more from a state transfer of the primary's files.
- **REPLICATION.TICK_MS**: The milliseconds between the ticks of the replica's consensus, in which its heartbeats and the timeouts of its view changes count.
- **REPLICATION.TLS**: Whether the members secure their frames to each other by mutual TLS, ON; OFF only for a group on one machine, as in tests.
- **SERVER.BATCH_BYTES**: About how many bytes of rows the server sends in one batch.
- **SERVER.COMPRESSION_MIN_BYTES**: The bytes of the shortest frame sent compressed to a client that asked for compression, read as it connects; 0 for no compression.
- **SERVER.HOST**: Hosts or addresses, by commas, where TCP connections are taken.
- **SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS**: The milliseconds the server waits for the next request of a client in a transaction before it closes the connection, the transaction rolled back and its locks let go; 0 for no limit.
- **SERVER.IDLE_TIMEOUT_MS**: The milliseconds the server waits on a client, for its next request or to take what it is sent, before it closes the connection, its transaction rolled back; 0 for no limit.
- **SERVER.KEEPALIVE_IDLE_MS**: The milliseconds a TCP connection is idle before the system probes whether its client is still there, rounded up to whole seconds; 0 for the system's own.
- **SERVER.KEEPALIVE_INTERVAL_MS**: The milliseconds between the system's probes of a TCP connection's client, unanswered, rounded up to whole seconds; 0 for the system's own.
- **SERVER.KEEPALIVE_PROBES**: The probes unanswered after which the system closes a TCP connection, its transaction rolled back; 0 for the system's own.
- **SERVER.LOB_INLINE_BYTES**: The longest large object, in octets, a result sends whole; a longer one is sent by its locator, which the client reads a chunk at a time.
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
- **SERVER.WORKERS**: Statements the server runs at once, at most; a worker waiting on its client for the next request runs none.

Plug-ins may declare settings of their own, listed alike: a plug-in of
any kind its own, under its name, as the column store declares
COLUMNSTORE.HEADER_CACHE_BYTES; a rule added to the optimizer, OPTIMIZER
and its name, as the built-in rules have.
