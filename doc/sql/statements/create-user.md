# CREATE USER

[Statements](README.md)

## NAME

CREATE USER: make a user of the database *(extension)*

## SYNOPSIS

```
<user definition> ::=
    CREATE USER <authorization identifier> [ WITH ] PASSWORD <character string literal>
```

## DESCRIPTION

CREATE USER makes a user: an authorization identifier that a client of
the server may prove it is, by the password given. The database keeps
no password, only a verifier made of it, from which the password cannot
be found sooner than by guessing.

A user is a name a client signs in as; what it may do is the privileges
granted to its authorization identifier, as to any. A session the host
program of an embedded database starts may be of any identifier, a user
or not; and, by default, the server trusts a client of its own machine
as whoever it says it is, a user or not: the setting SERVER.TRUST OFF
makes every client prove it ([Users](../../operator/05-users.md)).

The authorization identifier is an identifier: a regular one, such as
`carol`, is folded to upper case, CAROL; a delimited one, such as
`"carol"`, is taken as written. INFORMATION_SCHEMA.USERS lists the users
to the owner, and to any other user itself alone.

CREATE USER is made at once, apart from any transaction: no ROLLBACK
undoes it.

## RULES

1. The session has no transaction open: COMMIT or ROLLBACK first.
2. Its user is the database's owner.
3. No user has the identifier.
4. The database has a credentials plug-in to make a verifier: a server
   always has one.

## PRIVILEGES

The database's owner's alone.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 2A000 | -102 | PASSWORD, or its literal, is not written |
| 2A000 | -103 | A user has the identifier |
| 2A000 | -104 | Its user is not the database's owner |
| TB910 | -910 | An embedded database has no credentials plug-in |

## EXAMPLES

Run by ANN, the database's owner, of users ANN and BOB:

```sql
CREATE USER CAROL WITH PASSWORD 'correct horse';
SELECT * FROM INFORMATION_SCHEMA.USERS ORDER BY USER_NAME;
CREATE USER BOB PASSWORD 'x';
CREATE USER DAVE;
```

```
OK
 USER_NAME                                                    | IS_OWNER
--------------------------------------------------------------+----------
 ANN                                                          | YES
 BOB                                                          | NO
 CAROL                                                        | NO
(3 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): a user BOB exists already (at character 12)
turborel: SQLCODE -102 (SQLSTATE 2A000): expected PASSWORD, found end of statement (at character 16)
```

Run by BOB:

```sql
CREATE USER DAVE PASSWORD 'x';
```

```
turborel: SQLCODE -104 (SQLSTATE 2A000): only the database's owner runs a user statement, and BOB is not it
```

## STANDARD

An extension. Which identifiers are authorization identifiers is
implementation-defined [ISO/IEC 9075:1992 §5.4]; neither SQL-86 nor
SQL-92 has a statement that makes one.

## SEE ALSO

[ALTER USER](alter-user.md), [DROP USER](drop-user.md),
[SET SESSION AUTHORIZATION](set-session-authorization.md),
[GRANT](grant.md), [CONNECT](connect.md)
