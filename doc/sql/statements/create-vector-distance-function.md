# CREATE VECTOR DISTANCE FUNCTION *(extension)*

[Statements](README.md)

## NAME

CREATE VECTOR DISTANCE FUNCTION: define a metric of vectors in SQL

## SYNOPSIS

```
<distance function definition> ::=
    CREATE VECTOR DISTANCE FUNCTION <schema qualified routine name>
    ( <parameter name> VECTOR , <parameter name> VECTOR )
    RETURNS { FLOAT64 | <data type> }
    [ DETERMINISTIC ]
    [ PROPERTIES ( <distance property> [ , <distance property> ]... ) ]
    [ VALID FORMATS ( <format> [ , <format> ]... ) ]
    LANGUAGE SQL
    AS <character string literal>

<distance property> ::= SYMMETRIC | NON_NEGATIVE | TRIANGLE_INEQUALITY
<format> ::= FLOAT32 | FLOAT16 | FLOAT64 | INT8 | BINARY
```

VECTOR, DISTANCE, FUNCTION, FLOAT64, PROPERTIES, VALID and FORMATS are
read as words, not key words.

## DESCRIPTION

CREATE VECTOR DISTANCE FUNCTION defines a function of two vectors whose
value is a distance between them, and which
[VECTOR_DISTANCE](../14-vectors.md#distances) then names as a metric,
by its name, qualified or not, as a key word or a string. The literal
is the function's body, a value expression of its two parameters, a
number. FLOAT64 is DOUBLE PRECISION.

The function is an SQL-invoked function of a RETURN of its body, which
returns null of a null argument and reads no SQL data, so it is listed
in ROUTINES and dropped by
[DROP VECTOR DISTANCE FUNCTION](drop-vector-distance-function.md). Its
owner holds EXECUTE on it and may grant it to others.

The properties say what the distance is: SYMMETRIC, the same in either
order; NON_NEGATIVE, never below zero; TRIANGLE_INEQUALITY, never more
from a to c than from a to b and then b to c. VALID FORMATS names the
formats of the vectors it measures, every format if it names none.

A vector index measures by a built-in metric or one a program
registers, not by a distance function of SQL
([CREATE VECTOR INDEX](create-vector-index.md)).

## RULES

1. The function has two parameters, each of type VECTOR.
2. Its body is a value expression of its parameters, a number, and has
   no query. Its body binds when it is defined.
3. Its name is not that of a built-in or registered metric.
4. A property or format is written once.
5. A vector measured is of a format VALID FORMATS names; of another, the
   call raises SQLSTATE 22000 (SQLCODE -301).

## PRIVILEGES

The user owns the schema. VECTOR_DISTANCE by the function needs EXECUTE
on it.

## ERRORS

| SQLSTATE | SQLCODE | Raised when |
|---|---|---|
| 2A000 | -103 | a parameter is not VECTOR, or there are not two; a property or format is written twice or unknown; the body is no number or has a query; the name is a metric's |
| 0A000 | -900 | another language than SQL |
| 2A000 | -104 | VECTOR_DISTANCE by the function, by a user who holds no EXECUTE on it |

## EXAMPLES

```sql
CREATE TABLE POINTS (ID INTEGER, P VECTOR(2));
INSERT INTO POINTS VALUES (1, '[0, 0]'), (2, '[3, 4]'), (3, '[1, 1]');
CREATE VECTOR DISTANCE FUNCTION CHESSBOARD (A VECTOR, B VECTOR) RETURNS FLOAT64 DETERMINISTIC
    PROPERTIES (SYMMETRIC, NON_NEGATIVE, TRIANGLE_INEQUALITY) VALID FORMATS (FLOAT32)
    LANGUAGE SQL AS 'VECTOR_NORM(A - B, NORMINF)';
SELECT ID, VECTOR_DISTANCE(P, '[0, 0]', CHESSBOARD) AS D FROM POINTS ORDER BY ID;
SELECT FUNCTION_NAME, SYMMETRIC, NON_NEGATIVE, TRIANGLE_INEQUALITY, FORMATS FROM INFORMATION_SCHEMA.VECTOR_DISTANCE_FUNCTIONS;
```

```
OK
OK
OK
           ID |                        D
--------------+--------------------------
            1 |                      0E0
            2 |                      4E0
            3 |                      1E0
(3 rows)
 FUNCTION_NAME                                                | SYMMETRIC | NON_NEGATIVE | TRIANGLE_INEQUALITY | FORMATS
--------------------------------------------------------------+-----------+--------------+---------------------+--------------------------------------------------------------
 CHESSBOARD                                                   | YES       | YES          | YES                 | FLOAT32
(1 row)
```

```sql
CREATE VECTOR DISTANCE FUNCTION ONE (A VECTOR) RETURNS FLOAT64 LANGUAGE SQL AS '1';
CREATE VECTOR DISTANCE FUNCTION COSINE (A VECTOR, B VECTOR) RETURNS FLOAT64 LANGUAGE SQL AS '1';
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): a distance function has two parameters, each VECTOR (at character 47)
turborel: SQLCODE -103 (SQLSTATE 2A000): a distance function is named as no metric is, and COSINE is one (at character 32)
```

## STANDARD

CREATE VECTOR DISTANCE FUNCTION is an extension. No SQL standard has
vectors.

## SEE ALSO

[DROP VECTOR DISTANCE FUNCTION](drop-vector-distance-function.md),
[CREATE FUNCTION](create-function.md), [CREATE VECTOR INDEX](create-vector-index.md),
[Vectors](../14-vectors.md)
