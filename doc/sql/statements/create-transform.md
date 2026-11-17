# CREATE TRANSFORM

[Statements](README.md)

## NAME

CREATE TRANSFORM: define transform groups of a user-defined type

## SYNOPSIS

```
<transform definition> ::=
    CREATE { TRANSFORM | TRANSFORMS } FOR <user-defined type name> <transform group>...

<transform group> ::= <group name> ( <transform element> [ , <transform element> ] )

<transform element> ::=
      TO SQL WITH <specific routine designator>
    | FROM SQL WITH <specific routine designator>
```

## DESCRIPTION

A transform group names a function that takes a value of a user-defined
type FROM SQL, to a predefined type, and one that makes a value of the
type TO SQL from one. A host language or an external routine would
exchange the type's values through them. turborel has neither, so its
values cross to a client whole, and the groups are kept, described and
chosen by SET TRANSFORM GROUP for the programs that read them.

CREATE TRANSFORM runs within the session's transaction, as other
definitions do. INFORMATION_SCHEMA.TRANSFORMS shows the groups.

## RULES

1. The type exists, its user owns it, and neither it nor a proper
   supertype or subtype of it has a group of a name written, nor is a
   name written twice.
2. Each group has at most one function of each kind.
3. A FROM SQL function takes one parameter of the type and returns a
   predefined type. A TO SQL function takes one parameter of a
   predefined type, or is a method of the type taking one, and returns
   the type.
4. The functions are DETERMINISTIC and modify no SQL data, and the two
   of a group trade the type with compatible predefined types.

## PRIVILEGES

The owner of the type, who needs EXECUTE on each function.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the type, or lacks EXECUTE on a function |

## EXAMPLES

```sql
CREATE TYPE TEMP AS (DEGREES INTEGER) FINAL;
CREATE FUNCTION TEMP_OUT (T TEMP) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN T.DEGREES;
CREATE FUNCTION TEMP_IN (N INTEGER) RETURNS TEMP DETERMINISTIC CONTAINS SQL RETURN NEW TEMP().DEGREES(N);
CREATE TRANSFORM FOR TEMP HOST (FROM SQL WITH FUNCTION TEMP_OUT (TEMP), TO SQL WITH FUNCTION TEMP_IN (INTEGER));
SELECT GROUP_NAME, TRANSFORM_TYPE, SPECIFIC_NAME FROM INFORMATION_SCHEMA.TRANSFORMS ORDER BY 2;
```

```
OK
OK
OK
OK
 GROUP_NAME                                                   | TRANSFORM_TYPE                                               | SPECIFIC_NAME
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 HOST                                                         | FROM SQL                                                     | TEMP_OUT_1
 HOST                                                         | TO SQL                                                       | TEMP_IN_2
(2 rows)
```

```sql
CREATE TRANSFORM FOR TEMP HOST (FROM SQL WITH FUNCTION TEMP_OUT (TEMP));
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): type ANN.TEMP has a transform group HOST already [ISO/IEC 9075-2:2023 §11.67 SR2, SR4d] (at character 26)
```

## STANDARD

ISO/IEC 9075-2:2023 §11.67, Feature S241. See
[CONFORMANCE.md](../../../CONFORMANCE.md#structured-types).

## SEE ALSO

[ALTER TRANSFORM](alter-transform.md), [DROP TRANSFORM](drop-transform.md),
[SET TRANSFORM GROUP](set-transform-group.md)
