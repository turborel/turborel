# SET ROLE

[Statements](README.md)

## NAME

SET ROLE: set the session's current role

## SYNOPSIS

```
<set role statement> ::=
    SET ROLE { <value specification> | NONE }
```

The `<value specification>` is a character string literal, CURRENT_ROLE,
or, in a statement prepared, a `?`.

## DESCRIPTION

SET ROLE sets the session's current role. The session's statements then
run with the privileges of its user, of the role, and of each role
applicable for the role: those granted to it, to PUBLIC, and to a role
applicable for it in turn. Until a role is set, the roles granted to a
user bring it no privileges. NONE removes the current role.

The value is a role's name as it is written in SQL, trimmed of spaces,
an undelimited name taken in upper case. CURRENT_ROLE
then gives the role, and CURRENT_USER and SESSION_USER the user still.

What the session defines is not helped by its role unless the role
owns it. A view or a constraint is checked with the privileges of the
owner of its schema, which are its own if the owner is a user, and its
roles' too if the owner is a role. A trigger or a routine of a schema a
role owns runs as that role alone, so CURRENT_USER is then null, and
CURRENT_ROLE the role.

SET ROLE begins no transaction. [SET SESSION
AUTHORIZATION](set-session-authorization.md) removes the current role.
INFORMATION_SCHEMA.ENABLED_ROLES lists the roles the session has
enabled, and the other views of the [Information
Schema](../07-information-schema.md) show what they hold.

## RULES

1. No transaction is active.
2. The value is a character string.
3. The session has a current user, so SET ROLE runs in no trigger or
   routine of a schema a role owns.
4. Trimmed of spaces, the value names a role granted to the session's
   user, or to PUBLIC.

## PRIVILEGES

None, beyond rule 4.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25001 | -917 | A transaction is active |
| 2A000 | -103 | The value is not a character string |
| 0P000 | -104 | The session has no current user; or the value names no role granted to its user or to PUBLIC |

## EXAMPLES

ANN owns table STAFF, and has granted its role CLERK SELECT on it, and
CLERK to BOB. Run by BOB:

```sql
SELECT NAME FROM ANN.STAFF;
ROLLBACK;
SET ROLE 'clerk';
SELECT NAME FROM ANN.STAFF;
VALUES (CURRENT_USER, CURRENT_ROLE, SESSION_USER);
SET ROLE NONE;
COMMIT;
SET ROLE NONE;
```

```
turborel: SQLCODE -104 (SQLSTATE 2A000): BOB has no SELECT privilege on ANN.STAFF [X3.135-1986 §5.25 SR1; X3.135-1986 §6.6 SR3] (at character 17)
OK
OK
 NAME
--------------------------------
 Ada
(1 row)
     |       |
-----+-------+-----
 BOB | CLERK | BOB
(1 row)
turborel: SQLCODE -917 (SQLSTATE 25001): SET ROLE in a transaction [ISO/IEC 9075-2:2023 §19.3 GR1]
OK
OK
```

Run by CAROL, who was granted no role:

```sql
SET ROLE 'CLERK';
```

```
turborel: SQLCODE -104 (SQLSTATE 0P000): no role CLERK is granted to CAROL or to PUBLIC [ISO/IEC 9075-2:2023 §19.3 GR3, GR4]
```

## STANDARD

ISO/IEC 9075-2:2023 §19.3, Feature T331.

## SEE ALSO

[CREATE ROLE](create-role.md), [GRANT](grant.md),
[SET SESSION AUTHORIZATION](set-session-authorization.md)
