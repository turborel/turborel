# CREATE ROLE

[Statements](README.md)

## NAME

CREATE ROLE: define a role

## SYNOPSIS

```
<role definition> ::=
    CREATE ROLE <role name> [ WITH ADMIN <grantor> ]

<grantor> ::= CURRENT_USER | CURRENT_ROLE
```

## DESCRIPTION

CREATE ROLE defines a role: a name that privileges, and other roles,
are granted to, and that is granted in turn to users, to roles and to
PUBLIC. A session takes on the privileges of a role granted to its user
by [SET ROLE](set-role.md). A role may own a schema too, which is then
used by a session that has set the role.

The role is granted WITH ADMIN OPTION to its creator: to the session's
user, or, with WITH ADMIN CURRENT_ROLE, to the role the session has
set. Its holder may grant the role to others (see
[GRANT](grant.md)), revoke it (see [REVOKE](revoke.md)) and drop it
(see [DROP ROLE](drop-role.md)).

CREATE ROLE runs within the session's transaction: ROLLBACK undoes it.
A role is an authorization identifier, so DEFINITION_SCHEMA.USERS lists
it, but no client signs in as a role.

## RULES

1. The name is no authorization identifier the database knows already:
   no user's, no role's, no schema's owner, and no grantor or grantee of
   a privilege or a role. Nor is it PUBLIC or _SYSTEM.
2. With WITH ADMIN CURRENT_USER, the session has a current user; with
   WITH ADMIN CURRENT_ROLE, it has set a role.
3. The transaction is not READ ONLY.

## PRIVILEGES

The database's owner creates roles, and so does each user it allows by
[ALTER USER](alter-user.md) ... CREATE ROLE.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -103 | A rule above is broken |
| 42000 | -104 | The session's user may not create a role |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

Run by ANN, the database's owner, of whose users BOB and CAROL are two:

```sql
CREATE ROLE CLERK;
CREATE ROLE BOB;
COMMIT;
```

```
OK
turborel: SQLCODE -103 (SQLSTATE 42000): BOB is a user's authorization identifier [ISO/IEC 9075-2:2023 §12.4 SR1] (at character 12)
OK
```

BOB may not create a role until ANN allows it:

```sql
CREATE ROLE AUDITOR;
```

```
turborel: SQLCODE -104 (SQLSTATE 42000): BOB may not create a role, which the database's owner and each user it allows by ALTER USER ... CREATE ROLE may [ISO/IEC 9075-2:2023 §12.4 AR1] (at character 12)
```

Then ANN runs `ALTER USER BOB CREATE ROLE`, and BOB's CREATE ROLE AUDITOR
succeeds. Who may create a role, as ANN sees it:

```sql
SELECT USER_NAME, CREATE_ROLE FROM INFORMATION_SCHEMA.USERS ORDER BY 1;
```

```
 USER_NAME                                                    | CREATE_ROLE
--------------------------------------------------------------+-------------
 ANN                                                          | YES
 BOB                                                          | YES
 CAROL                                                        | NO
(3 rows)
```

## STANDARD

ISO/IEC 9075-2:2023 §12.4, Feature T331. Who may create a role is
implementation-defined (IW141): the database's owner, and the users it
allows.

## SEE ALSO

[DROP ROLE](drop-role.md), [SET ROLE](set-role.md), [GRANT](grant.md),
[REVOKE](revoke.md), [ALTER USER](alter-user.md)
