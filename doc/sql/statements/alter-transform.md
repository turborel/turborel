# ALTER TRANSFORM

[Statements](README.md)

## NAME

ALTER TRANSFORM: add or drop the functions of a transform group

## SYNOPSIS

```
<alter transform statement> ::=
    ALTER { TRANSFORM | TRANSFORMS } FOR <user-defined type name> <alter group>...

<alter group> ::= <group name> ( <alter transform action> [ { , <alter transform action> }... ] )

<alter transform action> ::=
      ADD ( <transform element> [ , <transform element> ] )
    | DROP ( <transform kind> <drop behavior> [ , <transform kind> <drop behavior> ] )

<transform kind> ::= TO SQL | FROM SQL
```

## DESCRIPTION

ALTER TRANSFORM changes a group CREATE TRANSFORM defined. ADD adds a
function of a kind the group lacks, as CREATE TRANSFORM checks one.
DROP drops the group's function of a kind, and leaves the function
itself.

ALTER TRANSFORM runs within the session's transaction, as other
definitions do.

## RULES

1. The type has a group of each name written, and its user owns the
   type.
2. ADD adds a function of a kind the group has none of.
3. DROP drops a function of a kind the group has.

## PRIVILEGES

The owner of the type, who needs EXECUTE on each function added.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken, or a function added breaks a rule of CREATE TRANSFORM |
| 2A000 | -104 | Its user does not own the type, or lacks EXECUTE on a function |

## EXAMPLES

```sql
CREATE TYPE TEMP AS (DEGREES INTEGER) FINAL;
CREATE FUNCTION TEMP_OUT (T TEMP) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN T.DEGREES;
CREATE FUNCTION TEMP_IN (N INTEGER) RETURNS TEMP DETERMINISTIC CONTAINS SQL RETURN NEW TEMP().DEGREES(N);
CREATE TRANSFORM FOR TEMP HOST (FROM SQL WITH FUNCTION TEMP_OUT (TEMP));
ALTER TRANSFORM FOR TEMP HOST (ADD (TO SQL WITH FUNCTION TEMP_IN (INTEGER)));
ALTER TRANSFORM FOR TEMP HOST (DROP (FROM SQL RESTRICT));
SELECT GROUP_NAME, TRANSFORM_TYPE FROM INFORMATION_SCHEMA.TRANSFORMS;
```

```
OK
OK
OK
OK
OK
OK
 GROUP_NAME                                                   | TRANSFORM_TYPE
--------------------------------------------------------------+--------------------------------------------------------------
 HOST                                                         | TO SQL
(1 row)
```

```sql
ALTER TRANSFORM FOR TEMP HOST (DROP (FROM SQL RESTRICT));
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): group HOST has a FROM SQL function, dropped once [ISO/IEC 9075-2:2023 §11.70 SR3 to SR5] (at character 25)
```

## STANDARD

ISO/IEC 9075-2:2023 §11.68 to §11.70, Feature S241. See
[CONFORMANCE.md](../../../CONFORMANCE.md#structured-types).

## SEE ALSO

[CREATE TRANSFORM](create-transform.md), [DROP TRANSFORM](drop-transform.md)
