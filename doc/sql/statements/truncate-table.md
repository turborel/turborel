# TRUNCATE TABLE

[Statements](README.md)

## NAME

TRUNCATE TABLE: delete every row of a table at once

## SYNOPSIS

```
<truncate table statement> ::=
    TRUNCATE TABLE <table name> [ <identity column restart option> ]

<identity column restart option> ::=
    CONTINUE IDENTITY
  | RESTART IDENTITY
```

## DESCRIPTION

TRUNCATE TABLE removes every row of a base table. A persistent table's
rows go at once. The table is given new, empty storage, as ALTER TABLE
gives it, so the statement costs the same however many rows it held, and
its indexes are emptied with it. A temporary table loses the session's
own rows, and other sessions keep theirs. A TRUNCATE TABLE that finds no
row completes with no data.

CONTINUE IDENTITY, the default, leaves the table's identity column to go
on from the last value it gave. RESTART IDENTITY restarts it at its start
value. A rollback undoes both the rows and the restart.

The transaction that truncates a table holds it to its end, so no other
transaction writes it meanwhile. A transaction that began before the
truncation committed is refused at its next use of the table, and may be
run again. If another transaction committed a row to the table after this
one's snapshot, this one's COMMIT fails, so no row goes unseen.

## RULES

1. The table is a base table of a schema. It is not a view, nor a table
   declared with DECLARE LOCAL TEMPORARY TABLE.
2. No foreign key references the table, its own included. DELETE, which
   runs the referential actions, deletes the rows of such a table.
3. The transaction is not READ ONLY, but for a temporary table.
4. No cursor of the transaction reads the table.
5. Each assertion, and each CHECK with a subquery, that reads the table
   holds once its rows are gone.
6. With RESTART IDENTITY, the identity column's start value is within its
   generator's bounds.

## PRIVILEGES

Ownership of the table's schema. DELETE on the table is not enough.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The table does not exist, is a view or a declared local temporary table, or a foreign key references it; the start value of RESTART IDENTITY is beyond its generator's bounds |
| 2A000 | -104 | Its user does not own the table's schema |
| 23000 | -201 | An assertion, or a CHECK with a subquery, that reads the table no longer holds |
| 36002 | -501 | A cursor of the transaction reads the table |
| TB911 | -911 | Another transaction holds the table, under LOCKS.WAIT OFF, or a wait for it would deadlock or passed LOCKS.TIMEOUT_MS; or the transaction began before a truncation of the table committed |
| 25006 | -917 | The transaction is READ ONLY |
| 02000 | 100 | Not an error: the table had no row |

A statement that fails deletes no row.

## EXAMPLES

```sql
CREATE TABLE TICKETS (ID INTEGER GENERATED ALWAYS AS IDENTITY, NAME VARCHAR(20));
INSERT INTO TICKETS (NAME) VALUES ('first'), ('second');
TRUNCATE TABLE TICKETS;
INSERT INTO TICKETS (NAME) VALUES ('third');
SELECT ID, NAME FROM TICKETS;
TRUNCATE TABLE TICKETS RESTART IDENTITY;
INSERT INTO TICKETS (NAME) VALUES ('fourth');
SELECT ID, NAME FROM TICKETS;
TRUNCATE TABLE TICKETS;
TRUNCATE TABLE TICKETS;
```

```
OK
OK
OK
OK
           ID | NAME
--------------+----------------------
            3 | third
(1 row)
OK
OK
           ID | NAME
--------------+----------------------
            1 | fourth
(1 row)
OK
no data
```

```sql
CREATE TABLE ORDERS (ID INTEGER NOT NULL PRIMARY KEY);
CREATE TABLE ITEMS (ORDER_ID INTEGER REFERENCES ORDERS);
TRUNCATE TABLE ORDERS;
```

```
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): foreign key ANN.ITEMS_FOREIGN1 of ANN.ITEMS references ANN.ORDERS, so its rows are not truncated [ISO/IEC 9075-2:2023 §14.10 SR4] (at character 15)
```

## STANDARD

ISO/IEC 9075-2:2023 §14.10, the `<truncate table statement>`, Features
F200 and F202. How a persistent table's rows go, and what that refuses
another transaction, are this implementation's own (RULES.md D64), as is
TB911.

## SEE ALSO

[DELETE](delete.md), [ALTER TABLE](alter-table.md),
[CREATE TABLE](create-table.md), [DROP TABLE](drop-table.md)
