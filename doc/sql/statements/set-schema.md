# SET SCHEMA

[Statements](README.md)

## NAME

SET SCHEMA: set the default schema

## SYNOPSIS

```
<set schema statement> ::= SET SCHEMA <value specification>
```

The `<value specification>` is a character string literal, USER,
CURRENT_USER, SESSION_USER or SYSTEM_USER, or, in a statement prepared,
a `?`.

## DESCRIPTION

SET SCHEMA sets the session's default schema: the schema of each name
written without one in the statements the session runs or prepares
after it. Its value is a schema's name as it is written in SQL: it is
trimmed of spaces, and an undelimited name is taken in upper case. It
may name the catalog too, `CATALOG.SCHEMA`.

A session's default schema is first its authorization identifier.

SET SCHEMA begins no transaction, may run within one, and is not undone
by ROLLBACK. The schema need not exist: a statement that then names a
table in it fails as the table is not found.

## RULES

1. The value is a character string.
2. Trimmed of spaces, it is a schema name: an identifier, or a catalog's
   name, a period and an identifier.
3. A catalog it names is the one catalog, the setting CATALOG.NAME.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The value is not a character string |
| 3F000 | -301 | The value is not a schema name |
| 3D000 | -301 | The value names a catalog other than the one there is |

## EXAMPLES

```sql
CREATE SCHEMA SALES AUTHORIZATION ANN CREATE TABLE ORDERS (N INTEGER);
INSERT INTO SALES.ORDERS VALUES (7);
COMMIT;
SET SCHEMA 'SALES';
SELECT N FROM ORDERS;
```

```
OK
OK
OK
OK
            N
--------------
            7
(1 row)
```

USER is the session's authorization identifier, ANN here, whose schema
has no table ORDERS:

```sql
SET SCHEMA USER;
SELECT N FROM ORDERS;
```

```
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): table ANN.ORDERS is not defined [X3.135-1986 §5.4 SR5] (at character 14)
```

The value is a name as SQL reads it, and may name the catalog:

```sql
SET SCHEMA ' turborel.sales ';
SET SCHEMA 'ELSEWHERE.SALES';
SET SCHEMA 'SALES ORDERS';
```

```
OK
turborel: SQLCODE -301 (SQLSTATE 3D000): the one catalog is TURBOREL, not ELSEWHERE [ISO/IEC 9075:1992 §16.2 GR3a]
turborel: SQLCODE -301 (SQLSTATE 3F000): 'SALES ORDERS' is not a <schema name> [ISO/IEC 9075:1992 §16.2 GR2]
```

## STANDARD

ISO/IEC 9075:1992 §16.2, and §4.30, by which the default schema is first
implementation-defined: here, the session's authorization identifier.
SQL-86 has no SET SCHEMA.

## SEE ALSO

[SET SESSION AUTHORIZATION](set-session-authorization.md),
[CREATE SCHEMA](create-schema.md),
[Names and Qualification](../03-names.md)
