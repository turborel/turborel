# DROP TYPE

[Statements](README.md)

## NAME

DROP TYPE: drop a distinct type

## SYNOPSIS

```
<drop data type statement> ::= DROP TYPE <user-defined type name> <drop behavior>

<drop behavior> ::= CASCADE | RESTRICT
```

## DESCRIPTION

DROP TYPE drops a distinct type, its two functions and the privileges on
them. RESTRICT refuses the drop if a column is of the type, an array of
it or a type based on it, if another type's source is based on it, or if
a view, a constraint, an assertion or a generated column names it or
invokes one of its functions.

CASCADE drops it all the same, with every type based on it. Each column
of one of those types, and each generated column that names one, is
dropped first, as ALTER TABLE DROP COLUMN with CASCADE drops it, and
the views, constraints and assertions that name one are dropped. A drop
that would drop every column of a table is refused before it drops any.

DROP TYPE runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The type exists, and its user owns the type's schema.
2. RESTRICT: nothing is based on the type, names it or invokes its
   functions.
3. CASCADE: no table would lose every column.
4. The transaction is not READ ONLY.

## PRIVILEGES

The type's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | No `<drop behavior>` is written |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the type's schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

Of the types and table of [CREATE TYPE](create-type.md):

```sql
CREATE VIEW HEAVY AS SELECT ID FROM PARCELS WHERE MASS > GRAMS(300);
DROP TYPE GRAMS CASCADE;
SELECT * FROM PARCELS;
SELECT COUNT(*) FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_NAME = 'HEAVY';
DROP TYPE MONEY RESTRICT;
```

```
OK
OK
           ID |        PRICE
--------------+--------------
            1 |        12.50
            2 |         8.00
(2 rows)

----------------------
                    0
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): column ANN.PARCELS.PRICE is of a type based on ANN.MONEY, so it is dropped only with CASCADE [ISO/IEC 9075-2:2023 §11.59 SR4b] (at character 10)
```

```sql
DROP TYPE NOPE CASCADE;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no type ANN.NOPE [ISO/IEC 9075-2:2023 §11.59 SR3] (at character 10)
```

## STANDARD

ISO/IEC 9075-2:2023 §11.59, Feature S011. A transform counts against
RESTRICT only if a CREATE TRANSFORM made it, as the one a type's
definition makes goes with it, and turborel has no CREATE TRANSFORM. See
[CONFORMANCE.md](../../../CONFORMANCE.md#distinct-types).

## SEE ALSO

[CREATE TYPE](create-type.md), [ALTER TABLE](alter-table.md)
