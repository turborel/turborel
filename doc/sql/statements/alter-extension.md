# ALTER EXTENSION

[Statements](README.md)

## NAME

ALTER EXTENSION: update an extension to another version *(extension)*

## SYNOPSIS

```
<alter extension statement> ::=
    ALTER EXTENSION <extension name> UPDATE [ TO <character string literal> ]
        [ FROM <character string literal> ]
```

## DESCRIPTION

ALTER EXTENSION ... UPDATE replaces an extension by another version of
it. Without TO it is the newest installed under PLUGINS.DIRECTORIES,
which must be newer than the one created, and with TO the newest that
meets its range. FROM names a package file or a directory, as of
[CREATE EXTENSION](create-extension.md).

The new version's objects replace the old's, its processes started
anew, and the scripts of SQL its manifest names, UPGRADE FROM a range
the old version meets, run after, in the order of their ranges, in the
session's transaction. Each extension that requires it must accept the
new version. ROLLBACK undoes it all.

## RULES

1. The extension is created.
2. A version installed meets TO, or, without TO, is newer than the one
   created.
3. Each extension that requires it accepts the new version.
4. The transaction is not READ ONLY.

## PRIVILEGES

Only the database's owner, or any user of a database with none.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -102 | The statement is not of the form above |
| 42000 | -103 | Rule 1 or 2 is broken |
| 42000 | -104 | Its user is not the database's owner |
| 38T09 | -301 | An extension requiring it does not accept the new version |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

Versions 1.0.0 and 1.1.0 of the example are installed, 1.1.0 with a
script that keeps a note of the update:

```sql
CREATE TABLE NOTES (X VARCHAR(200));
CREATE EXTENSION EXAMPLE VERSION '1.0.0';
ALTER EXTENSION EXAMPLE UPDATE;
SELECT X FROM NOTES;
SELECT VERSION FROM INFORMATION_SCHEMA.EXTENSIONS;
```

```
OK
OK
OK
 X
--------------------------------------------------------------
 updated to 1.1.0
(1 row)
 VERSION
--------------------------------------------------------------
 1.1.0
(1 row)
```

There is none newer now:

```sql
ALTER EXTENSION EXAMPLE UPDATE;
```

```
turborel: SQLCODE -103 (SQLSTATE 42000): extension EXAMPLE is at 1.1.0, the newest version installed
```

## STANDARD

An extension: no part of SQL loads programs of other parties.

## SEE ALSO

[CREATE EXTENSION](create-extension.md),
[DROP EXTENSION](drop-extension.md)
