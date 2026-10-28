# COMMIT

[Statements](README.md)

## NAME

COMMIT: end a transaction, keeping its changes

## SYNOPSIS

```
<commit statement> ::= COMMIT [ WORK ]
```

## DESCRIPTION

COMMIT ends the session's transaction. Unless something prevents it, the
transaction's changes to data and to definitions become visible to every
transaction that begins after it, and survive a crash of a database kept
in a directory.

COMMIT with no transaction active does nothing, and completes.

## RULES

As COMMIT runs:

1. Each cursor the session has open is closed.
2. Each instance of a temporary table defined ON COMMIT DELETE ROWS is
   emptied, so no row of one breaks a constraint.
3. Each constraint whose checking was deferred, by DEFERRABLE INITIALLY
   DEFERRED or SET CONSTRAINTS, is checked, as if SET CONSTRAINTS ALL
   IMMEDIATE ran.
4. If a constraint is not satisfied, the transaction is rolled back
   instead: none of its changes is kept.
5. If anything else prevents the commit, the transaction is likewise
   rolled back.

The transaction's locks are released once it ends, and the next statement
that needs a transaction begins a new one.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 40002 | -913 | A deferred constraint is not satisfied: the transaction was rolled back |
| 40001 | -911 | It conflicts with a transaction that committed first: it was rolled back, and may be run again |
| 40000 | -913 | An engine refused to commit, an earlier statement of the transaction could not be undone, or the commit record could not be written to the log: the transaction was rolled back |
| TB915 | -915 | The transaction committed, but its commit record could not be synced to disk, so whether it survives a crash is not known |

## EXAMPLES

A constraint checked at commit:

```sql
CREATE TABLE ACCOUNTS (
    ID INTEGER,
    BALANCE DECIMAL(9,2) CONSTRAINT NOT_NEGATIVE CHECK (BALANCE >= 0)
        DEFERRABLE INITIALLY DEFERRED);
COMMIT;
INSERT INTO ACCOUNTS VALUES (1, 100.00);
COMMIT WORK;
```

Each statement completes:

```
OK
OK
OK
OK
```

A change that breaks the constraint is accepted when it is made, and
refused at COMMIT, the transaction rolled back:

```sql
UPDATE ACCOUNTS SET BALANCE = BALANCE - 150 WHERE ID = 1;
COMMIT;
SELECT ID, BALANCE FROM ACCOUNTS;
```

```
OK
turborel: SQLCODE -913 (SQLSTATE 40002): the transaction is rolled back: CHECK ANN.NOT_NEGATIVE of ANN.ACCOUNTS is false for a row [ISO/IEC 9075:1992 §11.6 GR3] [ISO/IEC 9075:1992 §14.3 GR5a]
           ID |     BALANCE
--------------+-------------
            1 |      100.00
(1 row)
```

## STANDARD

ISO/IEC 9075:1992 §14.3, and X3.135-1986 §8.2, which requires WORK.
WORK may be left out, as SQL-92 allows. Cursors are closed [§14.3 GR2],
temporary tables of ON COMMIT DELETE ROWS emptied [GR3], and then
deferred constraints checked [GR4, GR5a]. Of the other failures that roll the
transaction back [GR5b], SQLSTATE 40001 is the standard's serialization
failure, and 40000, without a subclass, is the class alone. TB915 is this
implementation's own: the transaction did commit.

## SEE ALSO

[ROLLBACK](rollback.md), [SET CONSTRAINTS](set-constraints.md),
[SET TRANSACTION](set-transaction.md),
[Transactions](../../user/06-transactions.md)
