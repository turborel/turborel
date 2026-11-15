# MERGE

[Statements](README.md)

## NAME

MERGE: update, delete and insert rows of a table from one source

## SYNOPSIS

```
<merge statement> ::=
    MERGE INTO <table name> [ [ AS ] <correlation name> ]
    USING <table reference>
    ON <search condition>
    <merge when clause>...

<merge when clause> ::=
    WHEN MATCHED [ AND <search condition> ]
      THEN { UPDATE SET <set clause> [ { , <set clause> }... ] | DELETE }
  | WHEN NOT MATCHED [ AND <search condition> ]
      THEN INSERT [ ( <column name> [ { , <column name> }... ] ) ]
      [ OVERRIDING { USER | SYSTEM } VALUE ]
      VALUES ( <merge insert value> [ { , <merge insert value> }... ] )

<merge insert value> ::= <value expression> | NULL | DEFAULT
```

## DESCRIPTION

MERGE changes a table, or the base table of an updatable view, from the
rows of a source: a table, a view, a derived table or a table value
constructor. The ON condition pairs each source row with the target rows
it matches. Each WHEN clause says what is done, and for each row the first
clause that applies to it decides, so one statement does what an upsert
otherwise takes two statements and a retry for.

A target row some source row matches is changed by the first WHEN MATCHED
clause whose condition, if it has one, is true of a pair of it and a
source row. That clause updates it, its SET values computed from the
target row and that source row, or deletes it. If two source rows the
clause is true of match one target row, the statement is refused, since
it cannot tell which to take. A target row no clause applies to is left
as it is.

A source row that matches no target row is inserted by the first WHEN NOT
MATCHED clause that applies to it. A clause without a condition applies
to each such row. A clause with a condition c applies to a source row if
no target row makes `ON` or `NOT c` true, so it reads the target too.
With an empty target every source row is inserted, and a condition that
is unknown for a row inserts it. The values come from the source row
alone, so a target column may not be named in them.

The source and the target are read before any row is changed, so a row
the statement inserts is never matched, and a source that reads the
target sees it as it was. Rows are changed clause by clause, in the order
the clauses are written. Constraints are checked of the rows as the
statement leaves them, and a view's CHECK OPTION of every row it updates
or inserts. Generated columns are computed again from each row as
changed, and identity columns get their generators' next values as
INSERT gives them.

The target's triggers run as [CREATE TRIGGER](create-trigger.md) says,
clause by clause: each clause runs the BEFORE triggers of its event on
its own rows before it changes them, a statement trigger once for the
statement whatever clauses share its event, and the AFTER triggers run
at the end of the statement over the rows of every clause. A MERGE into
a view with an INSTEAD OF trigger is refused (SQLCODE -900).

A MERGE whose source has no row completes with no data. One whose source
has rows completes, however many it changes. Its row count is of the rows
read, not of those changed. With WHEN MATCHED clauses alone it is the
pairs of a target row and a source row ON matches, with WHEN NOT MATCHED
clauses alone the source rows that match no target row, and with both the
two together.

At READ COMMITTED, a target row another transaction changed after the
statement read it is matched again as that transaction left it, against
the same source row. If ON and its clause's condition still hold, it is
changed from that version. If not, it is left alone, and it is never
inserted instead. A row another transaction inserts meanwhile is not
seen, so an insert of the same key fails its unique constraint.

## RULES

1. The table exists and is not read-only: a base table, or a view that
   can be updated, and for a view with WHEN NOT MATCHED, one that can be
   inserted into.
2. The source is not a joined table directly, though a derived table of
   one is. Its name, or its correlation name, differs from the target's.
3. Each column set or inserted is a column of the table, named once in
   its clause.
4. An insert gives as many values as it names columns, each assignable
   to its column. A generated column is given only DEFAULT, and an
   identity column as its OVERRIDING clause allows.
5. No condition or value contains a set function.
6. The transaction is not READ ONLY, but for a temporary table.

