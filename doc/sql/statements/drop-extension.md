# DROP EXTENSION

[Statements](README.md)

## NAME

DROP EXTENSION: drop an extension *(extension)*

## SYNOPSIS

```
<drop extension statement> ::=
    DROP EXTENSION <extension name> [ CASCADE | RESTRICT ]
```

RESTRICT is the default.

## DESCRIPTION

DROP EXTENSION drops an extension and its objects, and stops its
processes once no statement calls it. An extension that requires it,
and a view, generated column, constraint or routine that calls one of
its objects, depend on it. With RESTRICT the drop is refused while one
does. With CASCADE they are dropped first. Its package stays installed;
`turborel extension remove` removes it.

DROP EXTENSION runs within the session's transaction: ROLLBACK undoes
it.

## RULES

1. The extension is created.
2. With RESTRICT, nothing depends on it.
3. The transaction is not READ ONLY.

## PRIVILEGES

Only the database's owner, or any user of a database with none.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | The statement is not of the form above |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user is not the database's owner |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

```sql
CREATE EXTENSION EXAMPLE;
CREATE VIEW DOUBLED AS SELECT TWICE(2) AS D FROM (VALUES (1)) AS O (X);
DROP EXTENSION EXAMPLE;
```

```
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): view ANN.DOUBLED binds only with extension EXAMPLE, so it is dropped only with CASCADE (at character 15)
```

With CASCADE, the view goes with it:

```sql
DROP EXTENSION EXAMPLE CASCADE;
SELECT COUNT(*) FROM INFORMATION_SCHEMA.EXTENSIONS;
```

```
OK

----------------------
                    0
(1 row)
```

## STANDARD

An extension: no part of SQL loads programs of other parties.

## SEE ALSO

[CREATE EXTENSION](create-extension.md),
[ALTER EXTENSION](alter-extension.md)
