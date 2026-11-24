# DROP TRANSFORM

[Statements](README.md)

## NAME

DROP TRANSFORM: drop transform groups of a user-defined type

## SYNOPSIS

```
<drop transform statement> ::=
    DROP { TRANSFORM | TRANSFORMS } { ALL | <group name> } FOR <user-defined type name> <drop behavior>

<drop behavior> ::= CASCADE | RESTRICT
```

## DESCRIPTION

DROP TRANSFORM drops a type's group of the name, or every group of the
type with ALL, and leaves their functions. No routine of turborel uses
a transform, so CASCADE and RESTRICT drop alike.

DROP TRANSFORM runs within the session's transaction: ROLLBACK undoes
it.

## RULES

1. The type exists, its user owns it, and it has a group of the name,
   or with ALL, any group.

## PRIVILEGES

The owner of the type.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -103 | A rule above is broken |
| 42000 | -104 | Its user does not own the type |

## EXAMPLES

```sql
CREATE TYPE TEMP AS (DEGREES INTEGER) FINAL;
CREATE FUNCTION TEMP_OUT (T TEMP) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN T.DEGREES;
CREATE TRANSFORM FOR TEMP HOST (FROM SQL WITH FUNCTION TEMP_OUT (TEMP)) CLIENT (FROM SQL WITH FUNCTION TEMP_OUT (TEMP));
DROP TRANSFORM HOST FOR TEMP RESTRICT;
SELECT GROUP_NAME FROM INFORMATION_SCHEMA.TRANSFORMS;
DROP TRANSFORM ALL FOR TEMP RESTRICT;
SELECT COUNT(*) FROM INFORMATION_SCHEMA.TRANSFORMS;
```

```
OK
OK
OK
OK
 GROUP_NAME
--------------------------------------------------------------
 CLIENT
(1 row)
OK

----------------------
                    0
(1 row)
```

```sql
DROP TRANSFORM HOST FOR TEMP RESTRICT;
```

```
turborel: SQLCODE -103 (SQLSTATE 42000): type ANN.TEMP has no transform group HOST [ISO/IEC 9075-2:2023 §11.71 SR2] (at character 24)
```

## STANDARD

ISO/IEC 9075-2:2023 §11.71, Feature S241. See
[CONFORMANCE.md](../../../CONFORMANCE.md#structured-types).

## SEE ALSO

[CREATE TRANSFORM](create-transform.md), [ALTER TRANSFORM](alter-transform.md)
