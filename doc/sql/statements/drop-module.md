# DROP MODULE

[Statements](README.md)

## NAME

DROP MODULE: drop an SQL-server module, with its routines and tables

## SYNOPSIS

```
<drop module statement> ::= DROP MODULE <SQL-server module name> <drop behavior>

<drop behavior> ::= CASCADE | RESTRICT
```

## DESCRIPTION

DROP MODULE drops a module [CREATE MODULE](create-module.md) defined,
with its routines, its temporary tables and the privileges on it.

With RESTRICT, a module is not dropped while a view, constraint,
assertion, generated column, trigger or routine outside it invokes one
of its routines. With CASCADE, those are dropped too, and what invokes
them, at any remove.

A module is dropped too, with CASCADE, when a drop or revoke takes one
of its routines: a drop of a table it reads or of a routine it invokes,
a revoke of a privilege its owner needs. Its other routines go with it.
DROP SCHEMA drops the modules of the schema, and those of other schemas
whose SCHEMA or PATH names it.

DROP MODULE runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The module exists, and its user owns its schema.
2. With RESTRICT, nothing outside the module invokes a routine of it.
3. The transaction is not READ ONLY.

## PRIVILEGES

The module's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | No `<drop behavior>` is written |
| 2A000 | -103 | There is no such module, or a rule above is broken |
| 2A000 | -104 | Its user does not own the module's schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

```sql
CREATE TABLE T (K INTEGER);
CREATE MODULE COUNTS
  FUNCTION ROWS_OF_T () RETURNS INTEGER READS SQL DATA RETURN (SELECT COUNT(*) FROM T);
  FUNCTION NONE () RETURNS INTEGER CONTAINS SQL RETURN 0;
END MODULE;
CREATE VIEW SIZES (N) AS SELECT ROWS_OF_T() FROM (VALUES (0)) AS ONE (X);
DROP FUNCTION NONE RESTRICT;
DROP MODULE COUNTS RESTRICT;
DROP MODULE COUNTS CASCADE;
SELECT * FROM SIZES;
SELECT COUNT(*) FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'ANN';
```

```
OK
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): routine ANN.NONE is of module ANN.COUNTS, not a schema-level routine: it goes with DROP MODULE [ISO/IEC 9075-2:2023 §11.62 SR2] (at character 14)
turborel: SQLCODE -103 (SQLSTATE 2A000): view ANN.SIZES invokes a routine of module ANN.COUNTS: drop it with CASCADE [ISO/IEC 9075-4:2006 draft §9.22 SR3] (at character 12)
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): table ANN.SIZES is not defined [X3.135-1986 §5.4 SR5] (at character 14)

----------------------
                    0
(1 row)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §9.22, Feature P001. SQL-86, SQL-92 and
ISO/IEC 9075-2 have no SQL-server modules.

## SEE ALSO

[CREATE MODULE](create-module.md), [DROP ROUTINE](drop-routine.md),
[DROP SCHEMA](drop-schema.md)
