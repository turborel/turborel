# SET TRANSFORM GROUP

[Statements](README.md)

## NAME

SET TRANSFORM GROUP: choose the session's transform groups

## SYNOPSIS

```
<set transform group statement> ::= SET <transform group characteristic>

<transform group characteristic> ::=
      DEFAULT TRANSFORM GROUP <value specification>
    | TRANSFORM GROUP FOR TYPE <user-defined type name> <value specification>

<current transform group for type> ::=
    CURRENT_TRANSFORM_GROUP_FOR_TYPE <user-defined type name>
```

## DESCRIPTION

SET DEFAULT TRANSFORM GROUP names the session's group for every
user-defined type, and SET TRANSFORM GROUP FOR TYPE the group of one
type and its subtypes, of which a group named for a subtype is the
subtype's own. The value is a group name, its spaces before and after
trimmed. The names stay until the session ends or another SET changes
them, and a transaction's rollback leaves them.

CURRENT_DEFAULT_TRANSFORM_GROUP gives the default group's name, and
CURRENT_TRANSFORM_GROUP_FOR_TYPE a type's group, that of the type
itself if it has one, else of its nearest supertype that does, else the
default group, else the empty string.

No host language or external routine of turborel uses a transform, so
the names choose no function. They are kept for the programs that read
them.

## RULES

1. FOR TYPE names a user-defined type.
2. The value is of a character string type, and is a group name, one
   identifier, once trimmed.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 0S000 | -301 | The value is no group name |
| 42000 | -103 | The type does not exist, or the value is not a character string |

## EXAMPLES

```sql
CREATE TYPE SHAPE AS (NAME CHARACTER VARYING(10)) NOT FINAL;
CREATE TYPE SQUARE UNDER SHAPE AS (SIDE INTEGER) NOT FINAL;
SET DEFAULT TRANSFORM GROUP 'PLAIN';
SET TRANSFORM GROUP FOR TYPE SHAPE ' DRAWN ';
SELECT CURRENT_DEFAULT_TRANSFORM_GROUP, CURRENT_TRANSFORM_GROUP_FOR_TYPE SQUARE FROM (VALUES (1)) AS T (N);
```

```
OK
OK
OK
OK
                                                              |
--------------------------------------------------------------+--------------------------------------------------------------
 PLAIN                                                        | DRAWN
(1 row)
```

```sql
SET DEFAULT TRANSFORM GROUP 'TWO WORDS';
```

```
turborel: SQLCODE -301 (SQLSTATE 0S000): 'TWO WORDS' is no <group name> [ISO/IEC 9075-2:2023 §19.13 GR1]
```

## STANDARD

ISO/IEC 9075-2:2023 §19.12, §19.13, §6.4, Feature S241. See
[CONFORMANCE.md](../../../CONFORMANCE.md#structured-types).

## SEE ALSO

[CREATE TRANSFORM](create-transform.md)
