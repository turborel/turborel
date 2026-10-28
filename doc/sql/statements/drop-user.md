# DROP USER

[Statements](README.md)

## NAME

DROP USER: drop a user of the database *(extension)*

## SYNOPSIS

```
<drop user statement> ::= DROP USER <authorization identifier>
```

## DESCRIPTION

DROP USER drops a user: no client may sign in as it by a password from
then on. What its authorization identifier owns stays, as do the
privileges granted to it: an identifier is a name, not a user, and a
user of the identifier made again has them. Sessions already signed in
go on.

It is made at once, apart from any transaction: no ROLLBACK undoes it.

## RULES

1. The session has no transaction open: COMMIT or ROLLBACK first.
2. Its user is the database's owner.
3. The user exists, and is not the database's owner. The owner is
   changed by the operator, with `turboreld user owner`
   ([Users](../../operator/05-users.md)).

## PRIVILEGES

The database's owner's alone.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 2A000 | -103 | There is no such user, or it is the database's owner |
| 2A000 | -104 | Its user is not the database's owner |

## EXAMPLES

Run by ANN, the database's owner:

```sql
DROP USER CAROL;
DROP USER ANN;
```

```
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN owns the database: give it another owner first (at character 10)
```

## STANDARD

An extension.

## SEE ALSO

[CREATE USER](create-user.md), [ALTER USER](alter-user.md),
[REVOKE](revoke.md)