## PRIVILEGES

UPDATE on each column a WHEN MATCHED clause sets, DELETE on the table if
a clause deletes, INSERT on each column a WHEN NOT MATCHED clause
inserts, and SELECT on the source's tables.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The table does not exist or is read-only; the source is a joined table, or exposes the target's name; a column is not of the table, or is named twice; the values are not as many as the columns, or one is not assignable to its column; a target column is named in the values; a set function is in a condition or a value |
| 2A000 | -104 | Its user lacks a privilege the statement needs |
| 21000 | -811 | Two source rows a WHEN MATCHED clause is true of match one target row |
| 23000 | -201 | A row as changed breaks a constraint |
| 44000 | -201 | A row updated or inserted through a view WITH CHECK OPTION is not a row of the view |
| 22xxx | -301 | A data exception as a value is computed or assigned |
| TB911 | -911 | A row was written by another transaction, not committed under LOCKS.WAIT OFF, or committed after this one's snapshot at REPEATABLE READ or SERIALIZABLE; or a wait for it would deadlock or passed LOCKS.TIMEOUT_MS |
| 25006 | -917 | The transaction is READ ONLY |
| 02000 | 100 | Not an error: the source had no row |

A statement that fails changes no row.

## EXAMPLES

```sql
CREATE TABLE STOCK (ITEM VARCHAR(10) NOT NULL PRIMARY KEY, QTY INTEGER);
CREATE TABLE DELIVERY (ITEM VARCHAR(10), QTY INTEGER);
INSERT INTO STOCK VALUES ('bolt', 10), ('nut', 5), ('washer', 2);
INSERT INTO DELIVERY VALUES ('bolt', 5), ('nut', -5), ('screw', 20);
MERGE INTO STOCK AS S USING DELIVERY AS D ON S.ITEM = D.ITEM
  WHEN MATCHED AND S.QTY + D.QTY = 0 THEN DELETE
  WHEN MATCHED THEN UPDATE SET QTY = S.QTY + D.QTY
  WHEN NOT MATCHED THEN INSERT (ITEM, QTY) VALUES (D.ITEM, D.QTY);
SELECT ITEM, QTY FROM STOCK ORDER BY ITEM;
DELETE FROM DELIVERY;
MERGE INTO STOCK USING DELIVERY ON STOCK.ITEM = DELIVERY.ITEM
  WHEN MATCHED THEN DELETE;
```

```
OK
OK
OK
OK
OK
 ITEM       |          QTY
------------+--------------
 bolt       |           15
 screw      |           20
 washer     |            2
(3 rows)
OK
no data
```

Two source rows matching one target row:

```sql
CREATE TABLE PRICES (ITEM VARCHAR(10) NOT NULL PRIMARY KEY, PRICE INTEGER);
INSERT INTO PRICES VALUES ('bolt', 3);
MERGE INTO PRICES USING (VALUES ('bolt', 4), ('bolt', 5)) AS N (ITEM, PRICE) ON PRICES.ITEM = N.ITEM
  WHEN MATCHED THEN UPDATE SET PRICE = N.PRICE;
SELECT ITEM, PRICE FROM PRICES;
```

```
OK
OK
turborel: SQLCODE -811 (SQLSTATE 21000): a target row is matched by more than one source row its WHEN clause 1 applies to [ISO/IEC 9075-2:2023 §14.12 GR6a i 1 B II]
 ITEM       |        PRICE
------------+--------------
 bolt       |            3
(1 row)
```

## STANDARD

ISO/IEC 9075-2:2023 §14.12, the `<merge statement>`, Features F312, F313
and F314. How a row another transaction changed is matched again is this
implementation's own (RULES.md D29), as is TB911. See
[CONFORMANCE.md](../../../CONFORMANCE.md#merge).

## SEE ALSO

[INSERT](insert.md), [UPDATE](update.md), [DELETE](delete.md),
[CREATE VIEW](create-view.md), [CREATE TRIGGER](create-trigger.md)
