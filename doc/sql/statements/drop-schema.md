# DROP SCHEMA

[Statements](README.md)

## NAME

DROP SCHEMA: drop a schema

## SYNOPSIS

```
<drop schema statement> ::= DROP SCHEMA <schema name> <drop behavior>

<schema name> ::= [ <catalog name> . ] <identifier>

<drop behavior> ::= CASCADE | RESTRICT
```

## DESCRIPTION

DROP SCHEMA drops a schema. RESTRICT refuses the drop if the schema
holds a table, a view, a domain or an assertion. CASCADE drops each of
them first: its tables, as DROP TABLE CASCADE does, and so the views,
constraints and assertions of other schemas that refer to them; its
views left; its domains, as DROP DOMAIN CASCADE does; and its
assertions. A constraint named in the schema of a table or a domain of
another schema is dropped too, as ALTER TABLE or ALTER DOMAIN DROP
CONSTRAINT drops it; RESTRICT refuses the drop while there is one.

DROP SCHEMA runs within the session's transaction: ROLLBACK undoes it,
as it does not undo a [CREATE SCHEMA](create-schema.md).

## RULES

1. The schema exists; a catalog named is the one catalog.
2. Its user owns the schema.
3. RESTRICT: the schema holds nothing, and names no constraint of a
   table or domain of another schema.
4. The transaction is not READ ONLY.

## PRIVILEGES

The schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | No `<drop behavior>` is written |
| 3F000 | -103 | There is no such schema, or a catalog other than the one there is is named |
| 2A000 | -103 | RESTRICT, and the schema holds something |
| 2A000 | -104 | Its user does not own the schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

Run by ANN, of a schema SHOP of a table ITEMS and a view of it:

```sql
DROP SCHEMA SHOP RESTRICT;
DROP SCHEMA SHOP CASCADE;
ROLLBACK;
SELECT SCHEMA_NAME FROM INFORMATION_SCHEMA.SCHEMATA WHERE SCHEMA_NAME = 'SHOP';
DROP SCHEMA NOPE CASCADE;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): schema SHOP holds SHOP.ITEMS, so it is dropped only with CASCADE [ISO/IEC 9075:1992 §11.2 SR3] (at character 12)
OK
OK
 SCHEMA_NAME
--------------------------------------------------------------
 SHOP
(1 row)
turborel: SQLCODE -103 (SQLSTATE 3F000): there is no schema NOPE [ISO/IEC 9075:1992 §11.2 SR2] (at character 12)
```

Run by BOB:

```sql
DROP SCHEMA ANN CASCADE;
```

```
turborel: SQLCODE -104 (SQLSTATE 2A000): schema ANN is owned by ANN, not BOB [ISO/IEC 9075:1992 §11.2 AR1] (at character 12)
```

## STANDARD

ISO/IEC 9075:1992 §11.2. SQL-86 has no DROP SCHEMA. Dropping the
constraints named in the schema of another schema's tables and domains,
which SQL-92 leaves, is an extension.

## SEE ALSO

[CREATE SCHEMA](create-schema.md), [DROP TABLE](drop-table.md),
[DROP DOMAIN](drop-domain.md)
