# UPDATE

[Statements](README.md)

## NAME

UPDATE: update rows

## SYNOPSIS

```
<update statement: searched> ::=
    UPDATE <table name>
    SET <set clause> [ { , <set clause> }... ]
    [ WHERE <search condition> ]

<set clause> ::= <update target> = <update source>

<update target> ::= <column name> | <column name> [ <simple value specification> ]

<update source> ::= <value expression> | NULL | DEFAULT
```

## DESCRIPTION

UPDATE changes the rows of a table, or through an updatable view of the
table it is drawn from, for which the search condition is true; every
row, without WHERE. Each SET clause gives a column its new value: a
value, the null value, or its default.

`SET A[i] = v` of an array column sets its element i alone, from 1. Past
the array's last element, the array is first extended with nulls to i.
The index is a literal or a parameter. The column is set once in the
statement, whole or by one element.

Each value is computed from the row as it was before the statement: SET
A = B, B = A exchanges the two. The search condition is evaluated for
every row before any is changed. Constraints are checked of the rows as
the statement leaves them, so a change may pass through a value another
row holds, as SET ID = ID + 1 on a key does. An UPDATE that changes no
row completes with no data.

A generated column is computed again from each row as changed. An
identity column set to DEFAULT takes its generator's next value. A
generated column, or an identity column GENERATED ALWAYS, is set only to
DEFAULT.

The positioned form, WHERE CURRENT OF a cursor, of a cursor named by
an extended name too ([ALLOCATE CURSOR](allocate-cursor.md)), runs only
in a program that embeds turborel; see [DECLARE CURSOR](declare-cursor.md).

## RULES

1. The table exists and is not read-only: a base table, or a view that
   can be updated.
2. Each column set is a column of it, set once.
3. Each value is assignable to its column's type, and contains no set
   function.
4. Each row as changed satisfies the table's constraints, and, through a
   view WITH CHECK OPTION, is a row of the view.
5. The transaction is not READ ONLY, but for a temporary table.
6. A generated column, or an identity column GENERATED ALWAYS, is set
   only to DEFAULT.
7. An element is set of an array column, at an index from 1 to its
   maximum cardinality, of an array not null.

## PRIVILEGES

UPDATE on each column set. SELECT on each other table a subquery reads;
reading the table's own columns, in WHERE or in a value, needs no
privilege beyond UPDATE.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The table does not exist or is read-only; a column set is not of it, or is set twice; a value is not assignable to its column, or contains a set function |
| 2A000 | -104 | Its user lacks UPDATE on a column set, or SELECT on a table read |
| 23000 | -201 | A row as changed breaks a constraint |
| 44000 | -201 | A row changed through a view WITH CHECK OPTION is no longer a row of the view |
| 22xxx | -301 | A data exception as a value is computed or assigned |
| 2202E | -301 | An element set at an index not from 1 to the maximum cardinality (rule 7) |
| 2200E | -301 | An element set of a null array (rule 7) |
| TB911 | -911 | A row was written by another transaction, not committed under LOCKS.WAIT OFF, or committed after this one's snapshot at REPEATABLE READ or SERIALIZABLE; or a wait for it would deadlock or passed LOCKS.TIMEOUT_MS: the statement is undone, and may be run again |
| 25000 | -917 | The transaction is READ ONLY |
| 02000 | 100 | Not an error: no row was changed |

A statement that fails changes no row.

## EXAMPLES

Of the table PARTS as the examples of [INSERT](insert.md) leave it:

```sql
UPDATE PARTS SET QTY = QTY + 5 WHERE ID > 10;
UPDATE PARTS SET NAME = DEFAULT, QTY = NULL WHERE ID = 2;
SELECT * FROM PARTS ORDER BY ID;
UPDATE PARTS SET QTY = 1 WHERE ID = 99;
```

```
OK
OK
           ID | NAME                 |          QTY
--------------+----------------------+--------------
            1 | bolt                 |          100
            2 | unnamed              |         NULL
            3 | unnamed              |            0
           11 | unnamed              |            5
           12 | unnamed              |            5
           13 | unnamed              |            5
           50 | fits                 |            5
(7 rows)
no data
```

Of the table TICKETS as the examples of [INSERT](insert.md) leave it:

```sql
UPDATE TICKETS SET SEAT = 'F01' WHERE TICKET = 1;
UPDATE TICKETS SET TICKET = DEFAULT WHERE TICKET = 2;
UPDATE TICKETS SET TICKET = 7 WHERE TICKET = 3;
SELECT * FROM TICKETS ORDER BY TICKET;
```

```
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): column TICKET is generated always, so it is set only to DEFAULT [ISO/IEC 9075-2:2023 §14.15 SR5] (at character 19)
       TICKET | SEAT | ROW_NO
--------------+------+--------
            1 | F01  | F
            3 | C01  | C
            4 | B07  | B
           50 | D02  | D
(4 rows)
```

Of a table of arrays:

```sql
CREATE TABLE SHIFTS (ID INTEGER PRIMARY KEY, DAYS CHAR(3) ARRAY[7]);
INSERT INTO SHIFTS VALUES (1, ARRAY['MON', 'TUE']), (2, NULL);
UPDATE SHIFTS SET DAYS[4] = 'FRI' WHERE ID = 1;
SELECT DAYS FROM SHIFTS WHERE ID = 1;
UPDATE SHIFTS SET DAYS[8] = 'SUN' WHERE ID = 1;
UPDATE SHIFTS SET DAYS[1] = 'SUN' WHERE ID = 2;
UPDATE SHIFTS SET DAYS[2] = 'WED', DAYS[3] = 'THU' WHERE ID = 1;
```

```
OK
OK
OK
 DAYS
--------------------------------------------
 ARRAY['MON', 'TUE', NULL, 'FRI']
(1 row)
turborel: SQLCODE -301 (SQLSTATE 2202E): element 8 of an array of maximum cardinality 7 is assigned [ISO/IEC 9075-2:2023 §15.5 GR3b i 4]
turborel: SQLCODE -301 (SQLSTATE 2200E): an element of a null array is assigned [ISO/IEC 9075-2:2023 §15.5 GR3b i 1]
turborel: SQLCODE -103 (SQLSTATE 2A000): column DAYS is set twice [X3.135-1986 §8.12 SR4] (at character 35)
```

Changes refused:

```sql
UPDATE PARTS SET ID = 1 WHERE ID = 2;
UPDATE PARTS SET QTY = SUM(QTY);
```

```
turborel: SQLCODE -201 (SQLSTATE 23000): PRIMARY KEY ANN.PARTS_PRIMARY1 (ID) of ANN.PARTS would be violated: a row of these values is there [X3.135-1986 §6.4 GR2]
turborel: SQLCODE -103 (SQLSTATE 2A000): a value of a SET clause contains no set function [X3.135-1986 §8.12 SR3] (at character 23)
```

## STANDARD

ISO/IEC 9075:1992 §13.10, the `<update statement: searched>`, and
X3.135-1986 §8.12. NULL and DEFAULT as an `<update source>` are SQL-92's
[§13.9]. The values identity columns and generated columns take are
ISO/IEC 9075-2:2023 §14.15 SR5 and §15.14. An `<update target>` of an
element is ISO/IEC 9075-2:2023's [§14.15 SR11, GR10b, §15.5 GR3b].
TB911 is this implementation's own.

## SEE ALSO

[INSERT](insert.md), [DELETE](delete.md),
[DECLARE CURSOR](declare-cursor.md), [SET TRANSACTION](set-transaction.md),
[Changing Data](../../user/05-changing-data.md)
