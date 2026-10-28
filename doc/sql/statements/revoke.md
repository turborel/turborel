# REVOKE

[Statements](README.md)

## NAME

REVOKE: take privileges away

## SYNOPSIS

```
<revoke statement> ::=
    REVOKE [ GRANT OPTION FOR ] <privileges> ON <object name>
    FROM <grantee> [ { , <grantee> }... ] <drop behavior>

<drop behavior> ::= CASCADE | RESTRICT
```

The `<privileges>`, `<object name>` and `<grantee>` are those of
[GRANT](grant.md).

## DESCRIPTION

REVOKE takes from each grantee the privileges its user granted it on
the object; GRANT OPTION FOR takes only the grant option, leaving the
privileges. A privilege another user granted the grantee is not taken.

What a privilege taken leaves without the privileges it needs is
abandoned: a privilege the grantee granted on, by the grant option it
loses; a view, a constraint, an assertion or a domain constraint of the
grantee's that needs the privilege. RESTRICT refuses the revoke if
anything would be abandoned; CASCADE destroys what is abandoned, and
what that abandons in turn.

A privilege named that its user had not granted the grantee is not
taken, and the statement completes with the warning SQLSTATE 01006.

REVOKE runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The object exists, as for [GRANT](grant.md).
2. RESTRICT: nothing is abandoned.
3. The transaction is not READ ONLY.

## PRIVILEGES

None: a user revokes only what it granted.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | No `<drop behavior>` is written, or the statement is not written as the synopsis shows |
| 2A000 | -103 | A rule above is broken |
| 25000 | -917 | The transaction is READ ONLY |
| 01006 | 0 | Not an error: a privilege named was not granted, and is not taken |

## EXAMPLES

Of the privileges of [GRANT](grant.md): BOB holds SELECT on STAFF with
grant option, granted SELECT to CAROL, and has a view STAFF_NAMES of
STAFF. Run by ANN:

```sql
REVOKE SELECT ON STAFF FROM BOB RESTRICT;
REVOKE GRANT OPTION FOR SELECT ON STAFF FROM BOB CASCADE;
SELECT GRANTOR, GRANTEE, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES
    WHERE TABLE_NAME = 'STAFF' AND PRIVILEGE_TYPE = 'SELECT' AND GRANTEE <> 'ANN' ORDER BY 2;
REVOKE SELECT ON STAFF FROM BOB CASCADE;
SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'STAFF_NAMES';
REVOKE DELETE ON STAFF FROM BOB RESTRICT;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): REVOKE ... RESTRICT would abandon a privilege BOB granted CAROL on ANN.STAFF [ISO/IEC 9075:1992 §11.37 SR18]
OK
 GRANTOR                                                      | GRANTEE                                                      | PRIVILEGE_TYPE                                               | IS_GRANTABLE
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 ANN                                                          | BOB                                                          | SELECT                                                       | NO
(1 row)
OK

----------------------
                    0
(1 row)
OK, with a warning: SQLSTATE 01006
```

Taking BOB's grant option took CAROL's SELECT, which it had granted;
taking BOB's SELECT took his view. ANN had granted BOB no DELETE.

## STANDARD

ISO/IEC 9075:1992 §11.37. SQL-86 has no REVOKE.

## SEE ALSO

[GRANT](grant.md), [DROP USER](drop-user.md)
