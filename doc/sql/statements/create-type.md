# CREATE TYPE

[Statements](README.md)

## NAME

CREATE TYPE: define a distinct or structured type

## SYNOPSIS

```
<user-defined type definition> ::=
    CREATE TYPE <user-defined type name> [ UNDER <supertype name> ]
    [ AS <representation> ] [ <type option>... ]
    [ <method specification> [ { , <method specification> }... ] ]

<representation> ::=
      <predefined type> | <array type> | <multiset type>
    | ( <attribute definition> [ { , <attribute definition> }... ] )

<attribute definition> ::= <attribute name> <data type> [ DEFAULT <default option> ] [ COLLATE <collation name> ]

<type option> ::=
      [ NOT ] INSTANTIABLE
    | [ NOT ] FINAL
    | REF IS SYSTEM GENERATED
    | REF USING <predefined type>
    | REF FROM ( <attribute name> [ { , <attribute name> }... ] )
    | CAST ( SOURCE AS REF ) WITH <identifier>
    | CAST ( REF AS SOURCE ) WITH <identifier>
    | CAST ( SOURCE AS DISTINCT ) WITH <function name>
    | CAST ( DISTINCT AS SOURCE ) WITH <function name>

<method specification> ::=
      [ INSTANCE | STATIC | CONSTRUCTOR ] METHOD <method name>
      ( [ <SQL parameter declaration> [ { , <SQL parameter declaration> }... ] ] )
      RETURNS <data type> [ SELF AS RESULT ] [ SPECIFIC <specific method name> ]
      [ <method characteristic>... ]
    | OVERRIDING [ INSTANCE ] METHOD <method name> ( ... ) RETURNS <data type>

<routine invocation> ::= [ <schema name> . ] <function name> ( <value expression> )
```

## DESCRIPTION

A distinct type is a type of its own made from its source type. Its
values are those of the source, but they compare only with values of the
same distinct type, so values of two types of one source, such as prices
and weights, cannot be compared by mistake.

The source is a predefined type, an array type or a multiset type. An exact numeric
source is kept as NUMERIC of its precision and scale, so INTEGER is
NUMERIC(10,0), and an approximate one as FLOAT of its precision.

A value is assigned to a column of the type if it is of the type, or of
a type that is assigned to the source, such as an integer to a type of
DECIMAL. A value of the type is assigned only to a site of exactly the
source type. CAST takes the same paths and no other. The values compare,
order and group as the source's do, so a column of the type may be a key
or be indexed. A type whose source is an array or multiset type
compares with nothing, and one of a multiset is taken wherever a
multiset is, by the multiset operators and predicates. No operator but comparison takes the type's values. Arithmetic,
SUM and the string functions are refused, and the values are taken out
first by the function to the source.

The definition makes two functions. The first makes a value of the type
from one of its source and is named for the type, or as `CAST (SOURCE AS
DISTINCT) WITH` names it. The second gives a value's source value and is
named for the source type, NUMERIC, CHARACTER and so on, or as `CAST
(DISTINCT AS SOURCE) WITH` names it. A function named by a key word, as
NUMERIC is, is invoked delimited, `"NUMERIC"(PRICE)`. A name not
qualified is found in the session's default schema, or in the schema
statement's own. Two types may name their functions alike. Of functions
of one name, the argument's type chooses, so two types of NUMERIC and
INTEGER sources may each name their function to the source AMOUNT. Their
specific names, which GRANT and REVOKE may name, are the type's name and
_TO_DISTINCT or _TO_SOURCE.

A structured type is written AS a list of attributes, or UNDER a
supertype whose attributes come first. A value has a value of each
attribute, and is of a most specific type, the type it was made of. A
value of a subtype is a value of its supertypes too, and is assigned to
a site of any of them. NEW makes a value of the attributes' defaults,
`v.a` observes an attribute and `v.a(x)` gives a copy with the
attribute changed. An UPDATE sets one by `SET c.a = x`. A type NOT
INSTANTIABLE has no value made of it but a subtype's, and one FINAL has
no subtypes. A structured type compares by nothing until [CREATE
ORDERING](create-ordering.md) gives it an ordering.

The method specifications declare the type's methods, whose bodies
[CREATE METHOD](create-method.md) gives. An OVERRIDING one gives a
subtype its own body of a supertype's method. A distinct type may have
methods too, but none OVERRIDING.

A reference type specification says how a reference to a row of a
typed table of the type is made, by the system, of a predefined type
the user gives, or from attributes. [CREATE TABLE](create-table.md)
`OF` the type makes such a table.

A client reads and writes a value of a distinct type as a value of its source,
and DESCRIBE describes it as TYPE 17, with the type's name.

CREATE TYPE runs within the session's transaction, as other definitions
do. INFORMATION_SCHEMA.USER_DEFINED_TYPES shows the types, and ROUTINES
the functions.

## RULES

1. The schema exists, and no type or domain of the schema has its name.
2. A distinct type's source is a predefined, an array or a multiset type,
   and no row type, distinct type or domain.
