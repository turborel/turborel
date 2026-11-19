# LOCK TABLE *(extension)*

[Statements](README.md)

## NAME

LOCK TABLE: lock tables to the end of the transaction

## SYNOPSIS

```
<lock table statement> ::=
    LOCK [ TABLE ] [ ONLY ] <table name> [ { , [ ONLY ] <table name> }... ]
        [ IN <lock mode> MODE ] [ NOWAIT ]

<lock mode> ::=
    ACCESS SHARE
  | ROW SHARE
  | ROW EXCLUSIVE
  | SHARE UPDATE EXCLUSIVE
  | SHARE
  | SHARE ROW EXCLUSIVE
  | EXCLUSIVE
  | ACCESS EXCLUSIVE
```

## DESCRIPTION

LOCK TABLE takes each table's lock for the transaction, which holds it
until it commits or rolls back. No statement unlocks it sooner. The lock
is the one the transaction's own statements take as they read and write
the table, so it only makes sure of now what they would wait for later.

The eight modes are taken as the three strengths of a table's lock:

| Modes | Strength | Stands against |
|---|---|---|
| ACCESS SHARE, ROW SHARE, SHARE | Shared | Exclusive |
| ROW EXCLUSIVE, SHARE UPDATE EXCLUSIVE | Update | Update and Exclusive |
| SHARE ROW EXCLUSIVE, EXCLUSIVE, ACCESS EXCLUSIVE | Exclusive | Every other |

A table locked shared is read by others and written by none. One locked
for update is read by others and written only by its holder. One locked
exclusive is held by its holder alone. With no mode, the lock is ACCESS
EXCLUSIVE.

A lock held by another transaction is waited for as any lock is. While
LOCKS.WAIT is on, the statement waits until the lock is free,
LOCKS.TIMEOUT_MS passes or the statement is cancelled. While it is off,
the statement is refused at once. A wait that would close a cycle of waits is a
deadlock, and is refused at once. With NOWAIT the statement does not
wait, and fails with SQLSTATE TBL01 if a lock is held against it.

A typed table is locked with its subtables, unless ONLY is written. The
tables are locked in the order written, so two transactions that lock
the same tables in the same order never deadlock on them.

Its command code, as GET DIAGNOSTICS gives it, is -42.

## RULES

1. Each table is a base table. A view, or a table declared with DECLARE
   LOCAL TEMPORARY TABLE, is refused.
2. The statement begins a transaction if none is active.

## PRIVILEGES

SELECT on each table for ACCESS SHARE, ROW SHARE and SHARE; DELETE on
each table for any stronger mode.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A table does not exist, or is not a base table |
| 2A000 | -104 | Its user lacks the privilege the mode needs |
| TBL01 | -911 | NOWAIT, and another transaction holds a lock against it |
| TB911 | -911 | Another transaction holds a lock against it, under LOCKS.WAIT OFF, or a wait for it would deadlock or passed LOCKS.TIMEOUT_MS |
| TB920 | -920 | The statement was cancelled as it waited |

The tables written before the one that failed stay locked, and the
transaction goes on.

## EXAMPLES

```sql
CREATE TABLE ACCOUNTS (ID INTEGER PRIMARY KEY, BALANCE DECIMAL(9,2));
INSERT INTO ACCOUNTS VALUES (1, 100.00);
COMMIT;
LOCK TABLE ACCOUNTS IN SHARE MODE;
SELECT BALANCE FROM ACCOUNTS;
LOCK ACCOUNTS NOWAIT;
COMMIT;
LOCK TABLE MISSING;
```

```
OK
OK
OK
OK
     BALANCE
-------------
      100.00
(1 row)
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): LOCK TABLE names ANN.MISSING, which is not a base table (RULES.md D186) (at character 11)
```

## STANDARD

LOCK TABLE is an extension; no part of ISO/IEC 9075 defines it. How its
modes are taken as a table's locks, and SQLSTATE TBL01, are this
implementation's own (RULES.md D186).

## SEE ALSO

[Locks](../23-locks.md), [SET TRANSACTION](set-transaction.md),
[COMMIT](commit.md), [ROLLBACK](rollback.md)
