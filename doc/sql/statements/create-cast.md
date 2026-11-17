# CREATE CAST

[Statements](README.md)

## NAME

CREATE CAST: define a cast of a user-defined type by a function

## SYNOPSIS

```
<user-defined cast definition> ::=
    CREATE CAST ( <source data type> AS <target data type> )
    WITH <specific routine designator> [ AS ASSIGNMENT ]

<specific routine designator> ::=
      SPECIFIC FUNCTION <specific name>
    | FUNCTION <function name> [ ( [ <data type> [ { , <data type> }... ] ] ) ]
```

## DESCRIPTION

A user-defined cast makes CAST of a value of the source type to the
target type invoke its function. Either type is a structured, distinct
or reference type, and the function takes one parameter of the source
type and returns the target type.

CAST of a value whose type has no cast of its own takes the cast of the
first of the types it is a subtype of, in its type precedence list, that
has one to the target. CAST to a supertype needs no cast, and gives the
value itself.

A cast AS ASSIGNMENT is also taken where a value is stored and its type
is not assignable to its site. That is a column by INSERT, UPDATE or
MERGE, a function's result, and a variable by SET, SELECT INTO or FETCH.
An argument of a routine is never cast so.

CREATE CAST runs within the session's transaction, as other definitions
do.

## RULES

1. No cast of the two types exists.
2. One of the types is a user-defined type, of the schema of the
   statement, and its user owns it.
3. The function exists, is DETERMINISTIC, takes one parameter whose type
   is the source type and returns the target type, and reads or modifies
   no SQL data.

## PRIVILEGES

The owner of the user-defined type, who needs EXECUTE on the function.
CAST invoking it needs EXECUTE on the function too.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the type, or lacks EXECUTE on the function |

## EXAMPLES

```sql
CREATE TYPE TEMP AS (DEGREES INTEGER) FINAL;
CREATE FUNCTION TEMP_TO_INT (T TEMP) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN T.DEGREES;
CREATE CAST (TEMP AS INTEGER) WITH FUNCTION TEMP_TO_INT (TEMP) AS ASSIGNMENT;
CREATE TABLE READINGS (K INTEGER, C INTEGER);
INSERT INTO READINGS VALUES (1, NEW TEMP().DEGREES(21));
SELECT K, C, CAST(NEW TEMP().DEGREES(5) AS INTEGER) FROM READINGS;
```

```
OK
OK
OK
OK
OK
            K |            C |
--------------+--------------+--------------
            1 |           21 |            5
(1 row)
```

```sql
CREATE CAST (TEMP AS INTEGER) WITH FUNCTION TEMP_TO_INT (TEMP);
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): there is a cast from ANN.TEMP to INTEGER already [ISO/IEC 9075-2:2023 §11.63 SR3] (at character 0)
```

## STANDARD

ISO/IEC 9075-2:2023 §6.13, §9.2, §11.63, Feature S211. See
[CONFORMANCE.md](../../../CONFORMANCE.md#structured-types).

## SEE ALSO

[DROP CAST](drop-cast.md), [CREATE FUNCTION](create-function.md),
[CREATE TYPE](create-type.md)
