# ALTER TYPE

[Statements](README.md)

## NAME

ALTER TYPE: add or drop a structured type's attributes and methods

## SYNOPSIS

```
<alter type statement> ::= ALTER TYPE <user-defined type name> <alter type action>

<alter type action> ::=
      ADD ATTRIBUTE <attribute definition>
    | DROP ATTRIBUTE <attribute name> RESTRICT
    | ADD <method specification>
    | DROP <specific method specification designator> RESTRICT

<specific method specification designator> ::=
    [ INSTANCE | STATIC | CONSTRUCTOR ] METHOD <method name> ( [ <data type> [ { , <data type> }... ] ] )
```

## DESCRIPTION

ALTER TYPE changes a structured type. ADD ATTRIBUTE adds an attribute
after the type's others, with its observer and mutator. DROP ATTRIBUTE
drops one the type itself defines, with its observer and mutator.

ADD adds a method specification, as CREATE TYPE writes one. An
OVERRIDING one overrides a method of a supertype. Its body is given by
[CREATE METHOD](create-method.md). DROP drops a method specification
whose method has no body, or whose body DROP METHOD dropped first.

A distinct type takes ADD and DROP of a method alone.

ALTER TYPE runs within the session's transaction, as other definitions
do.

## RULES

1. The type exists, and its user owns the type's schema.
2. No attribute of the type or of a supertype or subtype has the name an
   added one takes, and a dropped one is the type's own, not a
   supertype's, and not its last.
3. No base table has a column of the type, of a subtype or of a
   supertype while an attribute is added or dropped.
4. An added method is told apart from the type's others, as CREATE TYPE
   tells them apart. A dropped one has no body.

## PRIVILEGES

The owner of the type's schema.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The type does not exist, or a rule above is broken |
| 2A000 | -104 | Its user does not own the type's schema |

## EXAMPLES

```sql
CREATE TYPE POINT AS (X INTEGER, Y INTEGER) NOT FINAL;
ALTER TYPE POINT ADD ATTRIBUTE Z INTEGER;
ALTER TYPE POINT ADD METHOD NORM () RETURNS INTEGER;
CREATE METHOD NORM () RETURNS INTEGER FOR POINT RETURN SELF.X * SELF.X + SELF.Y * SELF.Y;
CREATE TABLE SHAPES (K INTEGER, P POINT);
INSERT INTO SHAPES VALUES (1, NEW POINT());
UPDATE SHAPES SET P.X = 3, P.Y = 4;
SELECT P.NORM() FROM SHAPES;
```

```
OK
OK
OK
OK
OK
OK
OK

--------------
           25
(1 row)
```

```sql
ALTER TYPE POINT DROP ATTRIBUTE Z RESTRICT;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): (RULES.md D81) column ANN.SHAPES.P is of a type of the family of ANN.POINT, whose stored values keep their attributes [ISO/IEC 9075-2:2023 §11.55 SR6] (at character 32)
```

```sql
DROP TABLE SHAPES CASCADE;
DROP METHOD NORM () FOR POINT RESTRICT;
ALTER TYPE POINT DROP METHOD NORM () RESTRICT;
ALTER TYPE POINT DROP ATTRIBUTE Z RESTRICT;
```

```
OK
OK
OK
OK
```

## STANDARD

ISO/IEC 9075-2:2023 §11.53 to §11.58, Features S023, S024. See
[CONFORMANCE.md](../../../CONFORMANCE.md#structured-types).

## SEE ALSO

[CREATE TYPE](create-type.md), [CREATE METHOD](create-method.md),
[DROP TYPE](drop-type.md),
[Data Types](../02-data-types.md#structured-types)
