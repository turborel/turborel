# DELETE

[Statements](README.md)

## NAME

DELETE: delete rows

## SYNOPSIS

```
<delete statement: searched> ::=
    DELETE FROM <table name> [ WHERE <search condition> ]
```

## DESCRIPTION

DELETE removes the rows of a table, or through an updatable view of the
table it is drawn from, for which the search condition is true; every
row, without WHERE. The search condition is evaluated for every row
before any is deleted. A foreign key that references a row deleted acts
as it says: refuses the delete, deletes or changes the rows that refer
to it, or sets their columns null or to their defaults. A DELETE that
deletes no row completes with no data.

The positioned form, WHERE CURRENT OF a cursor, runs only in a program
that embeds turborel; see [DECLARE CURSOR](declare-cursor.md).

## RULES

1. The table exists and is not read-only: a base table, or a view that
   can be updated.
2. The search condition contains no set function outside a subquery.
3. The rows left, and those referential actions change, satisfy the
   constraints.
4. The transaction is not READ ONLY, but for a temporary table.

## PRIVILEGES

DELETE on the table. SELECT on each other table a subquery reads.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The table does not exist or is read-only; the search condition has a set function outside a subquery |
| 2A000 | -104 | Its user lacks DELETE on the table, or SELECT on a table read |
| 23000 | -201 | A row deleted is referenced by a foreign key whose action refuses it, or a referential action breaks a constraint |
| TB911 | -911 | A row was written by another transaction, not committed, or committed after this one's snapshot: the statement is undone, and may be run again |
| 25000 | -917 | The transaction is READ ONLY |
| 02000 | 100 | Not an error: no row was deleted |

A statement that fails deletes no row.

## EXAMPLES

Of the table PARTS of [UPDATE](update.md), as it was before those
updates:

```sql
DELETE FROM PARTS WHERE QTY = 0;
SELECT ID FROM PARTS ORDER BY ID;
DELETE FROM PARTS WHERE ID = 99;
```

```
OK
           ID
--------------
            1
(1 row)
no data
```

```sql
DELETE FROM PARTS WHERE SUM(QTY) > 0;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): a set function needs an argument source (a <table expression>), which this <search condition> does not have [X3.135-1986 §5.8 SR1] (at character 24)
```

## STANDARD

ISO/IEC 9075:1992 §13.7, the `<delete statement: searched>`, and
X3.135-1986 §8.5. Referential actions are SQL-92's [§11.8]. TB911 is this
implementation's own.

## SEE ALSO

[INSERT](insert.md), [UPDATE](update.md), [DROP TABLE](drop-table.md),
[Constraints](../../user/07-constraints.md)
