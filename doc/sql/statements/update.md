# UPDATE

[Statements](README.md)

## NAME

UPDATE: update rows

## SYNOPSIS

```
<update statement: searched> ::=
    UPDATE [ WITH <XML lexically scoped options> ] <table name>
    [ FOR PORTION OF <period name> FROM <point in time> TO <point in time> ]
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

The table's triggers run as [CREATE TRIGGER](create-trigger.md) says:
its BEFORE triggers once the rows and their new values are found, before any is changed, after which a generated column is computed again from each row as they left it, its AFTER triggers at the end of the
statement. A view with an INSTEAD OF trigger of the statement's event
runs it in place of the change, and nothing is written through the view:
its row count is of the rows the trigger was given. SET DEFAULT gives a
column of the view the default an [INSERT](insert.md) would. An
updatable view
over such a view runs its trigger the same way, and so does the
positioned form through a cursor over either. A
trigger of UPDATE OF its columns runs only if a SET clause names one.

FOR PORTION OF a period of the table (see [CREATE TABLE](create-table.md))
changes only the time from its first point to its second. Each row the
WHERE condition is true of whose period overlaps that time is changed
for the part within it, its period cut to that part, and its parts
before and after are kept unchanged as rows of their own. A part kept
takes its identity column's next value and its generated columns again,
and is inserted as an INSERT inserts a row, its INSERT triggers
running; the constraints judge it with the rest as the statement ends.
Its row count is of the rows changed.

WITH's XML options, XMLNAMESPACES and XMLBINARY, are in scope of the
whole statement, its SET values, search condition and subqueries too, of the
positioned form as well: see [XML Values](../19-xml.md#xml-options-of-a-scope).

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
8. FOR PORTION OF names an application-time period of the table, a
   base table, not SYSTEM_TIME, a reserved word, whose
   columns no SET clause sets. Its points in time are datetimes
   comparable with the period's columns, reference no column of the
   table, and invoke no function that is not deterministic or modifies
   SQL-data.
9. No SET clause sets a column of ROW START or ROW END. A row of a
   table of a system-time period begins at the transaction timestamp as
   it is updated, and of a system-versioned table its old values are
   kept as a historical row.

## PRIVILEGES

UPDATE on each column set. SELECT on each other table a subquery reads;
reading the table's own columns, in WHERE or in a value, needs no
privilege beyond UPDATE.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The table does not exist or is read-only; a column set is not of it, or is set twice; a value is not assignable to its column, or contains a set function; a rule of FOR PORTION OF is broken (rule 8); WITH's XML options break a rule of [XML Values](../19-xml.md#xml-options-of-a-scope) |
| 2A000 | -104 | Its user lacks UPDATE on a column set, or SELECT on a table read |
| 23000 | -201 | A row as changed breaks a constraint |
| 44000 | -201 | A row changed through a view WITH CHECK OPTION is no longer a row of the view |
| 22xxx | -301 | A data exception as a value is computed or assigned |
| 2202E | -301 | An element set at an index not from 1 to the maximum cardinality (rule 7) |
| 2200E | -301 | An element set of a null array (rule 7) |
| 2201H | -301 | A row of a system-time period begins after the transaction timestamp: another transaction changed it later (rule 9) |
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

Of a table of prices through time, a week of January's price raised:

```sql
CREATE TABLE OFFERS (ITEM INTEGER, PRICE INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR VALID (S, E));
INSERT INTO OFFERS VALUES (1, 10, DATE '2026-01-01', DATE '2026-02-01');
UPDATE OFFERS FOR PORTION OF VALID FROM DATE '2026-01-10' TO DATE '2026-01-17' SET PRICE = 12;
SELECT * FROM OFFERS ORDER BY S;
```

```
OK
OK
OK
         ITEM |        PRICE | S          | E
--------------+--------------+------------+------------
            1 |           10 | 2026-01-01 | 2026-01-10
            1 |           12 | 2026-01-10 | 2026-01-17
            1 |           10 | 2026-01-17 | 2026-02-01
(3 rows)
```

## STANDARD

ISO/IEC 9075:1992 §13.10, the `<update statement: searched>`, and
X3.135-1986 §8.12. FOR PORTION OF is ISO/IEC 9075-2:2023 §14.14 SR7a
and §15.14 GR12, Feature T181. NULL and DEFAULT as an `<update source>` are SQL-92's
[§13.9]. The values identity columns and generated columns take are
ISO/IEC 9075-2:2023 §14.15 SR5 and §15.14. An `<update target>` of an
element is ISO/IEC 9075-2:2023's [§14.15 SR11, GR10b, §15.5 GR3b].
WITH's XML options are ISO/IEC 9075-14:2006 draft §14.6 and §14.7,
Features X082 and X132. TB911 is this implementation's own.

## SEE ALSO

[INSERT](insert.md), [DELETE](delete.md), [CREATE TRIGGER](create-trigger.md),
[DECLARE CURSOR](declare-cursor.md), [SET TRANSACTION](set-transaction.md),
[Changing Data](../../user/05-changing-data.md)
