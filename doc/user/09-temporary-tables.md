# Temporary Tables

[Contents](README.md)

## Global and created local temporary tables

A temporary table is defined as any table is, but each session has rows
of its own of it: no other session sees them, they are kept in memory,
of the database's MEMORY_BUDGET, and they are gone when the session
ends. CREATE GLOBAL TEMPORARY TABLE
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
temporary table, the column added taking its default in each, and the
table's constraints are checked of each session's rows. Until the
transaction altering the table ends, other sessions neither read nor
write their rows of it; and it waits for none: it is refused (SQLSTATE
TB911) while another session's transaction has its rows of the table
open, to be run again once that one ends. Another change, of a
constraint or a default, is made only while no session holds a row of
the table ([ALTER TABLE](../sql/statements/alter-table.md)).

CREATE INDEX on a temporary table gives each session an index of its own
rows, made as its next statement begins; a session whose transaction
had its rows open uses it once that transaction ends
([CREATE INDEX](../sql/statements/create-index.md)).
