# Locks *(extension)*

[Contents](README.md)

A transaction reads at its snapshot, and two transactions that write one
row are told apart when the second writes it. Some work needs more, to
read a row and be sure no one else changes it before the transaction
ends, to share out rows among workers, or to agree on a name that only
one session at a time may act on. Turborel gives three kinds of lock for
these, each held to the end of a transaction or of a session:

- row locks, which a query takes on the rows it gives, with FOR UPDATE
  or FOR SHARE;
- table locks, which [LOCK TABLE](statements/lock-table.md) takes;
- advisory locks, which mean what the application makes them mean.

A wait for any of them goes through one graph of waits, so a cycle among
them is a deadlock, found at once. While LOCKS.WAIT is on, a statement
waits for a lock until it is free, until LOCKS.TIMEOUT_MS passes, or
until it is cancelled. While it is off, the default, a lock held against
the statement is refused at once, SQLCODE -911.

The examples on this page are run in order, by the user ANN, on a queue
of jobs:

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE JOBS (ID INTEGER PRIMARY KEY, DONE BOOLEAN);
INSERT INTO JOBS VALUES (1, FALSE), (2, FALSE), (3, FALSE);
COMMIT;
```

```
OK
OK
OK
OK
```

## Row locks

```
<locking clause> ::=
    FOR { UPDATE | SHARE } [ OF <table name> [ { , <table name> }... ] ]
        [ NOWAIT | SKIP LOCKED ]
```

A locking clause ends the query a cursor reads, directly, prepared or
declared FOR UPDATE, after its ORDER BY and its row limit. Each row the
query gives is locked as it is read, and stays locked until the
transaction ends. FOR UPDATE locks a row exclusive. FOR SHARE locks it
shared, so other transactions may lock it FOR SHARE too, but none FOR
UPDATE.

A row locked by another transaction is written by this one only once
that one ends. An UPDATE or DELETE of it waits, as a FOR UPDATE does. A
plain query reads it as ever, at its snapshot, and never waits.

Rows are locked after the WHERE clause chooses them, and before the row
limit counts them. So a query with FETCH FIRST locks only the rows it
gives.

NOWAIT refuses the statement at once, with SQLSTATE TBL01, if a row is
locked against it. SKIP LOCKED passes over such a row, and over a row
another transaction changed since the snapshot, and gives the rest. A
queue of jobs read with SKIP LOCKED and FETCH FIRST 1 ROW ONLY gives each
worker a job of its own, and no worker waits for another.

OF names the tables of the query whose rows are locked, by their names or
correlation names. A query reads one table, so OF can name only it.

```sql
SELECT ID FROM JOBS WHERE NOT DONE ORDER BY ID FETCH FIRST 1 ROW ONLY FOR UPDATE SKIP LOCKED;
UPDATE JOBS SET DONE = TRUE WHERE ID = 1;
COMMIT;
SELECT ID FROM JOBS AS J WHERE NOT DONE ORDER BY ID FOR SHARE OF J NOWAIT;
COMMIT;
```

```
           ID
--------------
            1
(1 row)
OK
OK
           ID
--------------
            2
            3
