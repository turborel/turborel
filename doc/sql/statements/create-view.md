# CREATE VIEW

[Statements](README.md)

## NAME

CREATE VIEW: define a viewed table

## SYNOPSIS

```
<view definition> ::=
    CREATE VIEW <table name> [ ( <column name> [ { , <column name> }... ] ) ]
    AS <query expression>
    [ WITH [ CASCADED | LOCAL ] CHECK OPTION ]
```

The `<query expression>` is any query; see [SELECT](select.md).

## DESCRIPTION

CREATE VIEW defines a view: a table whose rows are those of its query,
found again each time the view is read. The view's columns are named by
its column list, or else as the query names them.

A view is updatable if its query is a query specification with no
DISTINCT, whose select list names columns only, whose FROM clause names
one table, a base table or an updatable view, with no GROUP BY or
HAVING, and none of whose subqueries reads the table it would change.
INSERT, UPDATE and DELETE on an updatable view change the table it is
drawn from. Any other view is read-only.

WITH CHECK OPTION refuses a row inserted or updated through the view
that is not then a row of it. CASCADED, the default, checks the row
against the condition of every view the change passes through; LOCAL,
against this view's own, and those of views below it that are WITH
CHECK OPTION themselves.

A view sees the columns its tables had when it was defined: a column
added to one later is not one of its.

The view's owner holds SELECT on it, grantable if the owner holds SELECT
grantable on each table it reads; and, on an updatable view, INSERT,
UPDATE and DELETE as it holds them on the table below.

CREATE VIEW runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The view's schema exists, and its user owns it. A view named without
   a schema is of the session's default schema.
2. No table or view of the schema has the name.
3. A column list names as many columns as the query has, none twice;
   without one, the query names each of its columns, none twice.
4. WITH CHECK OPTION is of an updatable view.
5. The query names no declared local temporary table.
6. The query follows the rules of a query.
7. The transaction is not READ ONLY.

## PRIVILEGES

SELECT on each table and view the query reads, held by the view's
owner.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 3F000 | -103 | The schema does not exist |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the schema, or lacks SELECT on a table read |
| 25000 | -917 | The transaction is READ ONLY |

Through the view, a row WITH CHECK OPTION refuses fails with SQLSTATE
44000, SQLCODE -201.

## EXAMPLES

Of the table STAFF of [SELECT](select.md):

```sql
CREATE VIEW ENGINEERS AS SELECT ID, NAME, SALARY FROM STAFF WHERE DEPT = 'ENG' WITH CHECK OPTION;
CREATE VIEW PAYROLL (DEPT, TOTAL) AS SELECT DEPT, SUM(SALARY) FROM STAFF GROUP BY DEPT;
SELECT * FROM PAYROLL ORDER BY DEPT;
UPDATE ENGINEERS SET SALARY = SALARY + 100 WHERE ID = 1;
INSERT INTO PAYROLL VALUES ('X', 1);
```

```
OK
OK
 DEPT |                TOTAL
------+----------------------
 ENG  |             15300.00
 OPS  |              5400.00
(2 rows)
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.PAYROLL is a read-only table [X3.135-1986 §8.7 SR2, §4.4] (at character 12)
```

A column added to STAFF later is not one of ENGINEERS':

```sql
COMMIT;
ALTER TABLE STAFF ADD COLUMN EMAIL VARCHAR(40);
SELECT * FROM ENGINEERS ORDER BY ID;
```

```
OK
OK
           ID | NAME         |     SALARY
--------------+--------------+------------
            1 | Ada          |    7300.00
            2 | Grace        |    8100.00
(2 rows)
```

Views refused:

```sql
CREATE VIEW BAD AS SELECT DEPT, SUM(SALARY) FROM STAFF GROUP BY DEPT;
CREATE VIEW BAD3 AS SELECT DEPT, COUNT(*) AS N FROM STAFF GROUP BY DEPT WITH CHECK OPTION;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.BAD needs a <view column list>: its query has an unnamed column or two of one name [X3.135-1986 §6.5 SR4] (at character 12)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.BAD3 is WITH CHECK OPTION, so must be updatable [X3.135-1986 §6.5 SR9] (at character 12)
```

## STANDARD

ISO/IEC 9075:1992 §11.19, and X3.135-1986 §6.5, whose view is of a query
specification. Which views are updatable is SQL-92's [§7.9 SR12]; LOCAL
and CASCADED are SQL-92's [§11.19 SR13, GR11]. A view is not
materialized: its query is bound into each query that reads it, which is
the implementor's to choose [X3.135-1986 §6.5 GR1]. A view sees the columns
of when it was defined [§11.11 GR2, note].

## SEE ALSO

[DROP VIEW](drop-view.md), [SELECT](select.md), [GRANT](grant.md),
[Concepts](../../user/03-concepts.md)
