# DECLARE LOCAL TEMPORARY TABLE

[Statements](README.md)

## NAME

DECLARE LOCAL TEMPORARY TABLE: declare a table of the session's own

## SYNOPSIS

```
<temporary table declaration> ::=
    DECLARE LOCAL TEMPORARY TABLE MODULE . <table name>
    ( <table element> [ { , <table element> }... ] )
    [ ON COMMIT { DELETE | PRESERVE } ROWS ]
```

The `<table element>`s are those of [CREATE TABLE](create-table.md).

## DESCRIPTION

DECLARE LOCAL TEMPORARY TABLE declares a table that is the session's
alone: no other session sees it, nor its rows. It is named MODULE.T,
and in no schema; its rows are kept as any temporary table's are
([Temporary Tables](../../user/09-temporary-tables.md)), never logged,
never recovered. It is in no view of the Information Schema. The session's
user holds every privilege on it.

ON COMMIT DELETE ROWS, the default, empties it as each transaction
ends; PRESERVE ROWS keeps its rows.

The declaration is made at once, apart from the session's transaction:
no ROLLBACK undoes it. The table goes when the session ends; nothing
else drops it.

## RULES

1. The name is qualified by MODULE.
2. The session has declared no table of the name.
3. The table elements follow the rules of [CREATE TABLE](create-table.md).
   A foreign key or a CHECK of a declared table refers to temporary
   tables only: to no persistent one.
4. Of the other statements, none alters the table, drops it, grants or
   revokes a privilege on it, or defines a view that names it; and a
   persistent table, or a global or created local temporary one, refers
   to it in no constraint.

## PRIVILEGES

Those of [CREATE TABLE](create-table.md) for the tables its constraints
refer to.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | The name is not qualified by MODULE |
| 01008 | 0 | Not an error: a warning, a BIT default shorter than its column padded with 0 bits |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user lacks a privilege a constraint needs |

## EXAMPLES

```sql
DECLARE LOCAL TEMPORARY TABLE MODULE.SCRATCH (K INTEGER, V VARCHAR(10)) ON COMMIT PRESERVE ROWS;
INSERT INTO MODULE.SCRATCH VALUES (1, 'a');
COMMIT;
SELECT * FROM MODULE.SCRATCH;
SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'SCRATCH';
```

```
OK
OK
OK
            K | V
--------------+------------
            1 | a
(1 row)

----------------------
                    0
(1 row)
```

What is refused, PERS a persistent table:

```sql
DECLARE LOCAL TEMPORARY TABLE MODULE.F1 (K INTEGER REFERENCES PERS);
DROP TABLE MODULE.SCRATCH CASCADE;
ALTER TABLE MODULE.SCRATCH ADD COLUMN Z INTEGER;
CREATE VIEW SV AS SELECT * FROM MODULE.SCRATCH;
GRANT SELECT ON MODULE.SCRATCH TO BOB;
CREATE TABLE P (K INTEGER CHECK (K IN (SELECT K FROM MODULE.SCRATCH)));
DECLARE LOCAL TEMPORARY TABLE MODULE.SCRATCH (K INTEGER);
DECLARE LOCAL TEMPORARY TABLE ANN.X (K INTEGER);
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): MODULE.F1 may not refer to ANN.PERS: a declared local temporary table, to a persistent base one [ISO/IEC 9075:1992 §11.8 SR4] (at character 62)
turborel: SQLCODE -103 (SQLSTATE 2A000): MODULE.SCRATCH is a declared local temporary table: it goes with the session [ISO/IEC 9075:1992 §11.18 SR3] (at character 11)
turborel: SQLCODE -103 (SQLSTATE 2A000): MODULE.SCRATCH is a declared local temporary table: it is not altered [ISO/IEC 9075:1992 §11.10 SR5]
turborel: SQLCODE -103 (SQLSTATE 2A000): view ANN.SV names MODULE.SCRATCH, a declared local temporary table [ISO/IEC 9075:1992 §11.19 SR5] (at character 12)
turborel: SQLCODE -103 (SQLSTATE 2A000): MODULE.SCRATCH is a declared local temporary table: no privilege on it is granted or revoked [ISO/IEC 9075:1992 §10.3 SR1] (at character 16)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.P may not refer to MODULE.SCRATCH: a persistent base table, to a declared local temporary one [ISO/IEC 9075:1992 §11.9 SR3] (at character 13)
turborel: SQLCODE -103 (SQLSTATE 2A000): MODULE.SCRATCH is declared already [ISO/IEC 9075:1992 §13.11 SR1]
turborel: SQLCODE -102 (SQLSTATE 2A000): expected MODULE, found ANN (at character 30)
```

## STANDARD

ISO/IEC 9075:1992 §13.11, a `<directly executable statement>` [§20.1].
SQL-92 declares the table in a `<module>`; here a session is the module.
MODULE is a reserved word; no schema may be named MODULE, even
delimited. SQL-86 has no temporary tables.

## SEE ALSO

[CREATE TABLE](create-table.md),
[Temporary Tables](../../user/09-temporary-tables.md)
