# GRANT DEBUG

[Statements](README.md)

## NAME

GRANT DEBUG: let a user debug a routine *(extension)*

## SYNOPSIS

```
<grant debug statement> ::=
    GRANT DEBUG [ , EXECUTE ] ON <routine object>
    TO <grantee> [ { , <grantee> }... ]
    [ WITH GRANT OPTION ]
    [ GRANTED BY <grantor> ]

<revoke debug statement> ::=
    REVOKE [ GRANT OPTION FOR ] DEBUG [ , EXECUTE ] ON <routine object>
    FROM <grantee> [ { , <grantee> }... ]
    [ GRANTED BY <grantor> ] <drop behavior>

<routine object> ::=
      SPECIFIC <routine type> <specific name>
    | <routine type> <routine name> [ ( [ <data type> [ { , <data type> }... ] ] ) ]

<routine type> ::= ROUTINE | FUNCTION | PROCEDURE
```

DEBUG is one `<action>` of [GRANT](grant.md) and [REVOKE](revoke.md),
written beside EXECUTE or alone. It is a word, not a reserved one.

## DESCRIPTION

DEBUG on a procedure or a function lets its holder debug the routine's
executions in another session: stop them at breakpoints, step through
their statements, and read the values of their parameters, variables
and cursors. It
lets its holder neither invoke the routine nor read its text; EXECUTE
and ownership do those.

The owner of the routine's schema holds DEBUG on it by owning it, with
grant option, as the owner of a domain holds USAGE; it is held by no
grant, so ROUTINE_PRIVILEGES shows no row of it. A grant is kept, and
revoked, as a grant of EXECUTE is: to a user, a role or PUBLIC, WITH
GRANT OPTION if its grantees may grant it in turn. A revoke of DEBUG
abandons no definition, as none needs it, but a grant made by the grant
option it takes: with RESTRICT the revoke is then refused, with CASCADE
that grant goes too.

ALL PRIVILEGES on a routine grants and revokes EXECUTE alone, the
privileges the standard defines, so DEBUG is granted and revoked only by
name.

INFORMATION_SCHEMA.ROUTINE_PRIVILEGES shows each grant of DEBUG, its
PRIVILEGE_TYPE DEBUG.

## RULES

1. The routine exists, and its name designates exactly one routine of
   its routine type, as of [GRANT](grant.md).
2. The routine is a schema-level routine: of a module's routine no
   privilege is granted.
3. DEBUG is granted of a routine alone, not of a table, a module or any
   other object.
4. The grantor holds DEBUG on the routine: by owning it, or granted it.

## PRIVILEGES

DEBUG on the routine, with grant option to grant it.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -103 | the routine is not of its routine type, is of a module, or the object is not a routine |
| 42000 | -104 | the grantor holds no DEBUG on the routine |
| 01007 | 0 | the grantor holds DEBUG without grant option: nothing is granted, a warning |
| 2B000 | -103 | REVOKE with RESTRICT takes a grant option another grant was made by |
| 01006 | 0 | REVOKE of a grant the grantor did not make: a warning |

## EXAMPLES

ANN owns the procedure ADD_STAFF and lets BOB debug it:

```sql
CREATE TABLE STAFF (ID INTEGER, NAME VARCHAR(20));
CREATE PROCEDURE ADD_STAFF (IN I INTEGER, IN N VARCHAR(20)) MODIFIES SQL DATA
    BEGIN
        DECLARE M VARCHAR(20);
        SET M = UPPER(N);
        INSERT INTO STAFF VALUES (I, M);
    END;
GRANT DEBUG ON PROCEDURE ADD_STAFF TO BOB WITH GRANT OPTION;
GRANT DEBUG ON TABLE STAFF TO BOB;
COMMIT;
```

```
OK
OK
OK
turborel: SQLCODE -103 (SQLSTATE 42000): DEBUG is granted on a routine, not on table ANN.STAFF (at character 21)
OK
```

The grant, as ROUTINE_PRIVILEGES shows it:

```sql
SELECT GRANTOR, GRANTEE, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.ROUTINE_PRIVILEGES
    WHERE ROUTINE_NAME = 'ADD_STAFF' AND PRIVILEGE_TYPE = 'DEBUG';
```

```
 GRANTOR                                                      | GRANTEE                                                      | PRIVILEGE_TYPE                                               | IS_GRANTABLE
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 ANN                                                          | BOB                                                          | DEBUG                                                        | YES
(1 row)
```

Taken back, the second time with nothing to take:

```sql
REVOKE DEBUG ON PROCEDURE ADD_STAFF FROM BOB CASCADE;
REVOKE DEBUG ON PROCEDURE ADD_STAFF FROM BOB CASCADE;
COMMIT;
```

```
OK
OK, with a warning: SQLSTATE 01006
OK
```

## STANDARD

DEBUG is turborel's *(extension)*: ISO/IEC 9075-2:2023 §12.3 defines
EXECUTE alone of a routine [SR4d]. It is granted and revoked by the
rules of §12.2 and §12.7 that EXECUTE is.

## SEE ALSO

[GRANT](grant.md), [REVOKE](revoke.md),
[CREATE PROCEDURE](create-procedure.md),
[CREATE FUNCTION](create-function.md)
