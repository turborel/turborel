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

<set clause> ::= <column name> = <update source>

<update source> ::= <value expression> | NULL | DEFAULT
```

## DESCRIPTION

UPDATE changes the rows of a table, or through an updatable view of the
table it is drawn from, for which the search condition is true; every
row, without WHERE. Each SET clause gives a column its new value: a
value, the null value, or its default.

Each value is computed from the row as it was before the statement: SET
A = B, B = A exchanges the two. The search condition is evaluated for
every row before any is changed. Constraints are checked of the rows as
the statement leaves them, so a change may pass through a value another
row holds, as SET ID = ID + 1 on a key does. An UPDATE that changes no
row completes with no data.

The positioned form, WHERE CURRENT OF a cursor, runs only in a program
that embeds turborel; see [DECLARE CURSOR](declare-cursor.md).

## RULES

1. The table exists and is not read-only: a base table, or a view that
   can be updated.
2. Each column set is a column of it, set once.
3. Each value is assignable to its column's type, and contains no set
   function.
4. Each row as changed satisfies the table's constraints, and, through a
   view WITH CHECK OPTION, is a row of the view.
5. The transaction is not READ ONLY, but for a temporary table.

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
| TB911 | -911 | A row was written by another transaction, not committed, or committed after this one's snapshot: the statement is undone, and may be run again |
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
[§13.9]. TB911 is this implementation's own.

## SEE ALSO

[INSERT](insert.md), [DELETE](delete.md),
[DECLARE CURSOR](declare-cursor.md), [SET TRANSACTION](set-transaction.md),
[Changing Data](../../user/05-changing-data.md)
