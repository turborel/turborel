# ALTER USER

[Statements](README.md)

## NAME

ALTER USER: give a user a new password *(extension)*

## SYNOPSIS

```
<alter user statement> ::=
    ALTER USER <authorization identifier> [ WITH ] PASSWORD <character string literal>
```

## DESCRIPTION

ALTER USER gives a user a new password: the clients that sign in as it
from then on prove it by that one. Sessions already signed in go
on: they proved who they are as they signed in.

It is made at once, apart from any transaction: no ROLLBACK undoes it.

## RULES

1. The session has no transaction open: COMMIT or ROLLBACK first.
2. Its user is the database's owner.
3. The user exists.
4. The database has a credentials plug-in to make a verifier: a server
   always has one.

## PRIVILEGES

The database's owner's alone.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 2A000 | -102 | PASSWORD, or its literal, is not written |
| 2A000 | -103 | There is no such user |
| 2A000 | -104 | Its user is not the database's owner |
| TB910 | -910 | An embedded database has no credentials plug-in |

## EXAMPLES

Run by ANN, the database's owner:

```sql
ALTER USER CAROL PASSWORD 'battery staple';
ALTER USER NOPE PASSWORD 'x';
```

```
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no user NOPE (at character 11)
```

## STANDARD

An extension.

## SEE ALSO

[CREATE USER](create-user.md), [DROP USER](drop-user.md)