(2 rows)
OK
```

### Row locks and isolation

A row is locked as it is now, not as the snapshot saw it. If another
transaction changed the row since this one's snapshot, then:

- at READ COMMITTED, the query runs again at a new snapshot, and gives
  and locks the rows as they are now;
- at REPEATABLE READ and SERIALIZABLE, the statement is refused,
  SQLCODE -911, SQLSTATE TB911, and may be run again in a new
  transaction.

So a transaction never holds a lock of a row it reads in an older
version.

### What a locking query is

A locking query reads one base table, in its FROM clause, through no
join, grouping, DISTINCT, window or set operation. A view that reads one
base table so may stand for it, and the rows locked are that table's.
A query of another shape is refused, SQLCODE -900. An unnamed cursor
takes FOR UPDATE as a locking clause, as no positioned statement names
it. A declared cursor's FOR UPDATE keeps its own meaning, and a declared
cursor takes no other locking clause.

A table whose engine leaves isolation to Turborel, as a plug-in's may,
has no row locks. Its whole table is locked instead, for update by FOR
UPDATE and shared by FOR SHARE, and SKIP LOCKED passes over nothing.

## Advisory locks

An advisory lock locks a key, a BIGINT, two INTEGERs, or a character
string. Turborel gives it no meaning. Sessions that agree on what a key
stands for use it to take turns. A pair of numbers is a key apart from
every single number.

| Function | Holds | Waits |
|---|---|---|
| ADVISORY_LOCK(key) | the session's, exclusive | yes |
| ADVISORY_LOCK_SHARED(key) | the session's, shared | yes |
| TRY_ADVISORY_LOCK(key) | the session's, exclusive | no |
| TRY_ADVISORY_LOCK_SHARED(key) | the session's, shared | no |
| ADVISORY_XACT_LOCK(key) | the transaction's, exclusive | yes |
| ADVISORY_XACT_LOCK_SHARED(key) | the transaction's, shared | yes |
| TRY_ADVISORY_XACT_LOCK(key) | the transaction's, exclusive | no |
| TRY_ADVISORY_XACT_LOCK_SHARED(key) | the transaction's, shared | no |
| ADVISORY_UNLOCK(key) | unlocks the session's exclusive hold | |
| ADVISORY_UNLOCK_SHARED(key) | unlocks the session's shared hold | |
| ADVISORY_UNLOCK_ALL() | unlocks every hold of the session's | |

Each gives a BOOLEAN. A lock taken gives TRUE. A try that finds the key
held against it gives FALSE, at once. An unlock gives TRUE if it
unlocked a hold of the session's, else FALSE. A null key gives null.

A session's hold lasts until it is unlocked or the session ends, whatever
its transactions do. A transaction's hold lasts until the transaction
ends, and no unlock ends it sooner. A session that takes a key again
holds it again, and unlocks it as many times. Shared holds stand
together, and an exclusive hold stands against any other session's.

Each function is also named with the prefix PG_, as PG_ADVISORY_LOCK, for
programs written to that spelling.

```sql
SELECT * FROM (VALUES (ADVISORY_LOCK(7), TRY_ADVISORY_LOCK('nightly report'))) AS R (TAKEN, TRIED);
SELECT * FROM (VALUES (ADVISORY_UNLOCK(7), ADVISORY_UNLOCK(7))) AS R (FIRST_UNLOCK, SECOND_UNLOCK);
SELECT * FROM (VALUES (ADVISORY_XACT_LOCK(1, 2))) AS R (TAKEN);
COMMIT;
```

```
 TAKEN | TRIED
-------+-------
 TRUE  | TRUE
(1 row)
 FIRST_UNLOCK | SECOND_UNLOCK
--------------+---------------
 TRUE         | FALSE
(1 row)
 TAKEN
-------
 TRUE
(1 row)
OK
```

## Errors

| SQLSTATE | SQLCODE | When |
|---|---|---|
| TBL01 | -911 | NOWAIT, and another transaction holds a lock of a row or table against the statement |
| TB911 | -911 | A lock is held against the statement, under LOCKS.WAIT OFF; a wait would deadlock or passed LOCKS.TIMEOUT_MS; at REPEATABLE READ or SERIALIZABLE, a row locked was changed since the snapshot |
| TB920 | -920 | The statement was cancelled as it waited |
| 2A000 | -103 | OF names a table the query does not read; an advisory function's key is of another type |
| 0A000 | -900 | A locking query of another shape |
| 22003 | -301 | An advisory key out of the range of its type |

## Standard

Row locks, table locks and advisory locks are extensions. ISO/IEC
9075-2:2023 §14.3 gives a `<cursor specification>` an `<updatability
clause>`, FOR UPDATE of a declared cursor, which locks nothing. How each
lock is held and waited for is this implementation's own (RULES.md D185,
D186, D187).

## See also

[LOCK TABLE](statements/lock-table.md),
[SET TRANSACTION](statements/set-transaction.md),
[DECLARE CURSOR](statements/declare-cursor.md),
[Query Expressions](06-queries.md)