3. Each option is written at most once. NOT FINAL, INSTANTIABLE, UNDER,
   REF and the CASTs to REF and TYPE are of structured types alone, and
   the CASTs to DISTINCT and SOURCE of distinct types alone.
4. A structured type is INSTANTIABLE unless NOT INSTANTIABLE is written,
   and says FINAL or NOT FINAL. A supertype exists, is NOT FINAL and its
   user holds UNDER on it. No attribute is of the type itself, nor named
   as another of the type or a supertype.
5. A method specification is told apart from every other of its name of
   the type and its supertypes by its parameters' types, and an
   OVERRIDING one matches a supertype's method. An original one says
   CONTAINS SQL, READS SQL DATA or MODIFIES SQL DATA.
6. Each of a distinct type's two functions is told apart from every other function of
   its name in the schema, an SQL-invoked function's or another type's,
   as [CREATE FUNCTION](create-function.md) tells overloads apart.
7. A column of a distinct type defaults to NULL alone, or to ARRAY[] if the
   source is an array type, or MULTISET[] if it is a multiset type.

## PRIVILEGES

Its schema's owner, who holds USAGE on the type and EXECUTE on its
functions and methods with grant option, and UNDER on a structured
type. A subtype needs UNDER on its supertype. Naming the type needs USAGE, and invoking a
function, or a CAST to or from the type, needs EXECUTE on the function.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 3F000 | -103 | The schema does not exist |
| 2A000 | -103 | A rule above is broken, or a value of the type meets an operator or a type it is not taken by |
| 2A000 | -104 | Its user does not own the schema, or lacks USAGE on the type or EXECUTE on a function |
| 0A000 | -900 | A method specification says SELF AS LOCATOR |

## EXAMPLES

```sql
CREATE TYPE MONEY AS DECIMAL(10,2);
CREATE TYPE GRAMS AS INTEGER CAST (DISTINCT AS SOURCE) WITH WEIGHT;
CREATE TABLE PARCELS (ID INTEGER, PRICE MONEY, MASS GRAMS);
INSERT INTO PARCELS VALUES (1, 12.5, 400), (2, 8, 250);
SELECT ID, PRICE FROM PARCELS WHERE PRICE > MONEY(10);
SELECT SUM("NUMERIC"(PRICE)), MAX(MASS) FROM PARCELS;
SELECT WEIGHT(MASS) * 2 FROM PARCELS ORDER BY 1;
SELECT ID FROM PARCELS WHERE PRICE > 10;
```

```
OK
OK
OK
OK
           ID |        PRICE
--------------+--------------
            1 |        12.50
(1 row)
                                          |
------------------------------------------+--------------
                                    20.50 |          400
(1 row)

---------------
           500
           800
(2 rows)
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.MONEY and NUMERIC(2,0) are not comparable [ISO/IEC 9075:1992 §8.2 SR1, SR3] (at character 29)
```

```sql
SELECT ID FROM PARCELS WHERE PRICE = MASS;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.MONEY and ANN.GRAMS are not comparable [ISO/IEC 9075:1992 §8.2 SR1, SR3] (at character 29)
```

```sql
CREATE TYPE BAD AS MONEY;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): a distinct type's representation is a <predefined type> or a <collection type>, not MONEY [ISO/IEC 9075-2:2023 §11.51 SR5a] (at character 19)
```

A structured type and a subtype of it, and a value of each:

```sql
CREATE TYPE PLACE AS (NAME CHARACTER VARYING(20)) NOT FINAL;
CREATE TYPE CITY UNDER PLACE AS (PEOPLE INTEGER) NOT FINAL;
CREATE TABLE STOPS (K INTEGER, P PLACE);
INSERT INTO STOPS VALUES (1, NEW PLACE().NAME('PASS')), (2, NEW CITY().NAME('OSLO').PEOPLE(700000));
SELECT K, P.NAME, P IS OF (CITY) FROM STOPS ORDER BY K;
SELECT TREAT(P AS CITY).PEOPLE FROM STOPS WHERE P IS OF (CITY);
DROP TABLE STOPS CASCADE;
DROP TYPE PLACE CASCADE;
```

```
OK
OK
OK
OK
            K | NAME                 |
--------------+----------------------+-------
            1 | PASS                 | FALSE
            2 | OSLO                 | TRUE
(2 rows)

--------------
       700000
(1 row)
OK
OK
```

## STANDARD

ISO/IEC 9075-2:2023 §4.9, §11.51, §9.18, §10.4, Features S011, S023,
S024, S025, S026, S028, S401. See
[CONFORMANCE.md](../../../CONFORMANCE.md#distinct-types) and
[structured types](../../../CONFORMANCE.md#structured-types).

## SEE ALSO

[DROP TYPE](drop-type.md), [ALTER TYPE](alter-type.md),
[CREATE METHOD](create-method.md), [CREATE ORDERING](create-ordering.md),
[CREATE CAST](create-cast.md), [GRANT](grant.md), [REVOKE](revoke.md),
[Data Types](../02-data-types.md#distinct-types)
