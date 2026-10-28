# Changing Data

[Contents](README.md)

## Inserting rows

INSERT adds rows, of values for the columns named, the others their
defaults, or null:

```sql
INSERT INTO EMPLOYEES (ID, NAME, DEPT) VALUES (4, 'Barbara', 'OPS');
```

A row a constraint refuses is not added: the statement is undone whole,
and the transaction goes on ([INSERT](../sql/statements/insert.md)).

## Updating rows

UPDATE sets columns of the rows its WHERE clause keeps; every row,
without one:

```sql
UPDATE EMPLOYEES SET SALARY = SALARY * 1.05 WHERE DEPT = 'ENG';
SELECT ID, NAME, SALARY FROM EMPLOYEES ORDER BY ID;
```

```
OK
OK
           ID | NAME                           |      SALARY
--------------+--------------------------------+-------------
            1 | Ada                            |     7560.00
            2 | Grace                          |     8505.00
            3 | Edsger                         |     5400.00
            4 | Barbara                        |        NULL
(4 rows)
```

Barbara, given no salary, has the null value.

## Deleting rows

DELETE removes the rows its WHERE clause keeps; every row, without one.
An UPDATE or DELETE of no row completes with no data:

```sql
DELETE FROM EMPLOYEES WHERE SALARY IS NULL;
UPDATE EMPLOYEES SET SALARY = 1 WHERE ID = 99;
ROLLBACK;
```

```
OK
no data
OK
```

ROLLBACK undoes the transaction's changes: the insert, the raise and the
delete.

## Cursors

A program that embeds turborel may read a query's rows one at a time,
by a cursor, and change the row it is on
([DECLARE CURSOR](../sql/statements/declare-cursor.md)). The client
reads a query's rows whole, as they come.
