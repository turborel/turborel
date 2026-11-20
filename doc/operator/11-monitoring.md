# Monitoring *(extension)*

[Contents](README.md)

A database shows what its sessions do now and what its statements have
done, in views of the Information Schema. A server's connections are in
CONNECTIONS, and the statements run since the database was opened are in
STATEMENT_STATISTICS. Both are this implementation's own
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
texts shown as NULL.

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
