# SET SESSION AUTHORIZATION

[Statements](README.md)

## NAME

SET SESSION AUTHORIZATION: set the session's authorization identifier

## SYNOPSIS

```
<set session authorization identifier statement> ::=
    SET SESSION AUTHORIZATION <value specification>
```

The `<value specification>` is a character string literal, USER,
CURRENT_USER, SESSION_USER or SYSTEM_USER, or, in a statement prepared,
a `?`.

## DESCRIPTION

SET SESSION AUTHORIZATION sets the session's authorization identifier:
whom its statements run as. The privileges of the new identifier are
those its statements then have, and USER, CURRENT_USER and SESSION_USER
give it. Its value is an identifier as it is written in SQL: it is
trimmed of spaces, and an undelimited name is taken in upper case.

A session begun by the database's owner may set any authorization
identifier, whether or not it is a user of the database. Any other
session may set only the one it began with, so a session the owner
began can set its own identifier back.

SET SESSION AUTHORIZATION begins no transaction. The session's default
schema stays as it was.

## RULES

1. The value is a character string.
2. No transaction is active.
3. Trimmed of spaces, the value is an authorization identifier: one
   identifier, regular or delimited, and not a key word.
4. The session was begun by the database's owner, or the value is the
   identifier the session began with.

## PRIVILEGES

None, beyond rule 4.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The value is not a character string |
| 25000 | -917 | A transaction is active |
| 28000 | -104 | The value is not an authorization identifier; or the session was not begun by the database's owner, and the value is not the identifier it began with |

## EXAMPLES

A session begun by ANN, the database's owner, runs as BOB, then as ANN
again:

```sql
SET SESSION AUTHORIZATION 'BOB';
SELECT CURRENT_USER, SESSION_USER FROM INFORMATION_SCHEMA.USERS;
COMMIT;
SET SESSION AUTHORIZATION 'ann';
```

```
OK
     |
-----+-----
 BOB | BOB
(1 row)
OK
OK
```

A session begun by BOB, who does not own the database:

```sql
SET SESSION AUTHORIZATION 'ANN';
```

```
turborel: SQLCODE -104 (SQLSTATE 28000): a session begun by BOB sets no authorization identifier but its own: only the database's owner's does [ISO/IEC 9075:1992 §16.4 GR5]
```

A value that is not an authorization identifier:

```sql
SET SESSION AUTHORIZATION '1A';
```

```
turborel: SQLCODE -104 (SQLSTATE 28000): '1A' is not an <authorization identifier> [ISO/IEC 9075:1992 §16.4 GR3]
```

## STANDARD

ISO/IEC 9075:1992 §16.4. Who may set another authorization identifier,
and what restrictions apply, is implementation-defined [GR4]: here, a
session begun by the database's owner sets any; any other, only its own.
SQL-86 has no SET SESSION AUTHORIZATION.

## SEE ALSO

[SET SCHEMA](set-schema.md), [CREATE USER](create-user.md),
[GRANT](grant.md),
[Users and Authentication](../../operator/05-users.md)
