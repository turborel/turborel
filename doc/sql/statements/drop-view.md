# DROP VIEW

[Statements](README.md)

## NAME

DROP VIEW: drop a viewed table

## SYNOPSIS

```
<drop view statement> ::= DROP VIEW <table name> <drop behavior>

<drop behavior> ::= CASCADE | RESTRICT
```

## DESCRIPTION

DROP VIEW drops a view and the privileges on it; no row of any table
changes. RESTRICT refuses the drop if another view reads it, or a
constraint or an assertion refers to it. CASCADE drops those views, and
the views that read them, however far, and the constraints and
assertions.

DROP VIEW runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The view exists, and is a view, not a base table.
2. Its user owns the view's schema.
3. RESTRICT: no other view, no constraint and no assertion refers to it.
4. No cursor of the transaction reads the view.
5. The transaction is not READ ONLY.

## PRIVILEGES

The view's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | No `<drop behavior>` is written |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the view's schema |
| 24000 | -501 | A cursor of the transaction reads the view |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

Of the view ENG of [DROP TABLE](drop-table.md):

```sql
CREATE VIEW ENG2 AS SELECT ID FROM ENG;
DROP VIEW ENG RESTRICT;
DROP VIEW ENG CASCADE;
SELECT * FROM ENG2;
DROP VIEW EMP CASCADE;
```

```
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.ENG is read by view ANN.ENG2, so it is dropped only with CASCADE [ISO/IEC 9075:1992 §11.20 SR3] (at character 10)
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): table ANN.ENG2 is not defined [X3.135-1986 §5.4 SR5] (at character 14)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.EMP is a base table, not a view [ISO/IEC 9075:1992 §11.20 SR2] (at character 10)
```

## STANDARD

ISO/IEC 9075:1992 §11.20. SQL-86 has no DROP VIEW.

## SEE ALSO

[CREATE VIEW](create-view.md), [DROP TABLE](drop-table.md)
