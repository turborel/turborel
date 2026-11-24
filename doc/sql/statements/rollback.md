# ROLLBACK

[Statements](README.md)

## NAME

ROLLBACK: end a transaction, undoing its changes, or undo them back to a savepoint

## SYNOPSIS

```
<rollback statement> ::=
    ROLLBACK [ WORK ] [ AND NO CHAIN ] [ TO SAVEPOINT <savepoint name> ]
```

## DESCRIPTION

ROLLBACK ends the session's transaction and cancels each change it made
to data and to definitions.

ROLLBACK with no transaction active does nothing, and completes.

ROLLBACK TO SAVEPOINT cancels only the changes made since the savepoint
was established by [SAVEPOINT](savepoint.md). The savepoint stays, and
the transaction goes on.

AND NO CHAIN changes nothing, as it is what a ROLLBACK without it does.
AND CHAIN is not supported.

## RULES

As ROLLBACK runs:

1. Each cursor the session has open is closed.
2. Each change the transaction made is undone.
3. Each instance of a temporary table defined ON COMMIT DELETE ROWS is
   emptied.

The transaction ends even if a change cannot be undone. The transaction's
locks are released once it ends, and the next statement that needs a
transaction begins a new one.

ROLLBACK may be a statement of a compound statement, or of another SQL
control statement, NOT ATOMIC and run directly. It ends the
transaction there, and the statements after it run in a new one (see
[BEGIN](begin.md)). It may not be in an ATOMIC compound statement, a
FOR, or the body of a routine or trigger.

As ROLLBACK TO SAVEPOINT runs:

1. Each change to data and to definitions the transaction made since the
   savepoint is undone, in tables, temporary tables, indexes and large
   objects.
2. Each savepoint established after it is destroyed.
3. Each large object locator given after it is invalid; those given
   before stay valid.
4. Each cursor the session has open is closed, one opened before the
   savepoint too.

The locks the transaction took since the savepoint stay held until it
ends. No prepared statement is deallocated, and the diagnostics area
tells of the ROLLBACK as of any statement.

The savepoint must be one of the current savepoint level (see
[SAVEPOINT](savepoint.md)). In a function, a trigger's action or an
ATOMIC compound statement, it must have been established after that
began. ROLLBACK TO SAVEPOINT may be a statement of the body of a
procedure or function and of an ATOMIC compound statement, but not of a
FOR or a trigger's action. In a routine's body it is refused if it would
cancel a change of definitions made before the statement that invoked
the routine began, since that statement runs on those definitions.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2D000 | -917 | It runs in a handler's action while an ATOMIC compound statement runs, without TO SAVEPOINT |
| 3B001 | -917 | No savepoint of that name is established at the current savepoint level, or it was established before the function, trigger's action or ATOMIC compound statement running began |
| 42000 | -103 | AND CHAIN with TO SAVEPOINT; or TO SAVEPOINT in a FOR or a trigger's action |
| 0A000 | -900 | AND CHAIN without TO SAVEPOINT; or TO SAVEPOINT in a routine's body would cancel a change of definitions the statement that invoked it runs on |
| 09000 | -917 | TO SAVEPOINT runs in a procedure a trigger's action calls (SQLSTATE 0W000) |
| TB914 | -914 | A change could not be undone. The transaction still ends; each table concerned is damaged, and refuses every later statement but DROP TABLE until it is repaired |

## EXAMPLES

The rows deleted are back:

```sql
DELETE FROM ACCOUNTS;
ROLLBACK;
SELECT COUNT(*) FROM ACCOUNTS;
```

```
OK
OK

----------------------
                    1
(1 row)
```

Going back to a savepoint, the transaction going on:

```sql
SAVEPOINT BEFORE_RAISE;
UPDATE ACCOUNTS SET BALANCE = BALANCE * 2;
ROLLBACK WORK TO SAVEPOINT BEFORE_RAISE;
UPDATE ACCOUNTS SET BALANCE = BALANCE + 1;
COMMIT;
SELECT BALANCE FROM ACCOUNTS;
```

```
OK
OK
OK
OK
OK
     BALANCE
-------------
      101.00
(1 row)
```

## STANDARD

ISO/IEC 9075:1992 §14.4, and X3.135-1986 §8.9, which requires WORK.
WORK may be left out, as SQL-92 allows. Cursors are closed [§14.4 GR2]
and the changes canceled [GR3]. Emptying a temporary table of ON COMMIT
DELETE ROWS at ROLLBACK is this implementation's choice: the rows the
transaction wrote are undone, and those before it were emptied when the
transaction before ended. TB914 is this implementation's own.

ISO/IEC 9075-4:2006 draft §4.10.4 makes a compound statement NOT
ATOMIC a non-atomic execution context, where ROLLBACK may run, and
ISO/IEC 9075-2:2023 §17.8 GR2 raises 2D000 in an atomic one.

ROLLBACK TO SAVEPOINT is ISO/IEC 9075-2:2023 §17.8 GR3, Feature T271,
at the savepoint levels of Feature T272. AND CHAIN with it is a syntax
error [SR1], and AND CHAIN alone needs Feature T261, which is not
supported. A savepoint not of the current level raises 3B001 [GR3b], as
does one established before the most recent atomic execution context
began [GR3c]. Changes are cancelled [GR3d], later savepoints destroyed
[GR3e], later locators made invalid [GR3f] and cursors closed [GR3g].
Every cursor opened before the savepoint is closed too (IA168). No
prepared statement is deallocated (IA167). The diagnostics area and the
descriptor areas change as after any statement, and no other way
(IA211). ISO/IEC 9075-4:2006 draft §14.13 SR9 keeps ROLLBACK out of a
FOR.

## SEE ALSO

[BEGIN](begin.md), [COMMIT](commit.md), [SAVEPOINT](savepoint.md),
[RELEASE SAVEPOINT](release-savepoint.md), [SET TRANSACTION](set-transaction.md),
[Transactions](../../user/06-transactions.md)
