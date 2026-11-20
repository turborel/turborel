# CREATE EXTENSION

[Statements](README.md)

## NAME

CREATE EXTENSION: load an extension of an external plug-in *(extension)*

## SYNOPSIS

```
<create extension statement> ::=
    CREATE EXTENSION <extension name> [ VERSION <character string literal> ]
        [ FROM <character string literal> ]
```

## DESCRIPTION

CREATE EXTENSION loads an extension, a program of another party run as
processes of its own, whose manifest declares the objects it brings.
Each object is then the database's, a function called as a built-in
one is, a procedure, a metric, an embedding model, an authentication
mechanism, a backup target or a partitioning strategy
([Plug-ins](../../client/04-plugins.md)).

Without FROM the extension is found by its name among the packages
installed under PLUGINS.DIRECTORIES, at the newest version the range of
VERSION takes, `*` if none is written. The extensions its manifest
REQUIRES are created first, each at the newest version that meets every
range asked of it, unless the database has one of that name already,
whose version then must meet it. FROM names a package file, ending
`.tbx`, which is installed first, or a directory under one of
PLUGINS.DIRECTORIES that holds a manifest.

The extension's directory is checked as it is loaded and as each of its
processes starts. Of a package, every file's digest is checked, and its
signature as PLUGINS.TRUSTED_KEYS and PLUGINS.REQUIRE_SIGNED say. No process is
started until an object is called.

The database logs the statement with the manifest it read, so recovery
and a replica make the extension again of the log alone, and keeps the
extension's version, the database's lock of its packages, in its
catalog. CREATE EXTENSION runs within the session's transaction:
ROLLBACK undoes it, its processes stopped.

## RULES

1. No extension of the name is created, and no object it declares has
   the name of a function, metric, model, authenticator, target or
   strategy that is built in, registered or another extension's.
2. A version installed meets VERSION and each range required of it.
3. The directory is under one of PLUGINS.DIRECTORIES.
4. The manifest is of format 1, and its NAME is the extension's.
5. The transaction is not READ ONLY.

## PRIVILEGES

Only the database's owner, or any user of a database with none, loads
an extension, as its program runs with the server's own rights. Any
user may call its objects.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | The statement is not of the form above |
| 2A000 | -103 | Rule 1 is broken |
| 2A000 | -104 | Its user is not the database's owner |
| 38T05 | -301 | The directory is under none of PLUGINS.DIRECTORIES, or no cryptography is given to check a package |
| 38T06 | -301 | The manifest breaks a rule of format 1 |
| 38T07 | -301 | A package's files are not those its digests name |
| 38T08 | -301 | A package is not signed by a key trusted, and must be |
| 38T09 | -301 | No version installed meets the ranges asked |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

The example plug-in of [Plug-ins](../../client/04-plugins.md) is
installed under PLUGINS.DIRECTORIES:

```sql
CREATE EXTENSION EXAMPLE;
VALUES (TWICE(21), SHOUT('loud'));
SELECT EXTENSION_NAME, VERSION, PROCESSES_RUNNING FROM INFORMATION_SCHEMA.EXTENSIONS;
```

```
OK
              |
--------------+--------------------------------------------------------------
           42 | LOUD
(1 row)
 EXTENSION_NAME                                               | VERSION                                                      | PROCESSES_RUNNING
--------------------------------------------------------------+--------------------------------------------------------------+-------------------
 EXAMPLE                                                      | 1.0.0                                                        |                 1
(1 row)
```

Once is enough:

```sql
CREATE EXTENSION EXAMPLE;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): there is an extension EXAMPLE already (at character 17)
```

## STANDARD

An extension: no part of SQL loads programs of other parties.

## SEE ALSO

[ALTER EXTENSION](alter-extension.md),
[DROP EXTENSION](drop-extension.md),
[Packages](../../client/04-plugins-packages.md)
