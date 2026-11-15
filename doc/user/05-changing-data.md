# Changing Data

[Contents](README.md)

## Inserting rows

INSERT adds rows, of values for the columns named, the others their
defaults, or null:

```sql
INSERT INTO EMPLOYEES (ID, NAME, DEPT) VALUES (4, 'Barbara', 'OPS');
```

```
OK
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

## Merging rows

MERGE updates the rows a source matches and inserts those it does not,
in one statement. Each source row takes the first WHEN clause that
applies to it, and a WHEN MATCHED clause may delete instead:

```sql
MERGE INTO EMPLOYEES AS E
  USING (VALUES (3, 'Edsger', 'OPS', 5600), (5, 'Frances', 'OPS', 6100)) AS N (ID, NAME, DEPT, SALARY)
  ON E.ID = N.ID
  WHEN MATCHED THEN UPDATE SET SALARY = N.SALARY
  WHEN NOT MATCHED THEN INSERT VALUES (N.ID, N.NAME, N.DEPT, N.SALARY);
SELECT ID, NAME, SALARY FROM EMPLOYEES ORDER BY ID;
ROLLBACK;
```

```
OK
           ID | NAME                           |      SALARY
--------------+--------------------------------+-------------
            1 | Ada                            |     7200.00
            2 | Grace                          |     8100.00
            3 | Edsger                         |     5600.00
            5 | Frances                        |     6100.00
(4 rows)
OK
```

Edsger's salary is updated, and Frances is inserted
([MERGE](../sql/statements/merge.md)).

## Cursors

A program that embeds turborel may read a query's rows one at a time,
by a cursor it declares, and change the row the cursor is on
([DECLARE CURSOR](../sql/statements/declare-cursor.md)). The client
declares no cursor, so it runs no FETCH, and no UPDATE or DELETE ...
WHERE CURRENT OF; but a query it runs is read by a cursor of its own all
the same, its rows sent and shown a batch at a time as they are
fetched, not held whole first.
