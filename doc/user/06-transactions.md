# Transactions

[Contents](README.md)

## When a transaction begins and ends

A transaction is the statements whose changes are made together, or not
at all. The first statement that reads or changes data begins one;
[COMMIT](../sql/statements/commit.md) ends it, its changes made lasting
and seen by all, and [ROLLBACK](../sql/statements/rollback.md) ends it,
its changes undone. A statement that fails is undone alone; the
transaction goes on. A COMMIT once done survives a crash.

Definitions, as CREATE SCHEMA and CREATE TABLE, run within the
transaction too, and are undone by ROLLBACK; the statements of users and
indexes are made at once.

## Isolation levels

A transaction is SERIALIZABLE unless [SET
TRANSACTION](../sql/statements/set-transaction.md) says otherwise: its
reads and writes are as if no other transaction ran at the same time.
It reads a snapshot, the database as of when it began, so readers never
wait for writers, nor writers for readers. REPEATABLE READ, READ
COMMITTED and READ UNCOMMITTED read with less isolation, a snapshot of
each statement at the last two.

## Conflicts

Two transactions that would change one row are not both let: the second
is refused, not made to wait, and may run again once the first ends.

```
-- in one session:
UPDATE EMPLOYEES SET SALARY = SALARY + 100 WHERE ID = 1;
-- in another, before the first commits:
UPDATE EMPLOYEES SET SALARY = 1 WHERE ID = 1;
```

```
turborel: SQLCODE -911 (SQLSTATE TB911): a row it would change was changed by a transaction this one does not see, or is being changed; run the transaction again
```

`SET LOCK_WAIT = 'WAIT'` makes a statement wait for a lock another
transaction holds, rather than be refused at once
([SET](../sql/statements/set-setting.md)). A commit that could not keep
the transactions serializable is refused, SQLSTATE 40001, the
transaction rolled back, to be run again.

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
