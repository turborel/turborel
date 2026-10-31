# ALTER DATABASE

[Statements](README.md)

## NAME

ALTER DATABASE: hand the database on to another owner *(extension)*

## SYNOPSIS

```
<alter database statement> ::=
    ALTER DATABASE OWNER TO <authorization identifier>
```

## DESCRIPTION

ALTER DATABASE makes a user the database's owner, in place of the
session's own: the owner hands the database on, so a running server's
owner is changed without stopping it. The new owner manages the users
and sets the database's settings from its next statement on.

Sessions go on as they are: none is ended. A session of the owner
before goes on as the owner no more, from its next statement.

It is made at once, apart from any transaction: no ROLLBACK undoes it.
It is kept with the definitions, so it holds when the database is
opened again, and told to the diagnostics, `owner_changed`.

## RULES

1. The session has no transaction open: COMMIT or ROLLBACK first.
2. Its user is the database's owner.
3. The `<authorization identifier>` is a user.

## PRIVILEGES

The database's owner's alone.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 2A000 | -102 | OWNER, TO or the identifier is not written |
| 2A000 | -103 | There is no such user |
| 2A000 | -104 | Its user is not the database's owner |

## EXAMPLES

Run by ANN, the database's owner, beside the user BOB:

```sql
ALTER DATABASE OWNER TO nope;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no user NOPE to own the database (at character 24)
```

```sql
ALTER DATABASE OWNER TO bob;
```

```
OK
```

ANN owns the database no more:

```sql
ALTER DATABASE OWNER TO ann;
```

```
turborel: SQLCODE -104 (SQLSTATE 2A000): only the database's owner runs ALTER DATABASE, and ANN is not it
```

## STANDARD

An extension. ISO/IEC 9075:1992 has no statement of a database's owner:
which identifiers are authorization identifiers is
implementation-defined [§5.4; Annex B, item 41].

## SEE ALSO

[CREATE USER](create-user.md), [DROP USER](drop-user.md),
[SET DATABASE](set-database.md),
[Users](../../operator/05-users.md)
