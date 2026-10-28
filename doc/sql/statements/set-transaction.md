# SET TRANSACTION

[Statements](README.md)

## NAME

SET TRANSACTION: set the next transaction's modes

## SYNOPSIS

```
<set transaction statement> ::=
    SET TRANSACTION <transaction mode> [ { , <transaction mode> }... ]

<transaction mode> ::=
      <isolation level>
    | <transaction access mode>
    | <diagnostics size>

<isolation level> ::= ISOLATION LEVEL <level of isolation>

<level of isolation> ::=
      READ UNCOMMITTED
    | READ COMMITTED
    | REPEATABLE READ
    | SERIALIZABLE

<transaction access mode> ::= READ ONLY | READ WRITE

<diagnostics size> ::= DIAGNOSTICS SIZE <number of conditions>

<number of conditions> ::= <simple value specification>
```

## DESCRIPTION

SET TRANSACTION sets the modes of the session's next transaction: how it
reads, whether it may change data, and how many conditions its
diagnostics area holds. It takes effect when the next transaction
begins, and lasts for that transaction alone: the one after takes the
defaults again, unless SET TRANSACTION is run again. It begins no
transaction itself.

A mode not written takes its default:

| Mode | Default |
|---|---|
| Isolation level | SERIALIZABLE |
| Access mode | READ ONLY under READ UNCOMMITTED; READ WRITE otherwise |

How each level reads:

| Level | Reads |
|---|---|
| SERIALIZABLE | At one snapshot of the database, taken as the transaction begins; what it read is checked again as it commits, so its outcome is that of transactions run one at a time |
| REPEATABLE READ | At one snapshot, taken as the transaction begins |
| READ COMMITTED | At a new snapshot taken as each statement begins, while the session has no cursor open; with one open, at the snapshot it has |
| READ UNCOMMITTED | As READ COMMITTED |

Below SERIALIZABLE, what a transaction read is not checked as it commits,
so two transactions may each commit what the other's reads would have
forbidden (write skew).

At every level, a transaction reads no change another has not committed,
and loses no update: a statement that writes a row another transaction
wrote and has not committed, or committed after this one's snapshot, is
refused (SQLSTATE TB911) and undone, and may be run again. Constraints
are checked, and referential actions run, as at SERIALIZABLE at every
level. A table of an engine that does not isolate transactions itself is
read as at SERIALIZABLE at every level; the built-in row store does.

A READ ONLY transaction changes no data, but that of a temporary table,
and no definition.

## RULES

1. No mode is written more than once.
2. READ WRITE is not written with READ UNCOMMITTED.
3. The `<number of conditions>` is an exact number of scale 0, and at
   least 1. A statement records one condition, its outcome, so any such
   size holds it.
4. SET TRANSACTION is not run while a transaction is active.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A mode is written more than once, READ WRITE is written with READ UNCOMMITTED, or the `<number of conditions>` is not an exact number of scale 0 |
| 35000 | -301 | The `<number of conditions>` is less than 1 |
| 25000 | -917 | A transaction is active |

In a READ ONLY transaction, a statement that would change data or a
definition fails with SQLSTATE 25000, SQLCODE -917.

## EXAMPLES

A transaction that reads, at one snapshot, and may not write:

```sql
SET TRANSACTION READ ONLY, ISOLATION LEVEL REPEATABLE READ;
SELECT COUNT(*) FROM ACCOUNTS;
UPDATE ACCOUNTS SET BALANCE = 0;
COMMIT;
```

```
OK

----------------------
                    1
(1 row)
turborel: SQLCODE -917 (SQLSTATE 25000): the transaction is READ ONLY [ISO/IEC 9075:1992 §13.10 GR1]
OK
```

The UPDATE that failed did not end the transaction, and the next
transaction takes the defaults again, SERIALIZABLE and READ WRITE.

SET TRANSACTION while a transaction is active is refused:

```sql
SELECT COUNT(*) FROM ACCOUNTS;
SET TRANSACTION READ ONLY;
```

```

----------------------
                    1
(1 row)
turborel: SQLCODE -917 (SQLSTATE 25000): SET TRANSACTION while a transaction is active [ISO/IEC 9075:1992 §14.1 GR1]
```

## STANDARD

ISO/IEC 9075:1992 §14.1. The defaults are those of SR2 and SR4. The
isolation levels are as §4.28 requires: none shows a phenomenon its
level excludes. REPEATABLE READ as snapshot isolation, and READ COMMITTED
and READ UNCOMMITTED as a snapshot for each statement, are this
implementation's choices [§14.1 GR5]; so is a diagnostics area of one
condition [GR6, GR7]. Writing a temporary table in a READ ONLY
transaction is allowed by §13.6 to §13.10 GR1. SQL-86 has no SET
TRANSACTION.

## SEE ALSO

[COMMIT](commit.md), [ROLLBACK](rollback.md),
[GET DIAGNOSTICS](get-diagnostics.md),
[Transactions](../../user/06-transactions.md)
