# ROLLBACK

[Statements](README.md)

## NAME

ROLLBACK: end a transaction, undoing its changes

## SYNOPSIS

```
<rollback statement> ::= ROLLBACK [ WORK ]
```

## DESCRIPTION

ROLLBACK ends the session's transaction and cancels each change it made
to data and to definitions.

ROLLBACK with no transaction active does nothing, and completes.

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

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2D000 | -917 | It runs in a handler's action while an ATOMIC compound statement runs |
| TB914 | -914 | A change could not be undone. The transaction still ends; each table concerned is damaged, and refuses every later statement but DROP TABLE until it is repaired |

## EXAMPLES

```sql
DELETE FROM ACCOUNTS;
ROLLBACK;
SELECT COUNT(*) FROM ACCOUNTS;
```

The rows deleted are back:

```
OK
OK

----------------------
                    1
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

## SEE ALSO

[BEGIN](begin.md), [COMMIT](commit.md), [SET TRANSACTION](set-transaction.md),
[Transactions](../../user/06-transactions.md)
