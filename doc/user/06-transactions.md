# Transactions

[Contents](README.md)

## When a transaction begins and ends

A transaction is the statements whose changes are made together, or not
at all. The first statement that reads or changes data begins one;
[COMMIT](../sql/statements/commit.md) ends it, its changes made lasting
and seen by all, and [ROLLBACK](../sql/statements/rollback.md) ends it,
its changes undone. A statement that fails is undone alone; the
transaction goes on. A COMMIT once done survives a crash, unless the
session set LOG.COMMIT_SYNC OFF, so as not to wait for the log: then
only once the log is next synced, within LOG.SYNC_AFTER_MS
([Settings](../operator/04-settings.md#a-sessions-own)); or the
database's LOG.DURABILITY is NONE: then a failure of the system may
lose it ([Databases](../operator/03-databases.md#recovery-after-a-crash)).

Definitions, as CREATE SCHEMA and CREATE TABLE, run within the
transaction too, and are undone by ROLLBACK; the statements of users and
indexes are made at once.

## Isolation levels

A transaction is READ COMMITTED unless [SET
TRANSACTION](../sql/statements/set-transaction.md) or the setting
TRANSACTION.ISOLATION says otherwise. Each of its statements reads a
snapshot, the database as of when that statement began, so readers never
wait for writers, nor writers for readers.

SERIALIZABLE reads and writes as if no other transaction ran at the same
time. It reads one snapshot, taken as it began, and checks that snapshot
again as it commits. It is the level the SQL standard makes the default,
and `SET DATABASE TRANSACTION.ISOLATION = 'SERIALIZABLE'` makes it this
database's. REPEATABLE READ reads one snapshot and checks nothing as it
commits, and READ UNCOMMITTED reads as READ COMMITTED does.

## Conflicts

Two transactions that would change one row are not both let at once. On
a server the second waits for the first to end. If the first rolled back,
the second changes the row as it found it. If the first committed, at READ
COMMITTED the second checks its WHERE clause against the row the first
left, and changes that row, its new values computed from it, or leaves it.
At REPEATABLE READ and SERIALIZABLE it is refused instead, SQLSTATE TB911,
and the statement may run again in a new transaction.

```
-- in one session, SALARY being 1000:
UPDATE EMPLOYEES SET SALARY = SALARY + 100 WHERE ID = 1;
-- in another, before the first commits; it waits:
UPDATE EMPLOYEES SET SALARY = SALARY + 1 WHERE ID = 1;
-- in the first:
COMMIT;
-- in the second, its UPDATE done:
COMMIT;
SELECT SALARY FROM EMPLOYEES WHERE ID = 1;
```

```
       SALARY
--------------
         1101
(1 row)
```

`SET LOCKS.WAIT = 'OFF'` refuses the second at once instead
([SET](../sql/statements/set-setting.md)), as a database no server serves
does by default, since a program driving its sessions from one thread
could never let the first end:

```
turborel: SQLCODE -911 (SQLSTATE TB911): a row it would change was changed by a transaction this one does not see, or is being changed; run the transaction again
```

Two transactions each waiting for a row the other changed would wait
forever: the second to wait is refused at once, SQLSTATE TB911, only its
statement undone. LOCKS.TIMEOUT_MS bounds any wait. A positioned UPDATE or
DELETE, and a referential action, waits too, but is refused if the first
committed. A commit that could not keep the transactions serializable is
refused, SQLSTATE 40001, the transaction rolled back, to be run again.

## Read-only transactions

`SET TRANSACTION READ ONLY` makes the next transaction read only: a
change, or a definition, is refused.

```sql
SET TRANSACTION READ ONLY;
UPDATE EMPLOYEES SET SALARY = 1 WHERE ID = 2;
```

```
OK
turborel: SQLCODE -917 (SQLSTATE 25000): the transaction is READ ONLY [ISO/IEC 9075:1992 §13.10 GR1]
```
