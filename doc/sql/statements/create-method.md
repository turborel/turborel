# CREATE METHOD

[Statements](README.md)

## NAME

CREATE METHOD: give a method of a user-defined type its body

## SYNOPSIS

```
<SQL-invoked method> ::= CREATE <method specification designator> <routine body>

<method specification designator> ::=
      SPECIFIC METHOD <specific method name>
    | [ INSTANCE | STATIC | CONSTRUCTOR ] METHOD <method name>
      ( [ <SQL parameter declaration> [ { , <SQL parameter declaration> }... ] ] )
      [ RETURNS <data type> [ SELF AS RESULT ] ]
      FOR <user-defined type name>

<method invocation> ::=
      <value expression primary> . <method name> [ ( <arguments> ) ]
    | ( <value expression primary> AS <data type> ) . <method name> [ ( <arguments> ) ]
    | <user-defined type name> :: <method name> [ ( <arguments> ) ]
    | NEW <user-defined type name> ( <arguments> )
```

## DESCRIPTION

A user-defined type declares its methods, in CREATE TYPE or ALTER TYPE
ADD, and CREATE METHOD gives one its body. The designator names the
specification by its specific name, or by its kind, name and parameter
types, which match the specification's.

An instance method is invoked of a value, `p.m(args)`, and its body
names the value SELF. Of the methods of the name, the one of the value's
most specific type runs, so an OVERRIDING method of a subtype runs for a
value of the subtype. `(p AS t).m()` runs the method of `t` instead. A
static method is invoked of the type, `t::m(args)`. A constructor method
is invoked by NEW, `NEW t(args)`, and its body changes SELF, a new value
of the type, and returns it. NEW with no arguments gives a new value of
the type's defaults.

DROP METHOD drops a body, as DROP FUNCTION drops a function, and leaves
the specification. The body's text is kept and bound again where the
method is invoked, as a function's is.

CREATE METHOD runs within the session's transaction, as other
definitions do. INFORMATION_SCHEMA.METHOD_SPECIFICATIONS shows the
specifications, and ROUTINES the methods.

## RULES

1. The type exists in the schema of the statement, and its user owns it.
2. The specification exists, of the kind and parameter types written,
   with the parameters' names and the result type it declares, and has
   no body yet.
3. The body returns a value assignable to the result type.

## PRIVILEGES

The owner of the type's schema, who holds EXECUTE on the method with
grant option. Invoking a method needs EXECUTE on it.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A rule above is broken, or no method takes the arguments |
| 2A000 | -104 | Its user does not own the type's schema, or lacks EXECUTE on a method invoked |

## EXAMPLES

```sql
CREATE TYPE SHAPE AS (NAME CHARACTER VARYING(10)) NOT FINAL
    METHOD AREA () RETURNS INTEGER CONTAINS SQL,
    STATIC METHOD UNIT () RETURNS SHAPE CONTAINS SQL;
CREATE TYPE SQUARE UNDER SHAPE AS (SIDE INTEGER) NOT FINAL
    OVERRIDING METHOD AREA () RETURNS INTEGER;
CREATE METHOD AREA () RETURNS INTEGER FOR SHAPE RETURN 0;
CREATE METHOD AREA () RETURNS INTEGER FOR SQUARE RETURN SELF.SIDE * SELF.SIDE;
CREATE STATIC METHOD UNIT () RETURNS SHAPE FOR SHAPE RETURN NEW SHAPE().NAME('UNIT');
CREATE TABLE FIGURES (K INTEGER, F SHAPE);
INSERT INTO FIGURES VALUES (1, SHAPE::UNIT()), (2, NEW SQUARE().SIDE(3));
SELECT K, F.AREA(), (F AS SHAPE).AREA() FROM FIGURES ORDER BY K;
```

```
OK
OK
OK
OK
OK
OK
OK
            K |              |
--------------+--------------+--------------
            1 |            0 |            0
            2 |            9 |            0
(2 rows)
```

```sql
CREATE METHOD AREA () RETURNS INTEGER FOR SHAPE RETURN 1;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): method AREA of type ANN.SHAPE has a body already, routine ANN.SHAPE_AREA_1 [ISO/IEC 9075-2:2023 §11.60 SR20b] (at character 0)
```

## STANDARD

ISO/IEC 9075-2:2023 §6.18, §6.19, §6.20, §11.60, Features S023,
S024, S027. See
[CONFORMANCE.md](../../../CONFORMANCE.md#structured-types).

## SEE ALSO

[CREATE TYPE](create-type.md), [ALTER TYPE](alter-type.md),
[DROP ROUTINE](drop-routine.md),
[Data Types](../02-data-types.md#structured-types)
