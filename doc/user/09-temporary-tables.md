# Temporary Tables

[Contents](README.md)

## Global and created local temporary tables

A temporary table is defined as any table is, but each session has rows
of its own of it: no other session sees them, and they are gone when the
session ends. CREATE GLOBAL TEMPORARY TABLE
or CREATE LOCAL TEMPORARY TABLE defines one, in a schema, for every
session ([CREATE TABLE](../sql/statements/create-table.md)).

## Declared local temporary tables

A session may declare a table of its own, named MODULE.T, in no schema,
gone when the session ends
([DECLARE LOCAL TEMPORARY TABLE](../sql/statements/declare-local-temporary-table.md)).

## ON COMMIT

ON COMMIT DELETE ROWS, the default, empties a session's rows as each
transaction ends; ON COMMIT PRESERVE ROWS keeps them until the session
ends.

```sql
CREATE GLOBAL TEMPORARY TABLE CART (ITEM VARCHAR(20), QTY INTEGER);
CREATE GLOBAL TEMPORARY TABLE NOTES (TEXT VARCHAR(40)) ON COMMIT PRESERVE ROWS;
COMMIT;
INSERT INTO CART VALUES ('bolt', 10);
INSERT INTO NOTES VALUES ('kept');
SELECT COUNT(*) FROM CART;
COMMIT;
SELECT COUNT(*) FROM CART;
SELECT TEXT FROM NOTES;
```

```
OK
OK
OK
OK
OK

----------------------
                    1
(1 row)
OK

----------------------
                    0
(1 row)
 TEXT
------------------------------------------
 kept
(1 row)
```

Another session, at once, finds NOTES empty: its rows are its own.

## Changing a temporary table

ALTER TABLE ADD COLUMN and DROP COLUMN change every session's rows of a
temporary table, the column added taking its default in each. Another
change, of a constraint or a default, changes no row, and is every
session's at once. The table's constraints, one added among them, are
checked of each session's rows, as that session sees the other
temporary tables a constraint reads: its own rows of them. Until the
transaction altering the table ends, other sessions neither read nor
write their rows of it; and it waits for none: it is refused (SQLSTATE
TB911) while another session's transaction has its rows of the table
open, or if one committed rows of it after the altering transaction
began, to be run again once that one ends, or in a new transaction
([ALTER TABLE](../sql/statements/alter-table.md)).

CREATE INDEX on a temporary table gives each session an index of its own
rows, made as its next statement begins; a session whose transaction
had its rows open uses it once that transaction ends
([CREATE INDEX](../sql/statements/create-index.md)).

## Where the rows are kept

A session's rows of a temporary table, with its indexes' entries and
the earlier versions of the committed rows a transaction updates or
deletes, kept until it ends, are read and written through a buffer pool of the session's
MEMORY.WORK_BYTES, reserved of the database's MEMORY.BUDGET_BYTES as a
transaction of the session first reads or writes the table. What the
pool does not hold is written to a spill file in the database's `tmp/`
directory, so a session's rows may be many times the budget. The file
is encrypted while the database encrypts anything, as other spill files
are. It is removed as ON COMMIT DELETE ROWS empties the rows and when
the session ends; one a crash leaves is removed as the database opens.
No change to the rows is logged, and none is recovered. A database
kept in memory keeps the file in memory too, of its budget.
