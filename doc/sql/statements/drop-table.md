# DROP TABLE

[Statements](README.md)

## NAME

DROP TABLE: drop a base table

## SYNOPSIS

```
<drop table statement> ::= DROP TABLE <table name> <drop behavior>

<drop behavior> ::= CASCADE | RESTRICT
```

## DESCRIPTION

DROP TABLE drops a base table: its rows, its indexes, and the privileges
on it. RESTRICT refuses the drop if a view reads the table, or a
constraint of another table or an assertion refers to it. CASCADE drops
those views, and the views that read them, however far; drops the
assertions; and takes from each other table the constraints that refer
to it, a foreign key that references it among them, leaving the table.

DROP TABLE runs within the session's transaction: ROLLBACK undoes it.
Until the transaction ends, others read the table as it was; one that
writes it is refused (SQLSTATE TB911), or waits after `SET LOCKS.WAIT =
'ON'`.

## RULES

1. The table exists, and is a base table, not a view.
2. Its user owns the table's schema.
3. The table is not a declared local temporary table: that goes when
   the session ends.
4. RESTRICT: no view, no constraint of another table, and no assertion
   refers to the table.
5. No cursor of the transaction reads the table.
6. The transaction is not READ ONLY.

## PRIVILEGES

The table's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | No `<drop behavior>` is written |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the table's schema |
| 24000 | -501 | A cursor of the transaction reads the table |
| 25000 | -917 | The transaction is READ ONLY |
| TB911 | -911 | Another transaction holds a lock on the table |

## EXAMPLES

Of tables DEPT (CODE CHAR(3) PRIMARY KEY) and EMP (ID INTEGER, DEPT
CHAR(3) REFERENCES DEPT), and a view ENG of EMP:

```sql
DROP TABLE DEPT RESTRICT;
DROP TABLE EMP RESTRICT;
DROP TABLE ENG CASCADE;
DROP TABLE NOPE CASCADE;
DROP TABLE DEPT;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): constraint ANN.EMP_FOREIGN1 of ANN.EMP refers to ANN.DEPT, so it is dropped only with CASCADE [ISO/IEC 9075:1992 §11.18 SR4] (at character 11)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.EMP is read by view ANN.ENG, so it is dropped only with CASCADE [ISO/IEC 9075:1992 §11.18 SR4] (at character 11)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.ENG is a view, not a base table [ISO/IEC 9075:1992 §11.18 SR2] (at character 11)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.NOPE is not defined [ISO/IEC 9075:1992 §11.18 SR1] (at character 11)
turborel: SQLCODE -102 (SQLSTATE 2A000): expected CASCADE or RESTRICT, found end of statement (at character 15)
```

With CASCADE, EMP stays, without its foreign key:

```sql
DROP TABLE DEPT CASCADE;
SELECT CONSTRAINT_NAME FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE TABLE_NAME = 'EMP';
```

```
OK
 CONSTRAINT_NAME
--------------------------------------------------------------
(0 rows)
```

## STANDARD

ISO/IEC 9075:1992 §11.18. SQL-86 has no DROP TABLE.

## SEE ALSO

[CREATE TABLE](create-table.md), [ALTER TABLE](alter-table.md),
[DROP VIEW](drop-view.md), [DROP SCHEMA](drop-schema.md)
