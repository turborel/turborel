# Monitoring *(extension)*

[Contents](README.md)

A database shows what its sessions do now and what its statements have
done, in views of the Information Schema. A server's connections are in
CONNECTIONS, and the statements run since the database was opened are in
STATEMENT_STATISTICS. What each table holds and takes is in
TABLE_STATISTICS, and how each index is used in INDEX_STATISTICS. All
four are this implementation's own
([The Information Schema](../sql/07-information-schema.md#this-implementations-own-views-extension)).

## Who sees what

Statement texts carry the values written in them, so who sees them is
narrowed. The database's owner sees everything. So does a user granted
the role MONITOR, if the owner created that role. The right is given and
taken back by the standard's own statements:

```sql
CREATE ROLE MONITOR;
GRANT MONITOR TO BOB;
```

```
OK
OK
```

REVOKE MONITOR FROM BOB takes it back. A role named MONITOR that another
user created gives nothing. Any other user sees their own connections in
CONNECTIONS, and in STATEMENT_STATISTICS every statement, other users'
texts shown as NULL. In TABLE_STATISTICS and INDEX_STATISTICS they see
the tables on which they hold a privilege, and those tables' indexes.

## What each session does

CONNECTIONS has, beside a connection's user, address and state, what its
session does now:

| Column | |
|---|---|
| STATEMENT_TEXT | the statement it runs, as its client sent it, while its STATE is RUNNING; else NULL |
| STATEMENT_BEGAN | when that statement began |
| TRANSACTION_BEGAN | when its open transaction began, the start of the statement that began it; NULL with none open |
| WAITING_ON | LOCK, waiting for a table's lock or a row another transaction changed; IO, waiting for the log's sync of a commit; WORKER, a request waiting for one of SERVER.WORKERS; else NULL |
| LOCK_NAME | the table or row waited for |
| LOCK_HOLDER | the ID of the connection whose transaction holds it, or NULL if no connection's does |

```sql
SELECT ID, STATE, STATEMENT_TEXT FROM INFORMATION_SCHEMA.CONNECTIONS;
```

```
                   ID | STATE   | STATEMENT_TEXT
----------------------+---------+--------------------------------------------------------------
                    1 | RUNNING | SELECT ID, STATE, STATEMENT_TEXT FROM INFORMATION_SCHEMA.CONNECTIONS
(1 row)
```

A session waiting on a lock names the connection holding it, which
[CANCEL CONNECTION](../sql/statements/cancel-connection.md) or
[END CONNECTION](../sql/statements/end-connection.md) can stop.

## Statement statistics

STATEMENT_STATISTICS has a row for each statement run since the database
was opened, by its normalized text and its user. A normalized text is the
statement's words one space apart, each literal written `?`, so runs that
differ in their literals alone are one statement, with a prepared
statement's runs of the same text.

| Column | |
|---|---|
| USER_NAME | the user who ran it |
| STATEMENT_ID | the normalized text's key, 16 hexadecimal digits, the same for every user |
| STATEMENT_TEXT | the normalized text, or NULL if another user's and not shown |
| CALLS | its runs |
| TOTAL_MICROS, MEAN_MICROS, MAX_MICROS | the microseconds of their work, all of them, on average, and the most one took |
| ROW_COUNT | the rows they changed, or a query's fetched |
| ROWS_READ | the rows their scans read from tables and indexes |
| BYTES_SPILLED | the bytes their sorts, groupings and joins put in work areas past MEMORY.WORK_BYTES |
| LAST_RUN | when the last began |

A statement's work is its call; a query's, its opening and every fetch,
not the time between, which is its client's.

```sql
CREATE TABLE T (K INTEGER);
INSERT INTO T VALUES (1);
INSERT INTO T VALUES (2);
SELECT K FROM T WHERE K > 1;
SELECT STATEMENT_TEXT, CALLS, ROW_COUNT, ROWS_READ
    FROM INFORMATION_SCHEMA.STATEMENT_STATISTICS
    WHERE STATEMENT_TEXT LIKE '% T %' AND STATEMENT_TEXT NOT LIKE '%STATISTICS%'
    ORDER BY STATEMENT_TEXT;
```

```
OK
OK
OK
            K
--------------
            2
(1 row)
 STATEMENT_TEXT                                               |                CALLS |            ROW_COUNT |            ROWS_READ
--------------------------------------------------------------+----------------------+----------------------+----------------------
 CREATE TABLE T (K INTEGER)                                   |                    1 |                    0 |                    0
 INSERT INTO T VALUES (?)                                     |                    2 |                    2 |                    0
 SELECT K FROM T WHERE K > ?                                  |                    1 |                    1 |                    2
(3 rows)
```

STATEMENTS.KEPT bounds how many statements are kept, 1000 unless set
otherwise ([Settings](../appendix/c-settings.md)). Past it, a new
statement takes the place of the one called least. Setting it to 0 keeps
none and forgets them all, which is how the statistics are reset. SET
DATABASE runs outside a transaction, so the one open is ended first:

```sql
COMMIT;
SET DATABASE STATEMENTS.KEPT = 0;
SET DATABASE STATEMENTS.KEPT = DEFAULT;
```

```
OK
OK
OK
```

## Table and index statistics

TABLE_STATISTICS has a row for each base table, and INDEX_STATISTICS a
row for each index. A partitioned table's row sums its partitions'.

| Column | |
|---|---|
| ENGINE_NAME | the table's engine |
| ROW_COUNT | its rows |
| BYTES | the bytes it takes, or NULL if its engine does not know them |
| BYTES_ESTIMATED | YES if some of BYTES is estimated, else NO |
| LAST_CHANGED | when a transaction that wrote it last committed, or NULL if none has since the database was opened |
| LAST_COMPACTED | when its engine's maintenance last did some work on it, or NULL |

The built-in engines count the pages of a column store table's frozen
groups. They estimate the rows still in a table's tree as their number
times the mean width its statistics last measured, so BYTES of a row
store table is NULL until its statistics are first gathered. Reading
TABLE_STATISTICS asks for them, and the database's maintenance gathers
them soon after.

| Column | |
|---|---|
| METHOD_NAME | the index's method |
| SEARCHES | the searches made of it since the database was opened or it was made: a scan of it, or a probe of an index join |
| ENTRIES_READ | the entries those searches gave |
| ENTRY_COUNT | its entries, or NULL if its method does not know them yet |
| HEIGHT | the levels of its tree, 1 while the root is a leaf, or NULL of a method keeping none |
| BYTES | its bytes, or NULL if its method does not know them |
| DISTINCT_KEYS | the distinct keys it counts, which the planner estimates an equality on all its columns by, or NULL while it has not counted them |

An index whose SEARCHES stays 0 under a workload costs its writes and
helps no read.

```sql
CREATE INDEX T_K ON T (K);
INSERT INTO T SELECT K + 2 FROM T;
INSERT INTO T SELECT K + 4 FROM T;
SELECT K FROM T WHERE K = 3;
COMMIT;
SELECT TABLE_NAME, ENGINE_NAME, ROW_COUNT, LAST_CHANGED
    FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_SCHEMA = 'ANN';
SELECT INDEX_NAME, METHOD_NAME, SEARCHES, ENTRIES_READ, ENTRY_COUNT, HEIGHT
    FROM INFORMATION_SCHEMA.INDEX_STATISTICS WHERE TABLE_SCHEMA = 'ANN';
```

```
OK
OK
OK
            K
--------------
            3
(1 row)
OK
 TABLE_NAME                                                   | ENGINE_NAME                                                  |            ROW_COUNT | LAST_CHANGED
--------------------------------------------------------------+--------------------------------------------------------------+----------------------+----------------------------------
 T                                                            | rowstore                                                     |                    8 | 2026-11-20 12:00:00.000000+00:00
(1 row)
 INDEX_NAME                                                   | METHOD_NAME                                                  |             SEARCHES |         ENTRIES_READ |          ENTRY_COUNT |               HEIGHT
--------------------------------------------------------------+--------------------------------------------------------------+----------------------+----------------------+----------------------+----------------------
 T_K                                                          | btree                                                        |                    0 |                    0 |                    8 |                    1
(1 row)
```

Eight rows are read faster by a scan than through T_K, so the query did
not search it, and its SEARCHES stays 0.

What the planner's statistics hold of each column, its rows, share
NULL, distinct values, least and greatest values, most common values and
histogram, is in COLUMN_STATISTICS
([Column statistics](../sql/07-information-schema.md#column-statistics)).
A user sees there the columns on which they hold a privilege.
