# DROP ROLE

[Statements](README.md)

## NAME

DROP ROLE: drop a role

## SYNOPSIS

```
<drop role statement> ::=
    DROP ROLE <role name> { RESTRICT | CASCADE }
```

## DESCRIPTION

DROP ROLE drops a role. The role is revoked from each grantee it was
granted to, by any grantor, as REVOKE does (see [REVOKE](revoke.md)).
So go the roles granted to the role, the privileges granted to it, and
the privileges and roles it granted, so that nothing names it once it
is gone, and its name may be taken again.

With RESTRICT, it is refused if the role granted a privilege or a role,
or if what goes would abandon a privilege, a role authorization granted
through it, a view, a constraint, an assertion, a trigger, a routine or
a generated column. With CASCADE, what is abandoned goes too, as REVOKE
... CASCADE takes it.

DROP ROLE runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The role exists.
2. It owns no schema. Drop the schema first.
3. With RESTRICT, it granted no privilege and no role, and nothing is
   abandoned.
4. The transaction is not READ ONLY.

## PRIVILEGES

The role is granted WITH ADMIN OPTION to the session's user, to the
role it has set, or to a role applicable for that role.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | There is no such role |
| 2B000 | -103 | The role owns a schema; or, with RESTRICT, it granted a privilege or a role, or something would be abandoned |
| 2A000 | -104 | No identifier the session has enabled holds the role WITH ADMIN OPTION |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

ANN has granted SELECT on STAFF to the role CLERK WITH GRANT OPTION, and
CLERK to BOB. BOB grants it on as CLERK:

```sql
SET ROLE 'CLERK';
GRANT SELECT ON ANN.STAFF TO CAROL GRANTED BY CURRENT_ROLE;
COMMIT;
```

Then ANN, who created CLERK and holds it WITH ADMIN OPTION:

```sql
DROP ROLE CLERK RESTRICT;
ROLLBACK;
DROP ROLE CLERK CASCADE;
COMMIT;
```

```
turborel: SQLCODE -103 (SQLSTATE 2B000): role CLERK granted a privilege or a role, which depends on it, so it is dropped only with CASCADE [ISO/IEC 9075-2:2023 §12.6 NOTE 652] (at character 10)
OK
OK
OK
```

CAROL's privilege went with the role that granted it:

```sql
SELECT NAME FROM ANN.STAFF;
```

```
turborel: SQLCODE -104 (SQLSTATE 2A000): CAROL has no SELECT privilege on ANN.STAFF [X3.135-1986 §5.25 SR1; X3.135-1986 §6.6 SR3] (at character 17)
```

## STANDARD

ISO/IEC 9075-2:2023 §12.6, Feature T331, which revokes the role from
each grantee as REVOKE R FROM A does. Revoking it by any grantor, and
taking the privileges and roles granted to the role or by it, is this
implementation's reading, so that no descriptor names a role that is no
more. The refusal for a role that owns a schema is this
implementation's, since no statement gives a schema another owner.

## SEE ALSO

[CREATE ROLE](create-role.md), [REVOKE](revoke.md), [DROP SCHEMA](drop-schema.md)
