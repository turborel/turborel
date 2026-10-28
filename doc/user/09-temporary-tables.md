# Temporary Tables

[Contents](README.md)

## Global and created local temporary tables

A temporary table is defined as any table is, but each session has rows
of its own of it: no other session sees them, they are kept in memory,
and they are gone when the session ends. CREATE GLOBAL TEMPORARY TABLE
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
