# SET CATALOG

[Statements](README.md)

## NAME

SET CATALOG: set the default catalog

## SYNOPSIS

```
<set catalog statement> ::= SET CATALOG <value specification>
```

The `<value specification>` is a character string literal, USER,
CURRENT_USER, SESSION_USER or SYSTEM_USER, or, in a statement prepared,
a `?`.

## DESCRIPTION

SET CATALOG sets the catalog that names written without one are of. A
database is one catalog, named by its setting CATALOG_NAME, TURBOREL
unless set; it is the default catalog already, so SET CATALOG of its name
changes nothing, and of any other is refused. The value is read as an
identifier, its spaces trimmed: `' turborel '` names TURBOREL.

SET CATALOG begins no transaction, and may run in one.

## RULES

1. The value is a character string.
2. Its value, trimmed, is an identifier, the catalog's name.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The value is not a character string |
| 3D000 | -301 | The value is not the catalog's name |

## EXAMPLES

```sql
SET CATALOG 'TURBOREL';
SET CATALOG 'OTHER';
SET CATALOG 1;
```

```
OK
turborel: SQLCODE -301 (SQLSTATE 3D000): 'OTHER' is not the one catalog, TURBOREL [ISO/IEC 9075:1992 §16.1 GR2, §5.4 SR20]
turborel: SQLCODE -103 (SQLSTATE 2A000): SET CATALOG takes a character string [ISO/IEC 9075:1992 §16.1 SR1]
```

## STANDARD

ISO/IEC 9075:1992 §16.1. Which identifiers are valid catalog names is
the implementation's to say [§5.4 SR20]: here the one catalog's. SQL-86
has no catalogs.

## SEE ALSO

[SET SCHEMA](set-schema.md), [Names](../03-names.md)
